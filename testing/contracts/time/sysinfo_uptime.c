/* Contract: sysinfo returns valid uptime, memory info, process count. */
#include <errno.h>
#include <stdio.h>
#include <sys/sysinfo.h>

int main(void) {
    struct sysinfo si;
    if (sysinfo(&si) != 0) {
        printf("CONTRACT_FAIL sysinfo: errno=%d\n", errno);
        return 1;
    }

    /* uptime >= 0 (kernel may boot in under 1 second) */
    if (si.uptime < 0) {
        printf("CONTRACT_FAIL uptime: %ld\n", si.uptime);
        return 1;
    }
    printf("uptime: ok\n");

    /* totalram > 0 */
    if (si.totalram == 0) {
        printf("CONTRACT_FAIL totalram: 0\n");
        return 1;
    }
    printf("totalram: ok\n");

    /* freeram <= totalram */
    if (si.freeram > si.totalram) {
        printf("CONTRACT_FAIL freeram: free=%lu total=%lu\n", si.freeram, si.totalram);
        return 1;
    }
    printf("freeram: ok\n");

    /* procs >= 1 */
    if (si.procs < 1) {
        printf("CONTRACT_FAIL procs: %u\n", si.procs);
        return 1;
    }
    printf("procs: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
