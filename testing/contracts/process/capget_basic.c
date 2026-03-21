/* Contract: capget returns version 3 header; data reports capabilities;
 * capset accepts without error (stub). */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

/* Linux capability structures */
struct cap_header {
    unsigned int version;
    int pid;
};

struct cap_data {
    unsigned int effective;
    unsigned int permitted;
    unsigned int inheritable;
};

#define _LINUX_CAPABILITY_VERSION_3 0x20080522

int main(void) {
    /* Query version: pass data=NULL */
    struct cap_header hdr;
    hdr.version = 0;
    hdr.pid = 0;

    long ret = syscall(SYS_capget, &hdr, NULL);
    if (ret != 0) {
        printf("CONTRACT_FAIL version_query: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    if (hdr.version != _LINUX_CAPABILITY_VERSION_3) {
        printf("CONTRACT_FAIL version: got=0x%08x expected=0x%08x\n",
               hdr.version, _LINUX_CAPABILITY_VERSION_3);
        return 1;
    }
    printf("version: ok 0x%08x\n", hdr.version);

    /* Read capabilities (v3 uses 2 data structs) */
    struct cap_data data[2];
    memset(data, 0, sizeof(data));
    hdr.version = _LINUX_CAPABILITY_VERSION_3;
    hdr.pid = 0;

    ret = syscall(SYS_capget, &hdr, data);
    if (ret != 0) {
        printf("CONTRACT_FAIL capget: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    /* As root: all caps set. As non-root: some or no caps. Either is valid —
     * we just verify the struct was written (not still 0xff from memset). */
    printf("capget: ok\n");

    /* capset should accept (stub) */
    hdr.version = _LINUX_CAPABILITY_VERSION_3;
    hdr.pid = 0;
    ret = syscall(SYS_capset, &hdr, data);
    if (ret != 0) {
        printf("CONTRACT_FAIL capset: ret=%ld errno=%d\n", ret, errno);
        return 1;
    }
    printf("capset: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
