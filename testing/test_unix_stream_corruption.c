// Minimal reproducer for the Unix-stream-socket data corruption
// signature observed in xfce-session.log (blog 201):
//   "Blob indicates that message exceeds maximum message length (128MiB)"
//
// Shape: send a sequence of length-prefixed frames [hdr:4][len:4][payload].
// Peer reads frame-by-frame and validates. If the reader sees a non-zero
// hdr magic where it shouldn't, or a length > expected, the stream has
// desynced — which is exactly what GLib and libICE report during XFCE
// startup.
//
// Two scenarios:
//   (1) socketpair: both ends in the same process, one thread each.
//   (2) abstract-namespace bind/listen/connect: one process each
//       (closer to the XSM/D-Bus shape).
//
// Under -smp 2 on Kevlar, if the underlying ring buffer or iovec path
// desyncs, at least one frame mismatch will be reported.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <sys/un.h>
#include <unistd.h>

#define MAGIC_HDR  0xAA55AA55u
#define NUM_FRAMES 10000

struct frame {
    uint32_t magic;
    uint32_t len;
    uint32_t seq;
    uint32_t payload;
};

// Sender: send NUM_FRAMES frames with sequential seq numbers.
// Uses writev with 3 iovecs per frame (header, seq, payload) to match
// how D-Bus and libICE actually serialize messages via writev/sendmsg.
static void send_all(int fd, const char *who) {
    for (uint32_t i = 0; i < NUM_FRAMES; i++) {
        uint32_t magic = MAGIC_HDR;
        uint32_t len   = sizeof(struct frame);
        uint32_t seq   = i;
        uint32_t payload = i ^ 0xdeadbeef;
        struct iovec iov[4] = {
            { .iov_base = &magic,   .iov_len = 4 },
            { .iov_base = &len,     .iov_len = 4 },
            { .iov_base = &seq,     .iov_len = 4 },
            { .iov_base = &payload, .iov_len = 4 },
        };
        ssize_t w = writev(fd, iov, 4);
        if (w != (ssize_t)sizeof(struct frame)) {
            printf("FAIL %s writev frame %u: rc=%zd errno=%d\n",
                   who, i, w, errno);
            return;
        }
    }
    printf("OK %s sent %u frames (writev x4 iovecs)\n", who, NUM_FRAMES);
}

// Receiver: expect NUM_FRAMES frames in order, validate each field.
static int recv_all(int fd, const char *who) {
    for (uint32_t i = 0; i < NUM_FRAMES; i++) {
        struct frame f = {0};
        size_t got = 0;
        while (got < sizeof(f)) {
            ssize_t r = read(fd, (char *)&f + got, sizeof(f) - got);
            if (r <= 0) {
                printf("FAIL %s read frame %u byte %zu: rc=%zd errno=%d\n",
                       who, i, got, r, errno);
                return 1;
            }
            got += r;
        }
        if (f.magic != MAGIC_HDR) {
            printf("FAIL %s frame %u: magic=%08x (want %08x) len=%u seq=%u payload=%08x\n",
                   who, i, f.magic, MAGIC_HDR, f.len, f.seq, f.payload);
            return 1;
        }
        if (f.len != sizeof(f)) {
            printf("FAIL %s frame %u: len=%u (want %zu)\n",
                   who, i, f.len, sizeof(f));
            return 1;
        }
        if (f.seq != i) {
            printf("FAIL %s frame %u: seq=%u (want %u)\n",
                   who, i, f.seq, i);
            return 1;
        }
        if (f.payload != (i ^ 0xdeadbeef)) {
            printf("FAIL %s frame %u: payload=%08x (want %08x)\n",
                   who, i, f.payload, i ^ 0xdeadbeef);
            return 1;
        }
    }
    printf("OK %s recv %u frames all valid\n", who, NUM_FRAMES);
    return 0;
}

// --- Scenario 1: socketpair, thread-based ---

static void *thread_send(void *arg) {
    int fd = (int)(long)arg;
    send_all(fd, "thread-sender");
    return NULL;
}

static int run_socketpair(void) {
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        printf("FAIL socketpair: errno=%d\n", errno);
        return 1;
    }
    pthread_t t;
    if (pthread_create(&t, NULL, thread_send, (void *)(long)sv[0]) != 0) {
        printf("FAIL pthread_create: errno=%d\n", errno);
        return 1;
    }
    int rc = recv_all(sv[1], "thread-receiver");
    pthread_join(t, NULL);
    close(sv[0]); close(sv[1]);
    return rc;
}

// --- Scenario 2: abstract-namespace listen/connect, process-based ---

static int bind_abstract(int fd, const char *name) {
    struct sockaddr_un sa = {0};
    sa.sun_family = AF_UNIX;
    sa.sun_path[0] = 0;  // abstract namespace marker
    size_t len = strlen(name);
    if (len > sizeof(sa.sun_path) - 1) return -1;
    memcpy(sa.sun_path + 1, name, len);
    socklen_t sl = offsetof(struct sockaddr_un, sun_path) + 1 + len;
    return bind(fd, (struct sockaddr *)&sa, sl);
}

static int connect_abstract(int fd, const char *name) {
    struct sockaddr_un sa = {0};
    sa.sun_family = AF_UNIX;
    sa.sun_path[0] = 0;
    size_t len = strlen(name);
    if (len > sizeof(sa.sun_path) - 1) return -1;
    memcpy(sa.sun_path + 1, name, len);
    socklen_t sl = offsetof(struct sockaddr_un, sun_path) + 1 + len;
    return connect(fd, (struct sockaddr *)&sa, sl);
}

static int run_abstract(void) {
    const char *name = "kevlar-unix-stream-test";
    int srv = socket(AF_UNIX, SOCK_STREAM, 0);
    if (srv < 0) { printf("FAIL socket: errno=%d\n", errno); return 1; }
    if (bind_abstract(srv, name) < 0) {
        printf("FAIL bind: errno=%d\n", errno); return 1;
    }
    if (listen(srv, 1) < 0) {
        printf("FAIL listen: errno=%d\n", errno); return 1;
    }
    pid_t pid = fork();
    if (pid < 0) { printf("FAIL fork: errno=%d\n", errno); return 1; }
    if (pid == 0) {
        // Child: connect + send.
        int cl = socket(AF_UNIX, SOCK_STREAM, 0);
        if (cl < 0) _exit(10);
        if (connect_abstract(cl, name) < 0) _exit(11);
        send_all(cl, "proc-sender");
        close(cl);
        _exit(0);
    }
    // Parent: accept + recv.
    int client_fd = accept(srv, NULL, NULL);
    if (client_fd < 0) {
        printf("FAIL accept: errno=%d\n", errno);
        return 1;
    }
    int rc = recv_all(client_fd, "proc-receiver");
    close(client_fd);
    close(srv);
    int status;
    waitpid(pid, &status, 0);
    return rc;
}

// --- Scenario 3: concurrent writers on the same fd ---
//
// Two threads write to the same socket fd. Each tagged frame has a
// writer ID in the payload low byte. Receiver verifies magic + len,
// does NOT assume ordering, but counts per-writer.
//
// The real desync signature from XFCE (bad magic / huge length) will
// show up if writev is non-atomic and the two writers' iovecs
// interleave mid-frame.

#define NUM_FRAMES_PER_WRITER 20000
#define NUM_WRITERS 4

struct dual_writer_ctx {
    int fd;
    int writer_id;
};

static void *dual_writer(void *arg) {
    struct dual_writer_ctx *ctx = arg;
    for (uint32_t i = 0; i < NUM_FRAMES_PER_WRITER; i++) {
        uint32_t magic = MAGIC_HDR;
        uint32_t len   = sizeof(struct frame);
        uint32_t seq   = i;
        uint32_t payload = (i << 8) | (ctx->writer_id & 0xff);
        struct iovec iov[4] = {
            { .iov_base = &magic,   .iov_len = 4 },
            { .iov_base = &len,     .iov_len = 4 },
            { .iov_base = &seq,     .iov_len = 4 },
            { .iov_base = &payload, .iov_len = 4 },
        };
        ssize_t w = writev(ctx->fd, iov, 4);
        if (w != (ssize_t)sizeof(struct frame)) {
            printf("FAIL writer-%d writev frame %u: rc=%zd errno=%d\n",
                   ctx->writer_id, i, w, errno);
            return NULL;
        }
    }
    return NULL;
}

static int run_concurrent(void) {
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        printf("FAIL socketpair: errno=%d\n", errno);
        return 1;
    }
    pthread_t tt[NUM_WRITERS];
    struct dual_writer_ctx ctx[NUM_WRITERS];
    for (int i = 0; i < NUM_WRITERS; i++) {
        ctx[i].fd = sv[0];
        ctx[i].writer_id = 0xa0 + i;
        pthread_create(&tt[i], NULL, dual_writer, &ctx[i]);
    }

    // Receiver on sv[1]: read frame-by-frame, check magic + len.
    int bad = 0;
    int counts[NUM_WRITERS] = {0};
    int total = NUM_WRITERS * NUM_FRAMES_PER_WRITER;
    for (int i = 0; i < total; i++) {
        struct frame f = {0};
        size_t got = 0;
        while (got < sizeof(f)) {
            ssize_t r = read(sv[1], (char *)&f + got, sizeof(f) - got);
            if (r <= 0) {
                printf("FAIL concurrent read %d byte %zu: rc=%zd errno=%d\n",
                       i, got, r, errno);
                bad = 1;
                goto done;
            }
            got += r;
        }
        if (f.magic != MAGIC_HDR || f.len != sizeof(struct frame)) {
            printf("FAIL concurrent frame %d: magic=%08x len=%u seq=%u payload=%08x\n",
                   i, f.magic, f.len, f.seq, f.payload);
            bad = 1;
            break;
        }
        // Strict check: seq field upper 24 bits == iteration for its writer.
        // payload = (iteration << 8) | writer_id. If writev is atomic, these
        // are consistent within a frame. If iovecs interleaved, payload's
        // upper 24 bits wouldn't match seq.
        int wid = f.payload & 0xff;
        uint32_t iter_from_payload = f.payload >> 8;
        int idx = wid - 0xa0;
        if (idx < 0 || idx >= NUM_WRITERS) {
            printf("FAIL unknown writer id frame %d: payload=%08x\n",
                   i, f.payload);
            bad = 1;
            break;
        }
        if (f.seq != iter_from_payload) {
            printf("FAIL writer-%d mismatch frame %d: seq=%u payload_iter=%u\n",
                   idx, i, f.seq, iter_from_payload);
            bad = 1;
            break;
        }
        counts[idx]++;
    }
done:
    for (int i = 0; i < NUM_WRITERS; i++) pthread_join(tt[i], NULL);
    close(sv[0]); close(sv[1]);
    if (!bad) {
        printf("OK concurrent: counts=[");
        for (int i = 0; i < NUM_WRITERS; i++) {
            printf("%d%s", counts[i], i < NUM_WRITERS - 1 ? "," : "");
        }
        printf("] expected %d each\n", NUM_FRAMES_PER_WRITER);
        for (int i = 0; i < NUM_WRITERS; i++) {
            if (counts[i] != NUM_FRAMES_PER_WRITER) return 1;
        }
    }
    return bad;
}

int main(void) {
    printf("=== Unix stream corruption reproducer ===\n");
    fflush(stdout);

    int rc = 0;
    printf("\n-- scenario 1: socketpair + thread --\n");
    fflush(stdout);
    rc |= run_socketpair();

    printf("\n-- scenario 2: abstract bind + fork --\n");
    fflush(stdout);
    rc |= run_abstract();

    printf("\n-- scenario 3: concurrent writev on shared fd --\n");
    fflush(stdout);
    rc |= run_concurrent();

    if (rc == 0) {
        printf("\nPASS — no stream corruption observed\n");
    } else {
        printf("\nFAIL — stream corruption reproduced\n");
    }
    fflush(stdout);
    return rc;
}
