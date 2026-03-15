/* Run all contract tests from /bin/contracts/ */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <dirent.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>

int main(void) {
    if (getpid() == 1) {
        mkdir("/proc", 0755);
        mount("proc", "/proc", "proc", 0, NULL);
        mkdir("/sys", 0755);
        mount("sysfs", "/sys", "sysfs", 0, NULL);
        mkdir("/tmp", 0755);
        mount("tmpfs", "/tmp", "tmpfs", 0, NULL);
        mount("tmpfs", "/dev", "tmpfs", 0, NULL);
        mkdir("/dev/shm", 0755);
        mount("tmpfs", "/dev/shm", "tmpfs", 0, NULL);
        mkdir("/sys/fs/cgroup", 0755);
        mount("cgroup2", "/sys/fs/cgroup", "cgroup2", 0, NULL);
        mknod("/dev/null", S_IFCHR | 0666, (1 << 8) | 3);
        mknod("/dev/zero", S_IFCHR | 0666, (1 << 8) | 5);
        mknod("/dev/urandom", S_IFCHR | 0666, (1 << 8) | 9);
        mknod("/dev/full", S_IFCHR | 0666, (1 << 8) | 7);
        mknod("/dev/kmsg", S_IFCHR | 0666, (1 << 8) | 11);
    }

    DIR *d = opendir("/bin/contracts");
    if (!d) { perror("opendir"); return 1; }

    int pass = 0, fail = 0, skip = 0;
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        if (ent->d_name[0] == '.') continue;
        char path[256];
        snprintf(path, sizeof(path), "/bin/contracts/%s", ent->d_name);

        pid_t pid = fork();
        if (pid == 0) {
            execl(path, ent->d_name, NULL);
            _exit(127);
        }
        int status;
        waitpid(pid, &status, 0);
        int rc = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
        if (rc == 0) {
            pass++;
            printf("PASS %s\n", ent->d_name);
        } else if (rc == 77) {
            skip++;
            printf("SKIP %s\n", ent->d_name);
        } else {
            fail++;
            printf("FAIL %s (rc=%d)\n", ent->d_name, rc);
        }
    }
    closedir(d);
    printf("=== %d passed, %d failed, %d skipped ===\n", pass, fail, skip);

    if (getpid() == 1)
        syscall(SYS_reboot, 0xfee1dead, 0x28121969, 0x4321fedc, 0);
    return fail > 0 ? 1 : 0;
}
