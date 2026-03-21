/* Contract: umask returns previous mask; new mask affects file creation
 * mode; umask(0) reads current mask without side effects (roundtrip). */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
    /* Save original umask */
    mode_t orig = umask(0);
    /* Restore it and verify roundtrip */
    mode_t check = umask(orig);
    if (check != 0) {
        printf("CONTRACT_FAIL roundtrip: expected=0 got=0%03o\n", check);
        return 1;
    }
    printf("roundtrip: ok orig=0%03o\n", orig);

    /* Set umask to 0077, create file, verify mode is masked */
    umask(0077);
    int fd = open("/tmp/umask_test", O_CREAT | O_WRONLY | O_TRUNC, 0666);
    if (fd < 0) {
        printf("CONTRACT_FAIL open: errno=%d\n", errno);
        return 1;
    }

    struct stat st;
    if (fstat(fd, &st) != 0) {
        printf("CONTRACT_FAIL fstat: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    /* 0666 & ~0077 = 0600 */
    mode_t expected = 0600;
    mode_t got = st.st_mode & 0777;
    if (got != expected) {
        printf("CONTRACT_FAIL masked_mode: expected=0%03o got=0%03o\n",
               expected, got);
        close(fd);
        return 1;
    }
    printf("masked_mode: ok mode=0%03o\n", got);
    close(fd);
    unlink("/tmp/umask_test");

    /* Set umask to 0, create file, should get full perms */
    umask(0);
    fd = open("/tmp/umask_test2", O_CREAT | O_WRONLY | O_TRUNC, 0777);
    if (fd < 0) {
        printf("CONTRACT_FAIL open2: errno=%d\n", errno);
        return 1;
    }
    if (fstat(fd, &st) != 0) {
        printf("CONTRACT_FAIL fstat2: errno=%d\n", errno);
        close(fd);
        return 1;
    }
    got = st.st_mode & 0777;
    if (got != 0777) {
        printf("CONTRACT_FAIL unmasked_mode: expected=0777 got=0%03o\n", got);
        close(fd);
        return 1;
    }
    printf("unmasked_mode: ok mode=0%03o\n", got);
    close(fd);
    unlink("/tmp/umask_test2");

    /* Restore original */
    umask(orig);
    printf("CONTRACT_PASS\n");
    return 0;
}
