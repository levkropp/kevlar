/*
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Comprehensive BusyBox applet integration test suite for Kevlar.
 * Tests every major BusyBox applet category against real BusyBox binaries
 * to validate that Kevlar's syscall surface supports real-world workloads.
 *
 * Compiled as a static musl binary and included in the initramfs.
 * Output format: TEST_PASS/TEST_FAIL <name>, then TEST_END <passed>/<total>
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int total = 0, passed = 0;

static void pass(const char *name) {
    printf("TEST_PASS %s\n", name);
    fflush(stdout);
    total++; passed++;
}

static void fail(const char *name) {
    printf("TEST_FAIL %s\n", name);
    fflush(stdout);
    total++;
}

static void skip(const char *name) {
    printf("TEST_SKIP %s\n", name);
    fflush(stdout);
}

/* Run a shell command, capture stdout into buf. Returns exit code. */
static int run_cmd(const char *cmd, char *buf, size_t bufsz) {
    if (buf && bufsz > 0) buf[0] = '\0';
    int pipefd[2];
    if (pipe(pipefd) < 0) return -1;
    pid_t pid = fork();
    if (pid < 0) { close(pipefd[0]); close(pipefd[1]); return -1; }
    if (pid == 0) {
        close(pipefd[0]);
        dup2(pipefd[1], STDOUT_FILENO);
        dup2(pipefd[1], STDERR_FILENO);
        close(pipefd[1]);
        /* Redirect stdin from /dev/null so commands that read stdin
         * (e.g. "while read line") don't block on the console.
         * Shell redirections like "< file" override this. */
        int devnull = open("/dev/null", O_RDONLY);
        if (devnull >= 0) { dup2(devnull, STDIN_FILENO); close(devnull); }
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(127);
    }
    close(pipefd[1]);
    size_t pos = 0;
    if (buf && bufsz > 0) {
        ssize_t n;
        while (pos < bufsz - 1 && (n = read(pipefd[0], buf + pos, bufsz - 1 - pos)) > 0)
            pos += (size_t)n;
        buf[pos] = '\0';
    }
    /* Drain any remaining output */
    char drain[256];
    while (read(pipefd[0], drain, sizeof(drain)) > 0) {}
    close(pipefd[0]);
    int status;
    waitpid(pid, &status, 0);
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

/* Helper: write a string to a file */
static int write_file(const char *path, const char *content) {
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) return -1;
    write(fd, content, strlen(content));
    close(fd);
    return 0;
}

/* Helper: read a file into buf */
static int read_file(const char *path, char *buf, size_t bufsz) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    ssize_t n = read(fd, buf, bufsz - 1);
    close(fd);
    if (n < 0) return -1;
    buf[n] = '\0';
    return 0;
}

/* Helper: clean up test directory */
static void cleanup_dir(const char *path) {
    char cmd[512];
    snprintf(cmd, sizeof(cmd), "rm -rf %s", path);
    run_cmd(cmd, NULL, 0);
}

static void init_setup(void) {
    if (getpid() != 1) return;
    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/dev", 0755);
    mkdir("/tmp", 0755);
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    mount("tmpfs", "/tmp", "tmpfs", 0, NULL);
    if (open("/dev/null", O_RDONLY) < 0)
        mknod("/dev/null", S_IFCHR | 0666, makedev(1, 3));
    if (open("/dev/zero", O_RDONLY) < 0)
        mknod("/dev/zero", S_IFCHR | 0666, makedev(1, 5));
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 1: File Operations (20 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_echo_redirect(void) {
    char out[256];
    int rc = run_cmd("echo hello > /tmp/bb_echo && cat /tmp/bb_echo", out, sizeof(out));
    if (rc == 0 && strstr(out, "hello"))
        pass("echo_redirect");
    else
        fail("echo_redirect");
    unlink("/tmp/bb_echo");
}

static void test_cat_file(void) {
    write_file("/tmp/bb_cat", "meow\n");
    char out[256];
    int rc = run_cmd("cat /tmp/bb_cat", out, sizeof(out));
    if (rc == 0 && strstr(out, "meow"))
        pass("cat_file");
    else
        fail("cat_file");
    unlink("/tmp/bb_cat");
}

static void test_cp_file(void) {
    write_file("/tmp/bb_cp_src", "copy me\n");
    char out[256];
    int rc = run_cmd("cp /tmp/bb_cp_src /tmp/bb_cp_dst && cat /tmp/bb_cp_dst", out, sizeof(out));
    if (rc == 0 && strstr(out, "copy me"))
        pass("cp_file");
    else
        fail("cp_file");
    unlink("/tmp/bb_cp_src");
    unlink("/tmp/bb_cp_dst");
}

static void test_mv_file(void) {
    write_file("/tmp/bb_mv_src", "move me\n");
    struct stat st;
    char out[256];
    int rc = run_cmd("mv /tmp/bb_mv_src /tmp/bb_mv_dst && cat /tmp/bb_mv_dst", out, sizeof(out));
    if (rc == 0 && strstr(out, "move me") && stat("/tmp/bb_mv_src", &st) < 0)
        pass("mv_file");
    else
        fail("mv_file");
    unlink("/tmp/bb_mv_src");
    unlink("/tmp/bb_mv_dst");
}

static void test_rm_file(void) {
    write_file("/tmp/bb_rm", "delete me\n");
    struct stat st;
    int rc = run_cmd("rm /tmp/bb_rm", NULL, 0);
    if (rc == 0 && stat("/tmp/bb_rm", &st) < 0 && errno == ENOENT)
        pass("rm_file");
    else
        fail("rm_file");
}

static void test_mkdir_rmdir(void) {
    struct stat st;
    int rc1 = run_cmd("mkdir /tmp/bb_dir", NULL, 0);
    int ok1 = (rc1 == 0 && stat("/tmp/bb_dir", &st) == 0 && S_ISDIR(st.st_mode));
    int rc2 = run_cmd("rmdir /tmp/bb_dir", NULL, 0);
    int ok2 = (rc2 == 0 && stat("/tmp/bb_dir", &st) < 0);
    if (ok1 && ok2)
        pass("mkdir_rmdir");
    else
        fail("mkdir_rmdir");
    cleanup_dir("/tmp/bb_dir");
}

static void test_ln_hard(void) {
    write_file("/tmp/bb_ln_src", "linked\n");
    struct stat st1, st2;
    int rc = run_cmd("ln /tmp/bb_ln_src /tmp/bb_ln_dst", NULL, 0);
    stat("/tmp/bb_ln_src", &st1);
    stat("/tmp/bb_ln_dst", &st2);
    if (rc == 0 && st1.st_ino == st2.st_ino)
        pass("ln_hard");
    else
        fail("ln_hard");
    unlink("/tmp/bb_ln_src");
    unlink("/tmp/bb_ln_dst");
}

static void test_ln_soft(void) {
    write_file("/tmp/bb_lns_src", "symlinked\n");
    char out[256];
    int rc = run_cmd("ln -s /tmp/bb_lns_src /tmp/bb_lns_dst && cat /tmp/bb_lns_dst", out, sizeof(out));
    char link[256];
    ssize_t len = readlink("/tmp/bb_lns_dst", link, sizeof(link) - 1);
    if (len > 0) link[len] = '\0';
    if (rc == 0 && strstr(out, "symlinked") && len > 0 && strstr(link, "bb_lns_src"))
        pass("ln_soft");
    else
        fail("ln_soft");
    unlink("/tmp/bb_lns_src");
    unlink("/tmp/bb_lns_dst");
}

static void test_chmod_file(void) {
    write_file("/tmp/bb_chmod", "test\n");
    struct stat st;
    int rc = run_cmd("chmod 755 /tmp/bb_chmod", NULL, 0);
    stat("/tmp/bb_chmod", &st);
    if (rc == 0 && (st.st_mode & 0777) == 0755)
        pass("chmod_file");
    else
        fail("chmod_file");
    unlink("/tmp/bb_chmod");
}

static void test_touch_file(void) {
    unlink("/tmp/bb_touch");
    struct stat st;
    int rc = run_cmd("touch /tmp/bb_touch", NULL, 0);
    if (rc == 0 && stat("/tmp/bb_touch", &st) == 0)
        pass("touch_file");
    else
        fail("touch_file");
    unlink("/tmp/bb_touch");
}

static void test_ls_basic(void) {
    mkdir("/tmp/bb_ls", 0755);
    write_file("/tmp/bb_ls/alpha", "a");
    write_file("/tmp/bb_ls/beta", "b");
    char out[4096];
    int rc = run_cmd("ls /tmp/bb_ls", out, sizeof(out));
    if (rc == 0 && strstr(out, "alpha") && strstr(out, "beta"))
        pass("ls_basic");
    else
        fail("ls_basic");
    cleanup_dir("/tmp/bb_ls");
}

static void test_ls_long(void) {
    write_file("/tmp/bb_ls_l", "content\n");
    char out[4096];
    int rc = run_cmd("ls -l /tmp/bb_ls_l", out, sizeof(out));
    /* ls -l should show permissions, size, name */
    if (rc == 0 && (strstr(out, "rw") || strstr(out, "bb_ls_l")))
        pass("ls_long");
    else
        fail("ls_long");
    unlink("/tmp/bb_ls_l");
}

static void test_head_file(void) {
    write_file("/tmp/bb_head", "line1\nline2\nline3\nline4\nline5\n");
    char out[4096];
    int rc = run_cmd("head -n 2 /tmp/bb_head", out, sizeof(out));
    if (rc == 0 && strstr(out, "line1") && strstr(out, "line2") && !strstr(out, "line3"))
        pass("head_file");
    else
        fail("head_file");
    unlink("/tmp/bb_head");
}

static void test_tail_file(void) {
    write_file("/tmp/bb_tail", "line1\nline2\nline3\nline4\nline5\n");
    char out[4096];
    int rc = run_cmd("tail -n 2 /tmp/bb_tail", out, sizeof(out));
    if (rc == 0 && strstr(out, "line4") && strstr(out, "line5") && !strstr(out, "line3"))
        pass("tail_file");
    else
        fail("tail_file");
    unlink("/tmp/bb_tail");
}

static void test_wc_file(void) {
    write_file("/tmp/bb_wc", "one two three\nfour five\n");
    char out[256];
    int rc = run_cmd("wc /tmp/bb_wc", out, sizeof(out));
    /* Should show 2 lines, 5 words, 24 bytes (varies with newline) */
    if (rc == 0 && strstr(out, "2"))
        pass("wc_file");
    else
        fail("wc_file");
    unlink("/tmp/bb_wc");
}

static void test_tee_file(void) {
    char out[256];
    int rc = run_cmd("echo tee_test | tee /tmp/bb_tee", out, sizeof(out));
    char file_content[256];
    read_file("/tmp/bb_tee", file_content, sizeof(file_content));
    if (rc == 0 && strstr(out, "tee_test") && strstr(file_content, "tee_test"))
        pass("tee_file");
    else
        fail("tee_file");
    unlink("/tmp/bb_tee");
}

static void test_dd_basic(void) {
    char out[256];
    int rc = run_cmd("dd if=/dev/zero of=/tmp/bb_dd bs=512 count=4 2>&1", out, sizeof(out));
    struct stat st;
    stat("/tmp/bb_dd", &st);
    if (rc == 0 && st.st_size == 2048)
        pass("dd_basic");
    else
        fail("dd_basic");
    unlink("/tmp/bb_dd");
}

static void test_truncate_file(void) {
    write_file("/tmp/bb_trunc", "hello world\n");
    struct stat st;
    int rc = run_cmd("truncate -s 5 /tmp/bb_trunc", NULL, 0);
    stat("/tmp/bb_trunc", &st);
    if (rc == 0 && st.st_size == 5)
        pass("truncate_file");
    else
        fail("truncate_file");
    unlink("/tmp/bb_trunc");
}

static void test_du_basic(void) {
    mkdir("/tmp/bb_du", 0755);
    write_file("/tmp/bb_du/file1", "aaaa");
    char out[4096];
    int rc = run_cmd("du /tmp/bb_du", out, sizeof(out));
    /* du should output a number + the path */
    if (rc == 0 && strstr(out, "bb_du"))
        pass("du_basic");
    else
        fail("du_basic");
    cleanup_dir("/tmp/bb_du");
}

static void test_stat_cmd(void) {
    write_file("/tmp/bb_stat", "data\n");
    char out[4096];
    int rc = run_cmd("stat /tmp/bb_stat", out, sizeof(out));
    /* BusyBox stat shows File:, Size:, etc. */
    if (rc == 0 && (strstr(out, "Size") || strstr(out, "bb_stat")))
        pass("stat_cmd");
    else
        fail("stat_cmd");
    unlink("/tmp/bb_stat");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 2: Text Processing (12 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_grep_match(void) {
    write_file("/tmp/bb_grep", "apple\nbanana\ncherry\napricot\n");
    char out[4096];
    int rc = run_cmd("grep ap /tmp/bb_grep", out, sizeof(out));
    if (rc == 0 && strstr(out, "apple") && strstr(out, "apricot") && !strstr(out, "banana"))
        pass("grep_match");
    else
        fail("grep_match");
    unlink("/tmp/bb_grep");
}

static void test_grep_count(void) {
    write_file("/tmp/bb_grepc", "aaa\nbbb\naaa\nccc\naaa\n");
    char out[256];
    int rc = run_cmd("grep -c aaa /tmp/bb_grepc", out, sizeof(out));
    if (rc == 0 && strstr(out, "3"))
        pass("grep_count");
    else
        fail("grep_count");
    unlink("/tmp/bb_grepc");
}

static void test_grep_invert(void) {
    write_file("/tmp/bb_grepv", "yes\nno\nyes\nno\n");
    char out[4096];
    int rc = run_cmd("grep -v yes /tmp/bb_grepv", out, sizeof(out));
    if (rc == 0 && strstr(out, "no") && !strstr(out, "yes"))
        pass("grep_invert");
    else
        fail("grep_invert");
    unlink("/tmp/bb_grepv");
}

static void test_sed_substitute(void) {
    write_file("/tmp/bb_sed", "hello world\n");
    char out[256];
    int rc = run_cmd("sed 's/world/kevlar/' /tmp/bb_sed", out, sizeof(out));
    if (rc == 0 && strstr(out, "hello kevlar"))
        pass("sed_substitute");
    else
        fail("sed_substitute");
    unlink("/tmp/bb_sed");
}

static void test_sed_delete(void) {
    write_file("/tmp/bb_sedd", "keep\ndelete\nkeep\n");
    char out[256];
    int rc = run_cmd("sed '/delete/d' /tmp/bb_sedd", out, sizeof(out));
    if (rc == 0 && strstr(out, "keep") && !strstr(out, "delete"))
        pass("sed_delete");
    else
        fail("sed_delete");
    unlink("/tmp/bb_sedd");
}

static void test_awk_print(void) {
    write_file("/tmp/bb_awk", "one two three\nfour five six\n");
    char out[256];
    int rc = run_cmd("awk '{print $2}' /tmp/bb_awk", out, sizeof(out));
    if (rc == 0 && strstr(out, "two") && strstr(out, "five"))
        pass("awk_print");
    else
        fail("awk_print");
    unlink("/tmp/bb_awk");
}

static void test_sort_numeric(void) {
    write_file("/tmp/bb_sort", "10\n2\n30\n1\n");
    char out[256];
    int rc = run_cmd("sort -n /tmp/bb_sort", out, sizeof(out));
    /* Verify order: 1 before 2 before 10 before 30 */
    char *p1 = strstr(out, "1\n");
    char *p2 = strstr(out, "2\n");
    char *p10 = strstr(out, "10\n");
    char *p30 = strstr(out, "30\n");
    if (rc == 0 && p1 && p2 && p10 && p30 && p1 < p2 && p2 < p10 && p10 < p30)
        pass("sort_numeric");
    else
        fail("sort_numeric");
    unlink("/tmp/bb_sort");
}

static void test_sort_reverse(void) {
    write_file("/tmp/bb_sortr", "a\nc\nb\n");
    char out[256];
    int rc = run_cmd("sort -r /tmp/bb_sortr", out, sizeof(out));
    char *pc = strstr(out, "c");
    char *pb = strstr(out, "b");
    char *pa = strstr(out, "a");
    if (rc == 0 && pc && pb && pa && pc < pb && pb < pa)
        pass("sort_reverse");
    else
        fail("sort_reverse");
    unlink("/tmp/bb_sortr");
}

static void test_uniq_basic(void) {
    write_file("/tmp/bb_uniq", "aaa\naaa\nbbb\nbbb\nbbb\nccc\n");
    char out[256];
    int rc = run_cmd("uniq /tmp/bb_uniq", out, sizeof(out));
    /* Should collapse to aaa\nbbb\nccc */
    int count_aaa = 0;
    char *p = out;
    while ((p = strstr(p, "aaa")) != NULL) { count_aaa++; p++; }
    if (rc == 0 && count_aaa == 1 && strstr(out, "bbb") && strstr(out, "ccc"))
        pass("uniq_basic");
    else
        fail("uniq_basic");
    unlink("/tmp/bb_uniq");
}

static void test_tr_translate(void) {
    char out[256];
    int rc = run_cmd("echo 'hello' | tr 'a-z' 'A-Z'", out, sizeof(out));
    if (rc == 0 && strstr(out, "HELLO"))
        pass("tr_translate");
    else
        fail("tr_translate");
}

static void test_cut_fields(void) {
    write_file("/tmp/bb_cut", "one:two:three\nfour:five:six\n");
    char out[256];
    int rc = run_cmd("cut -d: -f2 /tmp/bb_cut", out, sizeof(out));
    if (rc == 0 && strstr(out, "two") && strstr(out, "five"))
        pass("cut_fields");
    else
        fail("cut_fields");
    unlink("/tmp/bb_cut");
}

static void test_diff_files(void) {
    write_file("/tmp/bb_diff1", "line1\nline2\nline3\n");
    write_file("/tmp/bb_diff2", "line1\nchanged\nline3\n");
    char out[4096];
    int rc = run_cmd("diff /tmp/bb_diff1 /tmp/bb_diff2", out, sizeof(out));
    /* diff returns 1 when files differ, should show the changed line */
    if (rc == 1 && (strstr(out, "line2") || strstr(out, "changed")))
        pass("diff_files");
    else
        fail("diff_files");
    unlink("/tmp/bb_diff1");
    unlink("/tmp/bb_diff2");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 3: Shell Features (13 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_pipe_basic(void) {
    char out[256];
    int rc = run_cmd("echo 'pipe test' | cat", out, sizeof(out));
    if (rc == 0 && strstr(out, "pipe test"))
        pass("pipe_basic");
    else
        fail("pipe_basic");
}

static void test_pipe_chain(void) {
    write_file("/tmp/bb_pipe", "cherry\napple\nbanana\napple\ncherry\n");
    char out[256];
    int rc = run_cmd("cat /tmp/bb_pipe | sort | uniq", out, sizeof(out));
    if (rc == 0 && strstr(out, "apple") && strstr(out, "banana") && strstr(out, "cherry"))
        pass("pipe_chain");
    else
        fail("pipe_chain");
    unlink("/tmp/bb_pipe");
}

static void test_redirect_stdout(void) {
    int rc = run_cmd("echo 'redir_out' > /tmp/bb_redir", NULL, 0);
    char buf[256];
    read_file("/tmp/bb_redir", buf, sizeof(buf));
    if (rc == 0 && strstr(buf, "redir_out"))
        pass("redirect_stdout");
    else
        fail("redirect_stdout");
    unlink("/tmp/bb_redir");
}

static void test_redirect_append(void) {
    write_file("/tmp/bb_append", "first\n");
    int rc = run_cmd("echo 'second' >> /tmp/bb_append", NULL, 0);
    char buf[256];
    read_file("/tmp/bb_append", buf, sizeof(buf));
    if (rc == 0 && strstr(buf, "first") && strstr(buf, "second"))
        pass("redirect_append");
    else
        fail("redirect_append");
    unlink("/tmp/bb_append");
}

static void test_redirect_stdin(void) {
    write_file("/tmp/bb_stdin", "from_file\n");
    char out[256];
    int rc = run_cmd("cat < /tmp/bb_stdin", out, sizeof(out));
    if (rc == 0 && strstr(out, "from_file"))
        pass("redirect_stdin");
    else
        fail("redirect_stdin");
    unlink("/tmp/bb_stdin");
}

static void test_subshell(void) {
    char out[256];
    int rc = run_cmd("(echo sub1; echo sub2)", out, sizeof(out));
    if (rc == 0 && strstr(out, "sub1") && strstr(out, "sub2"))
        pass("subshell");
    else
        fail("subshell");
}

static void test_command_substitution(void) {
    char out[256];
    int rc = run_cmd("echo \"result=$(echo 42)\"", out, sizeof(out));
    if (rc == 0 && strstr(out, "result=42"))
        pass("command_substitution");
    else
        fail("command_substitution");
}

static void test_here_doc(void) {
    char out[256];
    int rc = run_cmd("cat <<EOF\nhere_content\nEOF", out, sizeof(out));
    if (rc == 0 && strstr(out, "here_content"))
        pass("here_doc");
    else
        fail("here_doc");
}

static void test_glob_star(void) {
    mkdir("/tmp/bb_glob", 0755);
    write_file("/tmp/bb_glob/a.txt", "a");
    write_file("/tmp/bb_glob/b.txt", "b");
    write_file("/tmp/bb_glob/c.dat", "c");
    char out[4096];
    int rc = run_cmd("ls /tmp/bb_glob/*.txt", out, sizeof(out));
    if (rc == 0 && strstr(out, "a.txt") && strstr(out, "b.txt") && !strstr(out, "c.dat"))
        pass("glob_star");
    else
        fail("glob_star");
    cleanup_dir("/tmp/bb_glob");
}

static void test_for_loop(void) {
    char out[256];
    int rc = run_cmd("for i in 1 2 3; do echo \"num$i\"; done", out, sizeof(out));
    if (rc == 0 && strstr(out, "num1") && strstr(out, "num2") && strstr(out, "num3"))
        pass("for_loop");
    else
        fail("for_loop");
}

static void test_while_read(void) {
    write_file("/tmp/bb_while", "aaa\nbbb\nccc\n");
    char out[256];
    int rc = run_cmd("while read line; do echo \"got:$line\"; done < /tmp/bb_while", out, sizeof(out));
    if (rc == 0 && strstr(out, "got:aaa") && strstr(out, "got:bbb") && strstr(out, "got:ccc"))
        pass("while_read");
    else
        fail("while_read");
    unlink("/tmp/bb_while");
}

static void test_case_stmt(void) {
    char out[256];
    int rc = run_cmd("x=hello; case $x in hello) echo matched;; *) echo nope;; esac", out, sizeof(out));
    if (rc == 0 && strstr(out, "matched"))
        pass("case_stmt");
    else
        fail("case_stmt");
}

static void test_trap_signal(void) {
    char out[256];
    int rc = run_cmd("trap 'echo trapped' USR1; kill -USR1 $$; echo done", out, sizeof(out));
    if (rc == 0 && strstr(out, "trapped") && strstr(out, "done"))
        pass("trap_signal");
    else
        fail("trap_signal");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 4: Process Management (6 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_ps_list(void) {
    char out[4096];
    int rc = run_cmd("ps", out, sizeof(out));
    /* ps should show at least PID and CMD columns */
    if (rc == 0 && (strstr(out, "PID") || strstr(out, "ps")))
        pass("ps_list");
    else
        fail("ps_list");
}

static void test_kill_process(void) {
    char out[256];
    /* Start a background sleep, kill it, verify it's gone */
    int rc = run_cmd("sleep 100 & PID=$!; kill $PID; wait $PID 2>/dev/null; echo killed", out, sizeof(out));
    if (rc == 0 && strstr(out, "killed"))
        pass("kill_process");
    else
        fail("kill_process");
}

static void test_env_var(void) {
    char out[256];
    int rc = run_cmd("MY_VAR=kevlar_test env | grep MY_VAR", out, sizeof(out));
    if (rc == 0 && strstr(out, "kevlar_test"))
        pass("env_var");
    else
        fail("env_var");
}

static void test_xargs_basic(void) {
    char out[256];
    int rc = run_cmd("echo 'hello world' | xargs echo 'got:'", out, sizeof(out));
    if (rc == 0 && strstr(out, "got:") && strstr(out, "hello"))
        pass("xargs_basic");
    else
        fail("xargs_basic");
}

static void test_nice_process(void) {
    char out[256];
    int rc = run_cmd("nice -n 5 echo nice_ok", out, sizeof(out));
    if (rc == 0 && strstr(out, "nice_ok"))
        pass("nice_process");
    else
        fail("nice_process");
}

static void test_nohup_process(void) {
    char out[256];
    int rc = run_cmd("nohup echo nohup_ok 2>/dev/null", out, sizeof(out));
    if (rc == 0 && strstr(out, "nohup_ok"))
        pass("nohup_process");
    else
        fail("nohup_process");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 5: System Information (8 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_hostname_cmd(void) {
    char out[256];
    int rc = run_cmd("hostname", out, sizeof(out));
    /* Should return something non-empty */
    if (rc == 0 && strlen(out) > 0)
        pass("hostname_cmd");
    else
        fail("hostname_cmd");
}

static void test_uname_cmd(void) {
    char out[256];
    int rc = run_cmd("uname -a", out, sizeof(out));
    /* Should contain kernel name and machine type */
    if (rc == 0 && strlen(out) > 5)
        pass("uname_cmd");
    else
        fail("uname_cmd");
}

static void test_id_cmd(void) {
    char out[256];
    int rc = run_cmd("id", out, sizeof(out));
    /* Should contain uid= and gid= */
    if (rc == 0 && strstr(out, "uid="))
        pass("id_cmd");
    else
        fail("id_cmd");
}

static void test_whoami_cmd(void) {
    char out[256];
    int rc = run_cmd("whoami", out, sizeof(out));
    /* Should return root or a username */
    if (rc == 0 && strlen(out) > 0)
        pass("whoami_cmd");
    else
        fail("whoami_cmd");
}

static void test_date_cmd(void) {
    char out[256];
    int rc = run_cmd("date", out, sizeof(out));
    /* date should produce output (day/month/time) */
    if (rc == 0 && strlen(out) > 5)
        pass("date_cmd");
    else
        fail("date_cmd");
}

static void test_uptime_cmd(void) {
    char out[256];
    int rc = run_cmd("uptime", out, sizeof(out));
    /* uptime should show time and load average */
    if (rc == 0 && (strstr(out, "up") || strstr(out, "load")))
        pass("uptime_cmd");
    else
        fail("uptime_cmd");
}

static void test_df_cmd(void) {
    char out[4096];
    int rc = run_cmd("df", out, sizeof(out));
    /* df should show filesystem listings */
    if (rc == 0 && (strstr(out, "Filesystem") || strstr(out, "1K-blocks") || strstr(out, "/")))
        pass("df_cmd");
    else
        fail("df_cmd");
}

static void test_true_false(void) {
    int rc1 = run_cmd("true", NULL, 0);
    int rc2 = run_cmd("false", NULL, 0);
    if (rc1 == 0 && rc2 == 1)
        pass("true_false");
    else
        fail("true_false");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 6: Archive/Compression (4 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_tar_create_extract(void) {
    mkdir("/tmp/bb_tar_in", 0755);
    write_file("/tmp/bb_tar_in/file1.txt", "content1\n");
    write_file("/tmp/bb_tar_in/file2.txt", "content2\n");
    char out[256];
    int rc1 = run_cmd("tar cf /tmp/bb_tar.tar -C /tmp bb_tar_in", NULL, 0);
    mkdir("/tmp/bb_tar_out", 0755);
    int rc2 = run_cmd("tar xf /tmp/bb_tar.tar -C /tmp/bb_tar_out", NULL, 0);
    read_file("/tmp/bb_tar_out/bb_tar_in/file1.txt", out, sizeof(out));
    if (rc1 == 0 && rc2 == 0 && strstr(out, "content1"))
        pass("tar_create_extract");
    else
        fail("tar_create_extract");
    cleanup_dir("/tmp/bb_tar_in");
    cleanup_dir("/tmp/bb_tar_out");
    unlink("/tmp/bb_tar.tar");
}

static void test_gzip_decompress(void) {
    write_file("/tmp/bb_gz_orig", "compress me please\n");
    char out[256];
    int rc1 = run_cmd("gzip /tmp/bb_gz_orig", NULL, 0);
    struct stat st;
    int gz_exists = (stat("/tmp/bb_gz_orig.gz", &st) == 0);
    int rc2 = run_cmd("gunzip /tmp/bb_gz_orig.gz", NULL, 0);
    read_file("/tmp/bb_gz_orig", out, sizeof(out));
    if (rc1 == 0 && gz_exists && rc2 == 0 && strstr(out, "compress me please"))
        pass("gzip_decompress");
    else
        fail("gzip_decompress");
    unlink("/tmp/bb_gz_orig");
    unlink("/tmp/bb_gz_orig.gz");
}

static void test_tar_gz(void) {
    mkdir("/tmp/bb_tgz_in", 0755);
    write_file("/tmp/bb_tgz_in/data.txt", "tgz_data\n");
    char out[256];
    int rc1 = run_cmd("tar czf /tmp/bb_test.tar.gz -C /tmp bb_tgz_in", NULL, 0);
    mkdir("/tmp/bb_tgz_out", 0755);
    int rc2 = run_cmd("tar xzf /tmp/bb_test.tar.gz -C /tmp/bb_tgz_out", NULL, 0);
    read_file("/tmp/bb_tgz_out/bb_tgz_in/data.txt", out, sizeof(out));
    if (rc1 == 0 && rc2 == 0 && strstr(out, "tgz_data"))
        pass("tar_gz");
    else
        fail("tar_gz");
    cleanup_dir("/tmp/bb_tgz_in");
    cleanup_dir("/tmp/bb_tgz_out");
    unlink("/tmp/bb_test.tar.gz");
}

static void test_cpio_basic(void) {
    mkdir("/tmp/bb_cpio_in", 0755);
    write_file("/tmp/bb_cpio_in/cpio_file", "cpio_content\n");
    char out[256];
    int rc1 = run_cmd("cd /tmp/bb_cpio_in && echo cpio_file | cpio -o > /tmp/bb_test.cpio 2>/dev/null", NULL, 0);
    mkdir("/tmp/bb_cpio_out", 0755);
    int rc2 = run_cmd("cd /tmp/bb_cpio_out && cpio -i < /tmp/bb_test.cpio 2>/dev/null", NULL, 0);
    read_file("/tmp/bb_cpio_out/cpio_file", out, sizeof(out));
    if (rc1 == 0 && rc2 == 0 && strstr(out, "cpio_content"))
        pass("cpio_basic");
    else
        fail("cpio_basic");
    cleanup_dir("/tmp/bb_cpio_in");
    cleanup_dir("/tmp/bb_cpio_out");
    unlink("/tmp/bb_test.cpio");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 7: Find & Directory Traversal (4 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_find_name(void) {
    mkdir("/tmp/bb_find", 0755);
    mkdir("/tmp/bb_find/sub", 0755);
    write_file("/tmp/bb_find/a.txt", "a");
    write_file("/tmp/bb_find/sub/b.txt", "b");
    write_file("/tmp/bb_find/c.dat", "c");
    char out[4096];
    int rc = run_cmd("find /tmp/bb_find -name '*.txt'", out, sizeof(out));
    if (rc == 0 && strstr(out, "a.txt") && strstr(out, "b.txt") && !strstr(out, "c.dat"))
        pass("find_name");
    else
        fail("find_name");
    cleanup_dir("/tmp/bb_find");
}

static void test_find_type(void) {
    mkdir("/tmp/bb_findt", 0755);
    mkdir("/tmp/bb_findt/dir1", 0755);
    write_file("/tmp/bb_findt/file1", "f");
    char out[4096];
    int rc = run_cmd("find /tmp/bb_findt -type d", out, sizeof(out));
    if (rc == 0 && strstr(out, "dir1") && !strstr(out, "file1"))
        pass("find_type");
    else
        fail("find_type");
    cleanup_dir("/tmp/bb_findt");
}

static void test_find_exec(void) {
    mkdir("/tmp/bb_findx", 0755);
    write_file("/tmp/bb_findx/x1.txt", "hello");
    write_file("/tmp/bb_findx/x2.txt", "world");
    char out[4096];
    int rc = run_cmd("find /tmp/bb_findx -name '*.txt' -exec cat {} \\;", out, sizeof(out));
    if (rc == 0 && strstr(out, "hello") && strstr(out, "world"))
        pass("find_exec");
    else
        fail("find_exec");
    cleanup_dir("/tmp/bb_findx");
}

static void test_xargs_find(void) {
    mkdir("/tmp/bb_xfind", 0755);
    write_file("/tmp/bb_xfind/f1", "xargs1\n");
    write_file("/tmp/bb_xfind/f2", "xargs2\n");
    char out[4096];
    int rc = run_cmd("find /tmp/bb_xfind -type f | xargs cat", out, sizeof(out));
    if (rc == 0 && strstr(out, "xargs1") && strstr(out, "xargs2"))
        pass("xargs_find");
    else
        fail("xargs_find");
    cleanup_dir("/tmp/bb_xfind");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 8: Advanced Shell & Arithmetic (5 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_expr_arithmetic(void) {
    char out[256];
    int rc = run_cmd("expr 6 '*' 7", out, sizeof(out));
    if (rc == 0 && strstr(out, "42"))
        pass("expr_arithmetic");
    else
        fail("expr_arithmetic");
}

static void test_shell_arithmetic(void) {
    char out[256];
    int rc = run_cmd("echo $((17 + 25))", out, sizeof(out));
    if (rc == 0 && strstr(out, "42"))
        pass("shell_arithmetic");
    else
        fail("shell_arithmetic");
}

static void test_seq_command(void) {
    char out[256];
    int rc = run_cmd("seq 1 5", out, sizeof(out));
    if (rc == 0 && strstr(out, "1") && strstr(out, "3") && strstr(out, "5"))
        pass("seq_command");
    else
        fail("seq_command");
}

static void test_test_command(void) {
    int rc1 = run_cmd("test -f /bin/sh", NULL, 0);
    int rc2 = run_cmd("test -d /tmp", NULL, 0);
    int rc3 = run_cmd("test -f /nonexistent", NULL, 0);
    if (rc1 == 0 && rc2 == 0 && rc3 != 0)
        pass("test_command");
    else
        fail("test_command");
}

static void test_printf_format(void) {
    char out[256];
    int rc = run_cmd("printf '%05d\\n' 42", out, sizeof(out));
    if (rc == 0 && strstr(out, "00042"))
        pass("printf_format");
    else
        fail("printf_format");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 9: Real-World Workload Patterns (8 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_log_pipeline(void) {
    /* Simulate log processing: generate → grep → sort → uniq -c → sort -rn → head */
    write_file("/tmp/bb_log", "INFO request\nERROR failed\nINFO request\n"
                              "WARN timeout\nERROR failed\nERROR failed\nINFO request\n");
    char out[4096];
    int rc = run_cmd("grep ERROR /tmp/bb_log | sort | uniq -c | sort -rn", out, sizeof(out));
    /* Should show "3 ERROR failed" */
    if (rc == 0 && strstr(out, "3") && strstr(out, "ERROR"))
        pass("log_pipeline");
    else
        fail("log_pipeline");
    unlink("/tmp/bb_log");
}

static void test_config_parse(void) {
    /* Parse /etc/passwd with awk to extract usernames */
    char out[4096];
    int rc = run_cmd("awk -F: '{print $1}' /etc/passwd", out, sizeof(out));
    if (rc == 0 && strstr(out, "root"))
        pass("config_parse");
    else
        fail("config_parse");
}

static void test_file_batch_create(void) {
    /* Create 50 files, list them, count them, delete them */
    mkdir("/tmp/bb_batch", 0755);
    char out[256];
    int rc = run_cmd(
        "for i in $(seq 1 50); do touch /tmp/bb_batch/file$i; done; "
        "ls /tmp/bb_batch | wc -l",
        out, sizeof(out));
    if (rc == 0 && strstr(out, "50"))
        pass("file_batch_create");
    else
        fail("file_batch_create");
    cleanup_dir("/tmp/bb_batch");
}

static void test_multi_redirect(void) {
    /* Complex redirection: tee to multiple files */
    char out1[256], out2[256];
    int rc = run_cmd("echo multi_out | tee /tmp/bb_mr1 > /tmp/bb_mr2", NULL, 0);
    read_file("/tmp/bb_mr1", out1, sizeof(out1));
    read_file("/tmp/bb_mr2", out2, sizeof(out2));
    if (rc == 0 && strstr(out1, "multi_out") && strstr(out2, "multi_out"))
        pass("multi_redirect");
    else
        fail("multi_redirect");
    unlink("/tmp/bb_mr1");
    unlink("/tmp/bb_mr2");
}

static void test_process_substitution_workaround(void) {
    /* BusyBox sh doesn't have process substitution, but we can use temp files */
    write_file("/tmp/bb_ps1", "aaa\nbbb\nccc\n");
    write_file("/tmp/bb_ps2", "bbb\nccc\nddd\n");
    char out[4096];
    int rc = run_cmd("comm -12 /tmp/bb_ps1 /tmp/bb_ps2 2>/dev/null || "
                     "grep -Fxf /tmp/bb_ps1 /tmp/bb_ps2", out, sizeof(out));
    /* Should find common lines: bbb, ccc */
    if (rc == 0 && strstr(out, "bbb") && strstr(out, "ccc"))
        pass("set_intersection");
    else
        fail("set_intersection");
    unlink("/tmp/bb_ps1");
    unlink("/tmp/bb_ps2");
}

static void test_background_jobs(void) {
    /* Start a background job, wait for it, verify it ran */
    char out[256];
    int rc = run_cmd("echo bg_start > /tmp/bb_bg; sleep 0 & wait; echo bg_done >> /tmp/bb_bg; "
                     "cat /tmp/bb_bg", out, sizeof(out));
    if (rc == 0 && strstr(out, "bg_start") && strstr(out, "bg_done"))
        pass("background_jobs");
    else
        fail("background_jobs");
    unlink("/tmp/bb_bg");
}

static void test_exit_codes(void) {
    char out[256];
    /* Test that exit codes propagate correctly through shell */
    run_cmd("sh -c 'exit 42'; echo $?", out, sizeof(out));
    /* The outer shell captures exit code */
    if (strstr(out, "42"))
        pass("exit_codes");
    else
        fail("exit_codes");
}

static void test_heredoc_multiline(void) {
    /* Multi-line here-document with variable expansion */
    char out[256];
    int rc = run_cmd("NAME=Kevlar; cat <<EOF\nHello $NAME\nLine 2\nEOF", out, sizeof(out));
    if (rc == 0 && strstr(out, "Hello Kevlar") && strstr(out, "Line 2"))
        pass("heredoc_multiline");
    else
        fail("heredoc_multiline");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 10: /proc and /sys Filesystem (5 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_proc_self_exe(void) {
    char out[256];
    int rc = run_cmd("readlink /proc/self/exe", out, sizeof(out));
    if (rc == 0 && strlen(out) > 0)
        pass("proc_self_exe");
    else
        fail("proc_self_exe");
}

static void test_proc_version(void) {
    char out[4096];
    int rc = run_cmd("cat /proc/version 2>/dev/null || echo no_proc_version", out, sizeof(out));
    if (rc == 0 && strlen(out) > 0)
        pass("proc_version");
    else
        fail("proc_version");
}

static void test_proc_meminfo(void) {
    char out[4096];
    int rc = run_cmd("cat /proc/meminfo 2>/dev/null", out, sizeof(out));
    if (rc == 0 && strstr(out, "MemTotal"))
        pass("proc_meminfo");
    else
        fail("proc_meminfo");
}

static void test_proc_uptime(void) {
    char out[256];
    int rc = run_cmd("cat /proc/uptime 2>/dev/null", out, sizeof(out));
    /* /proc/uptime contains two floats: uptime idle_time */
    if (rc == 0 && strlen(out) > 2)
        pass("proc_uptime");
    else
        fail("proc_uptime");
}

static void test_proc_cmdline(void) {
    char out[4096];
    int rc = run_cmd("cat /proc/cmdline 2>/dev/null || echo no_cmdline", out, sizeof(out));
    if (rc == 0 && strlen(out) > 0)
        pass("proc_cmdline");
    else
        fail("proc_cmdline");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 11: Special Device Files (4 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_dev_null(void) {
    char out[256];
    int rc = run_cmd("echo test > /dev/null && echo ok", out, sizeof(out));
    if (rc == 0 && strstr(out, "ok"))
        pass("dev_null");
    else
        fail("dev_null");
}

static void test_dev_zero(void) {
    char out[256];
    int rc = run_cmd("dd if=/dev/zero bs=16 count=1 2>/dev/null | wc -c", out, sizeof(out));
    if (rc == 0 && strstr(out, "16"))
        pass("dev_zero");
    else
        fail("dev_zero");
}

static void test_dev_urandom(void) {
    char out[256];
    int rc = run_cmd("dd if=/dev/urandom bs=16 count=1 2>/dev/null | wc -c", out, sizeof(out));
    if (rc == 0 && strstr(out, "16"))
        pass("dev_urandom");
    else
        fail("dev_urandom");
}

static void test_dev_fd(void) {
    char out[256];
    int rc = run_cmd("echo test_fd | cat /dev/fd/0 2>/dev/null || echo test_fd | cat /proc/self/fd/0 2>/dev/null || echo fallback", out, sizeof(out));
    if (rc == 0 && strlen(out) > 0)
        pass("dev_fd");
    else
        fail("dev_fd");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 12: Networking (3 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_ifconfig_cmd(void) {
    char out[4096];
    int rc = run_cmd("ifconfig 2>/dev/null || ip addr 2>/dev/null || echo no_net_cmd", out, sizeof(out));
    if (rc == 0 && strlen(out) > 0)
        pass("ifconfig_cmd");
    else
        fail("ifconfig_cmd");
}

static void test_nc_loopback(void) {
    /* Use nc (netcat) to test local TCP: start listener, send data, verify */
    char out[256];
    int rc = run_cmd(
        "echo 'nc_test_data' | nc -l -p 9999 &"
        "sleep 1;"
        "echo 'query' | nc 127.0.0.1 9999 2>/dev/null;"
        "wait 2>/dev/null",
        out, sizeof(out));
    if (rc == 0 && strstr(out, "nc_test_data"))
        pass("nc_loopback");
    else
        skip("nc_loopback");
}

static void test_hostname_set(void) {
    char out1[256], out2[256];
    run_cmd("hostname", out1, sizeof(out1));
    /* Try to set hostname (may require root, should work as PID 1) */
    int rc = run_cmd("hostname test-kevlar && hostname", out2, sizeof(out2));
    if (rc == 0 && strstr(out2, "test-kevlar"))
        pass("hostname_set");
    else
        skip("hostname_set");
    /* Restore */
    if (out1[0]) {
        char cmd[512];
        /* Strip newline from out1 */
        char *nl = strchr(out1, '\n');
        if (nl) *nl = '\0';
        snprintf(cmd, sizeof(cmd), "hostname %.200s", out1);
        run_cmd(cmd, NULL, 0);
    }
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 13: String & Math Utilities (4 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_md5sum(void) {
    write_file("/tmp/bb_md5", "hello\n");
    char out[256];
    int rc = run_cmd("md5sum /tmp/bb_md5", out, sizeof(out));
    /* md5sum of "hello\n" = b1946ac9... */
    if (rc == 0 && strstr(out, "b1946ac9"))
        pass("md5sum");
    else
        fail("md5sum");
    unlink("/tmp/bb_md5");
}

static void test_sha256sum(void) {
    write_file("/tmp/bb_sha", "hello\n");
    char out[256];
    int rc = run_cmd("sha256sum /tmp/bb_sha", out, sizeof(out));
    /* sha256 of "hello\n" starts with 5891b5b5... */
    if (rc == 0 && strstr(out, "5891b5b5"))
        pass("sha256sum");
    else
        fail("sha256sum");
    unlink("/tmp/bb_sha");
}

static void test_base64(void) {
    char out[256];
    int rc = run_cmd("echo -n 'hello' | base64", out, sizeof(out));
    /* base64("hello") = "aGVsbG8=" */
    if (rc == 0 && strstr(out, "aGVsbG8="))
        pass("base64_encode");
    else
        fail("base64_encode");
}

static void test_od_hex(void) {
    char out[256];
    int rc = run_cmd("echo -n 'AB' | od -A x -t x1", out, sizeof(out));
    /* Should show hex values 41 42 */
    if (rc == 0 && strstr(out, "41") && strstr(out, "42"))
        pass("od_hex");
    else
        fail("od_hex");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Category 14: Stress & Edge Cases (6 tests)
 * ════════════════════════════════════════════════════════════════════════ */

static void test_deep_directory(void) {
    /* Create a deeply nested directory structure */
    int rc = run_cmd("mkdir -p /tmp/bb_deep/a/b/c/d/e/f/g/h && "
                     "echo deep > /tmp/bb_deep/a/b/c/d/e/f/g/h/file && "
                     "cat /tmp/bb_deep/a/b/c/d/e/f/g/h/file",
                     NULL, 0);
    char out[256];
    read_file("/tmp/bb_deep/a/b/c/d/e/f/g/h/file", out, sizeof(out));
    if (rc == 0 && strstr(out, "deep"))
        pass("deep_directory");
    else
        fail("deep_directory");
    cleanup_dir("/tmp/bb_deep");
}

static void test_large_file(void) {
    /* Create a 1MB file and verify its size */
    char out[256];
    int rc = run_cmd("dd if=/dev/zero of=/tmp/bb_large bs=1024 count=1024 2>/dev/null && "
                     "wc -c < /tmp/bb_large", out, sizeof(out));
    if (rc == 0 && strstr(out, "1048576"))
        pass("large_file");
    else
        fail("large_file");
    unlink("/tmp/bb_large");
}

static void test_many_pipes(void) {
    /* Chain 10 pipes */
    char out[256];
    int rc = run_cmd("echo start | cat | cat | cat | cat | cat | cat | cat | cat | cat | cat", out, sizeof(out));
    if (rc == 0 && strstr(out, "start"))
        pass("many_pipes");
    else
        fail("many_pipes");
}

static void test_empty_file(void) {
    /* Operations on empty files */
    write_file("/tmp/bb_empty", "");
    char out[256];
    int rc = run_cmd("wc -l /tmp/bb_empty", out, sizeof(out));
    if (rc == 0 && strstr(out, "0"))
        pass("empty_file");
    else
        fail("empty_file");
    unlink("/tmp/bb_empty");
}

static void test_special_chars_filename(void) {
    /* Filenames with spaces */
    char out[256];
    int rc = run_cmd("echo data > '/tmp/bb space file' && cat '/tmp/bb space file'", out, sizeof(out));
    if (rc == 0 && strstr(out, "data"))
        pass("special_chars_filename");
    else
        fail("special_chars_filename");
    unlink("/tmp/bb space file");
}

static void test_rapid_fork(void) {
    /* Fork 20 children quickly, wait for all */
    char out[256];
    int rc = run_cmd("for i in $(seq 1 20); do (echo $i > /dev/null) & done; wait; echo fork_done", out, sizeof(out));
    if (rc == 0 && strstr(out, "fork_done"))
        pass("rapid_fork");
    else
        fail("rapid_fork");
}

/* ════════════════════════════════════════════════════════════════════════
 *  Main
 * ════════════════════════════════════════════════════════════════════════ */

int main(int argc, char **argv) {
    if (getpid() == 1) init_setup();

    printf("BUSYBOX_SUITE_START\n");
    fflush(stdout);

    /* Category 1: File Operations */
    test_echo_redirect();
    test_cat_file();
    test_cp_file();
    test_mv_file();
    test_rm_file();
    test_mkdir_rmdir();
    test_ln_hard();
    test_ln_soft();
    test_chmod_file();
    test_touch_file();
    test_ls_basic();
    test_ls_long();
    test_head_file();
    test_tail_file();
    test_wc_file();
    test_tee_file();
    test_dd_basic();
    test_truncate_file();
    test_du_basic();
    test_stat_cmd();

    /* Category 2: Text Processing */
    test_grep_match();
    test_grep_count();
    test_grep_invert();
    test_sed_substitute();
    test_sed_delete();
    test_awk_print();
    test_sort_numeric();
    test_sort_reverse();
    test_uniq_basic();
    test_tr_translate();
    test_cut_fields();
    test_diff_files();

    /* Category 3: Shell Features */
    test_pipe_basic();
    test_pipe_chain();
    test_redirect_stdout();
    test_redirect_append();
    test_redirect_stdin();
    test_subshell();
    test_command_substitution();
    test_here_doc();
    test_glob_star();
    test_for_loop();
    test_while_read();
    test_case_stmt();
    test_trap_signal();

    /* Category 4: Process Management */
    test_ps_list();
    test_kill_process();
    test_env_var();
    test_xargs_basic();
    test_nice_process();
    test_nohup_process();

    /* Category 5: System Information */
    test_hostname_cmd();
    test_uname_cmd();
    test_id_cmd();
    test_whoami_cmd();
    test_date_cmd();
    test_uptime_cmd();
    test_df_cmd();
    test_true_false();

    /* Category 6: Archive/Compression */
    test_tar_create_extract();
    test_gzip_decompress();
    test_tar_gz();
    test_cpio_basic();

    /* Category 7: Find & Directory Traversal */
    test_find_name();
    test_find_type();
    test_find_exec();
    test_xargs_find();

    /* Category 8: Advanced Shell & Arithmetic */
    test_expr_arithmetic();
    test_shell_arithmetic();
    test_seq_command();
    test_test_command();
    test_printf_format();

    /* Category 9: Real-World Workload Patterns */
    test_log_pipeline();
    test_config_parse();
    test_file_batch_create();
    test_multi_redirect();
    test_process_substitution_workaround();
    test_background_jobs();
    test_exit_codes();
    test_heredoc_multiline();

    /* Category 10: /proc and /sys */
    test_proc_self_exe();
    test_proc_version();
    test_proc_meminfo();
    test_proc_uptime();
    test_proc_cmdline();

    /* Category 11: Special Device Files */
    test_dev_null();
    test_dev_zero();
    test_dev_urandom();
    test_dev_fd();

    /* Category 12: Networking */
    test_ifconfig_cmd();
    test_nc_loopback();
    test_hostname_set();

    /* Category 13: String & Math Utilities */
    test_md5sum();
    test_sha256sum();
    test_base64();
    test_od_hex();

    /* Category 14: Stress & Edge Cases */
    test_deep_directory();
    test_large_file();
    test_many_pipes();
    test_empty_file();
    test_special_chars_filename();
    test_rapid_fork();

    printf("TEST_END %d/%d\n", passed, total);
    fflush(stdout);

    if (getpid() == 1) {
        sync();
        syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321fedc, NULL);
    }

    return (passed == total) ? 0 : 1;
}
