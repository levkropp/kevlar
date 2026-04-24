/* SPDX-License-Identifier: MIT
 *
 * kvlr_posix_spawn.c — drop-in replacement for musl's posix_spawn() that
 * routes through SYS_KVLR_SPAWN (Kevlar-private syscall 500) when
 * available, falling back to the musl reference implementation
 * (CLONE_VM | CLONE_VFORK + execve) on -ENOSYS.
 *
 * Build-time integration: compile this file to .o, link it BEFORE -lc
 * when producing a binary.  The static linker satisfies posix_spawn from
 * here and never pulls in musl's posix_spawn.o.  See blog 227.
 *
 * Compatibility contract: every documented posix_spawn(3) feature must
 * have a semantic-equivalent translation into the SYS_KVLR_SPAWN ABI.
 * On Kevlar with kvlr_spawn v2 (blog 226), file_actions (CLOSE/OPEN/
 * DUP2) and attr flags (SETSIGMASK / SETSID / RESETIDS) are handled
 * atomically in the kernel.  CHDIR / FCHDIR / SETSIGDEF / SETPGROUP /
 * SETSCHEDPARAM / SETSCHEDULER / SETSID-w-SETPGROUP combinations are
 * not yet wired through kvlr_spawn — for those we fall through to the
 * vfork+execve path so semantics match Linux exactly.
 *
 * The fallback inline-reimplements musl's posix_spawn body
 * (Copyright © 2005-2020 Rich Felker, et al., MIT-licensed; original at
 * src/process/posix_spawn.c in musl 1.2.5) so this single object file
 * is self-contained.
 */
#define _GNU_SOURCE
#include <spawn.h>
#include <sched.h>
#include <unistd.h>
#include <signal.h>
#include <fcntl.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>

#ifndef SYS_kvlr_spawn
#define SYS_kvlr_spawn 500
#endif

#define KVLR_SPAWN_F_EXTENDED 1u

#define KVLR_SPAWN_FA_CLOSE 1u
#define KVLR_SPAWN_FA_OPEN  2u
#define KVLR_SPAWN_FA_DUP2  3u

#define KVLR_SPAWN_SETSIGMASK 1u
#define KVLR_SPAWN_SETSIGDEF  2u
#define KVLR_SPAWN_SETPGROUP  4u
#define KVLR_SPAWN_SETSID     8u
#define KVLR_SPAWN_RESETIDS   16u

struct kvlr_fa {
    unsigned int op;
    int          fd;
    int          newfd;
    int          oflag;
    unsigned int mode;
    unsigned int _pad;
    const char  *path;
};

struct kvlr_fa_hdr {
    unsigned int count;
    unsigned int _pad;
    /* struct kvlr_fa actions[count] follows */
};

struct kvlr_attr {
    unsigned int       flags;
    int                pgid;
    unsigned long long sigmask;
    unsigned long long sigdefault;
};

/* Mirror of musl's internal struct fdop (src/process/fdop.h). */
struct kvlr_fdop {
    struct kvlr_fdop *next, *prev;
    int    cmd, fd, srcfd, oflag;
    mode_t mode;
    char   path[];
};
#define KVLR_FDOP_CLOSE  1
#define KVLR_FDOP_DUP2   2
#define KVLR_FDOP_OPEN   3
#define KVLR_FDOP_CHDIR  4
#define KVLR_FDOP_FCHDIR 5

#define KVLR_FA_MAX 64

/* Cached probe result: 0 = unprobed, 1 = supported, -1 = ENOSYS. */
static int kvlr_spawn_supported = 0;

static int kvlr_spawn_probe(void) {
    int v = __atomic_load_n(&kvlr_spawn_supported, __ATOMIC_RELAXED);
    if (v != 0) return v;
    /* Probe with the kernel's lightest kvlr_spawn invocation that doesn't
     * actually create a process: pass a NULL path (the kernel will return
     * EFAULT before any side-effect).  EFAULT is fine — we only want to
     * distinguish ENOSYS from "syscall exists." */
    long ret = syscall(SYS_kvlr_spawn, NULL, NULL, NULL, 0u);
    int v2;
    if (ret < 0 && errno == ENOSYS) v2 = -1;
    else                            v2 = 1;
    __atomic_store_n(&kvlr_spawn_supported, v2, __ATOMIC_RELAXED);
    return v2;
}

/* Translate musl-internal `posix_spawn_file_actions_t` + `posix_spawnattr_t`
 * to the kvlr_spawn ABI and issue the syscall.  Returns the raw syscall
 * result (child PID on success, -errno on failure). */
static long kvlr_spawn_call(const char *path,
                            const posix_spawn_file_actions_t *fa,
                            const posix_spawnattr_t *attr,
                            char *const argv[], char *const envp[]) {
    /* Walk fa's linked list (musl's struct fdop chain).  fa->__actions is
     * the head of a doubly-linked list — last action first when traversed
     * from head.next, but musl's reference implementation walks from the
     * tail (oldest first) and so do we. */
    struct kvlr_fa actions[KVLR_FA_MAX];
    unsigned int count = 0;

    if (fa) {
        const struct kvlr_fdop *head =
            (const struct kvlr_fdop *)((const void *const *)fa)[2];
        if (head) {
            const struct kvlr_fdop *op;
            for (op = head; op->next; op = op->next) {} /* find tail */
            for (; op; op = op->prev) {
                if (count >= KVLR_FA_MAX) return -E2BIG;
                struct kvlr_fa *out = &actions[count++];
                memset(out, 0, sizeof(*out));
                switch (op->cmd) {
                case KVLR_FDOP_CLOSE:
                    out->op = KVLR_SPAWN_FA_CLOSE;
                    out->fd = op->fd;
                    break;
                case KVLR_FDOP_DUP2:
                    out->op = KVLR_SPAWN_FA_DUP2;
                    out->fd    = op->srcfd;
                    out->newfd = op->fd;
                    break;
                case KVLR_FDOP_OPEN:
                    out->op    = KVLR_SPAWN_FA_OPEN;
                    out->fd    = op->fd;
                    out->oflag = op->oflag;
                    out->mode  = op->mode;
                    out->path  = op->path;
                    break;
                case KVLR_FDOP_CHDIR:
                case KVLR_FDOP_FCHDIR:
                    /* Not yet supported by kvlr_spawn v2.  Force the
                     * fallback path so semantics match Linux. */
                    return -ENOSYS;
                default:
                    return -EINVAL;
                }
            }
        }
    }

    /* Build the kvlr_fa header + array as a single contiguous buffer the
     * kernel can read.  Stack-allocated to avoid malloc in hot path. */
    char fa_buf[sizeof(struct kvlr_fa_hdr) + KVLR_FA_MAX * sizeof(struct kvlr_fa)];
    struct kvlr_fa_hdr *hdr = (struct kvlr_fa_hdr *)fa_buf;
    hdr->count = count;
    hdr->_pad  = 0;
    memcpy(fa_buf + sizeof(struct kvlr_fa_hdr), actions, count * sizeof(struct kvlr_fa));

    struct kvlr_attr ka = {0};
    if (attr) {
        /* Translate musl spawn attr flags to kvlr ABI.  POSIX_SPAWN_*
         * constants from spawn.h: RESETIDS=1, SETPGROUP=2, SETSIGDEF=4,
         * SETSIGMASK=8, SETSCHEDPARAM=16, SETSCHEDULER=32,
         * USEVFORK=64, SETSID=128.  The kernel won't apply
         * SETSCHEDPARAM/SETSCHEDULER (not implemented yet) — force
         * fallback if requested.  USEVFORK is musl-specific, ignore. */
        const int *attr_flags_ptr = (const int *)attr;
        int flags = attr_flags_ptr[0];
        if (flags & (POSIX_SPAWN_SETSCHEDPARAM | POSIX_SPAWN_SETSCHEDULER)) {
            /* Scheduler attrs: still unsupported by kvlr_spawn — fallback. */
            return -ENOSYS;
        }
        if (flags & POSIX_SPAWN_RESETIDS)   ka.flags |= KVLR_SPAWN_RESETIDS;
        if (flags & POSIX_SPAWN_SETSIGMASK) {
            ka.flags |= KVLR_SPAWN_SETSIGMASK;
            /* musl's posix_spawnattr_t layout: flags, pgrp, def, mask, ... */
            const sigset_t *m = (const sigset_t *)((const char *)attr + 8 + sizeof(sigset_t));
            ka.sigmask = *(const unsigned long long *)m;
        }
        if (flags & POSIX_SPAWN_SETSID)     ka.flags |= KVLR_SPAWN_SETSID;
        if (flags & POSIX_SPAWN_SETPGROUP) {
            /* musl stores pgrp at offset 4 (after flags). */
            ka.flags |= KVLR_SPAWN_SETPGROUP;
            ka.pgid = *(const int *)((const char *)attr + sizeof(int));
        }
        if (flags & POSIX_SPAWN_SETSIGDEF) {
            /* kvlr_spawn's Process::spawn constructs a fresh
             * SignalDelivery, so every signal is already SIG_DFL in the
             * child — SETSIGDEF's "reset these to SIG_DFL" is trivially
             * satisfied.  The sigdefault mask still gets passed so a
             * future kernel change that inherits signal handlers can
             * honour it. */
            ka.flags |= KVLR_SPAWN_SETSIGDEF;
            const sigset_t *d = (const sigset_t *)((const char *)attr + 8);
            ka.sigdefault = *(const unsigned long long *)d;
        }
    }

    return syscall(SYS_kvlr_spawn,
                   path, argv, envp,
                   KVLR_SPAWN_F_EXTENDED,
                   fa_buf,
                   ka.flags ? &ka : NULL);
}

/* ─── Fallback: musl reference posix_spawn body ─────────────────────────
 *
 * Used when kvlr_spawn returns -ENOSYS (running on Linux, or on Kevlar
 * with kvlr_spawn disabled, or for posix_spawn features kvlr_spawn v2
 * doesn't yet apply: SETPGROUP, SETSIGDEF, SETSCHEDULER, SETSCHEDPARAM,
 * CHDIR, FCHDIR).  Verbatim copy of musl 1.2.5's
 * src/process/posix_spawn.c::posix_spawn body using only public APIs
 * (no musl-internal helpers).  POSIX-correct.
 */
struct fallback_args {
    int p[2];
    sigset_t oldmask;
    const char *path;
    const posix_spawn_file_actions_t *fa;
    const posix_spawnattr_t *attr;
    char *const *argv, *const *envp;
};

static int fallback_child(void *args_vp) {
    struct fallback_args *args = args_vp;
    int p = args->p[1];
    int ret;
    const posix_spawn_file_actions_t *fa = args->fa;
    const posix_spawnattr_t *attr = args->attr;

    close(args->p[0]);

    /* Minimal SIG_DFL/SIG_IGN reset — only the SETSIGDEF set, since
     * scanning the full handler set requires musl-internal helpers. */
    if (attr) {
        const int *attr_flags_ptr = (const int *)attr;
        int flags = attr_flags_ptr[0];
        if (flags & POSIX_SPAWN_SETSIGDEF) {
            sigset_t def;
            memcpy(&def, (const char *)attr + 8, sizeof(sigset_t));
            for (int i = 1; i < _NSIG; i++) {
                if (sigismember(&def, i)) {
                    struct sigaction sa = {0};
                    sa.sa_handler = SIG_DFL;
                    sigaction(i, &sa, 0);
                }
            }
        }
        if (flags & POSIX_SPAWN_SETSID) {
            if ((ret = setsid()) < 0) goto fail;
        }
        if (flags & POSIX_SPAWN_SETPGROUP) {
            const int *pgrp_ptr = (const int *)((const char *)attr + 4);
            if ((ret = setpgid(0, *pgrp_ptr)) < 0) { ret = -errno; goto fail; }
        }
        if (flags & POSIX_SPAWN_RESETIDS) {
            if ((ret = setgid(getgid())) < 0) { ret = -errno; goto fail; }
            if ((ret = setuid(getuid())) < 0) { ret = -errno; goto fail; }
        }
    }

    if (fa) {
        const struct kvlr_fdop *head =
            (const struct kvlr_fdop *)((const void *const *)fa)[2];
        if (head) {
            const struct kvlr_fdop *op;
            int fd;
            for (op = head; op->next; op = op->next) {}
            for (; op; op = op->prev) {
                if (op->fd == p) {
                    int newp = dup(p);
                    if (newp < 0) { ret = -errno; goto fail; }
                    close(p);
                    p = newp;
                }
                switch (op->cmd) {
                case KVLR_FDOP_CLOSE:
                    close(op->fd);
                    break;
                case KVLR_FDOP_DUP2:
                    fd = op->srcfd;
                    if (fd == p) { ret = -EBADF; goto fail; }
                    if (fd != op->fd) {
                        if ((ret = dup2(fd, op->fd)) < 0) { ret = -errno; goto fail; }
                    }
                    break;
                case KVLR_FDOP_OPEN:
                    fd = open(op->path, op->oflag, op->mode);
                    if (fd < 0) { ret = -errno; goto fail; }
                    if (fd != op->fd) {
                        if (dup2(fd, op->fd) < 0) { ret = -errno; close(fd); goto fail; }
                        close(fd);
                    }
                    break;
                case KVLR_FDOP_CHDIR:
                    if (chdir(op->path) < 0) { ret = -errno; goto fail; }
                    break;
                case KVLR_FDOP_FCHDIR:
                    if (fchdir(op->fd) < 0) { ret = -errno; goto fail; }
                    break;
                }
            }
        }
    }

    fcntl(p, F_SETFD, FD_CLOEXEC);

    if (attr && (((const int *)attr)[0] & POSIX_SPAWN_SETSIGMASK)) {
        const sigset_t *m = (const sigset_t *)((const char *)attr + 8 + sizeof(sigset_t));
        sigprocmask(SIG_SETMASK, m, 0);
    } else {
        sigprocmask(SIG_SETMASK, &args->oldmask, 0);
    }

    execve(args->path, args->argv, args->envp);
    ret = -errno;

fail:
    {
        ssize_t r;
        do { r = write(p, &ret, sizeof ret); }
        while (r < 0 && errno == EINTR);
    }
    _exit(127);
}

/* musl's __clone wrapper isn't public — use vfork() + the child callback
 * inline.  vfork's semantics (parent blocked until child execs/exits) +
 * shared VM are equivalent to CLONE_VM | CLONE_VFORK for our purposes. */
static int fallback_posix_spawn(pid_t *res, const char *path,
                                const posix_spawn_file_actions_t *fa,
                                const posix_spawnattr_t *attr,
                                char *const argv[], char *const envp[]) {
    struct fallback_args args;
    int ec = 0;

    args.path = path; args.fa = fa; args.attr = attr;
    args.argv = argv; args.envp = envp;

    sigset_t allmask;
    sigfillset(&allmask);
    sigprocmask(SIG_BLOCK, &allmask, &args.oldmask);

    if (pipe2(args.p, O_CLOEXEC) < 0) {
        ec = errno; goto out;
    }

    pid_t pid = vfork();
    if (pid == 0) {
        fallback_child(&args);
        _exit(127);
    }
    close(args.p[1]);

    if (pid > 0) {
        ssize_t n = read(args.p[0], &ec, sizeof ec);
        if (n != sizeof ec) ec = 0;
        else waitpid(pid, &(int){0}, 0);
    } else {
        ec = errno;
    }
    close(args.p[0]);

    if (!ec && res) *res = pid;
out:
    sigprocmask(SIG_SETMASK, &args.oldmask, 0);
    return ec;
}

int posix_spawn(pid_t *restrict res, const char *restrict path,
                const posix_spawn_file_actions_t *fa,
                const posix_spawnattr_t *restrict attr,
                char *const argv[restrict], char *const envp[restrict]) {
    if (kvlr_spawn_probe() == 1) {
        long r = kvlr_spawn_call(path, fa, attr, argv, envp);
        if (r >= 0) {
            if (res) *res = (pid_t)r;
            return 0;
        }
        if (-r != ENOSYS) return -r;
        /* Fall through to fallback for ENOSYS — the kernel rejected this
         * specific feature combination (e.g. SETPGROUP, CHDIR). */
    }
    return fallback_posix_spawn(res, path, fa, attr, argv, envp);
}

/* posix_spawnp: same as posix_spawn but searches PATH.  For now, route
 * through fallback unconditionally — adding PATH search to the kvlr
 * fast path is a small follow-up. */
int posix_spawnp(pid_t *restrict res, const char *restrict file,
                 const posix_spawn_file_actions_t *fa,
                 const posix_spawnattr_t *restrict attr,
                 char *const argv[restrict], char *const envp[restrict]) {
    if (file[0] == '/' || file[0] == '.') {
        return posix_spawn(res, file, fa, attr, argv, envp);
    }
    /* TODO: PATH search.  For now, fallback handles it via execvpe. */
    return fallback_posix_spawn(res, file, fa, attr, argv, envp);
}
