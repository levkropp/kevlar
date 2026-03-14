/* Contract: FUTEX_CMP_REQUEUE, FUTEX_WAKE_OP, FUTEX_WAIT_BITSET work correctly. */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/futex.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>

static int futex_op(uint32_t *uaddr, int op, uint32_t val,
                    const struct timespec *timeout, uint32_t *uaddr2, uint32_t val3) {
    return syscall(SYS_futex, uaddr, op, val, timeout, uaddr2, val3);
}

int main(void) {
    /* ── FUTEX_CMP_REQUEUE: val3 mismatch returns EAGAIN ─────────── */
    uint32_t futex1 = 42;
    uint32_t futex2 = 0;
    int ret = futex_op(&futex1, FUTEX_CMP_REQUEUE, 1,
                       (const struct timespec *)(uintptr_t)1, &futex2, 99);
    if (ret != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL futex_cmp_requeue_eagain: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_cmp_requeue_eagain: ok\n");

    /* ── FUTEX_CMP_REQUEUE: matching val3 with no waiters returns 0 ─ */
    ret = futex_op(&futex1, FUTEX_CMP_REQUEUE, 1,
                   (const struct timespec *)(uintptr_t)1, &futex2, 42);
    if (ret != 0) {
        printf("CONTRACT_FAIL futex_cmp_requeue_empty: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_cmp_requeue_empty: ok\n");

    /* ── FUTEX_WAKE_OP: apply SET operation and wake ─────────────── */
    uint32_t wake_target = 100;
    /* val3 encodes: op=SET(0), cmp=EQ(0), oparg=77, cmparg=100
       Bits: op=0x0, cmp=0x0, oparg=77=0x04D, cmparg=100=0x064
       val3 = (0<<28) | (0<<24) | (77<<12) | 100 = 0x0004D064 */
    uint32_t val3 = (0u << 28) | (0u << 24) | (77u << 12) | 100u;
    ret = futex_op(&futex1, FUTEX_WAKE_OP, 1,
                   (const struct timespec *)(uintptr_t)1, &wake_target, val3);
    if (ret < 0) {
        printf("CONTRACT_FAIL futex_wake_op: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    /* Verify the operation was applied: wake_target should now be 77 (SET) */
    if (wake_target != 77) {
        printf("CONTRACT_FAIL futex_wake_op_value: expected 77, got %u\n", wake_target);
        return 1;
    }
    printf("futex_wake_op: ok\n");

    /* ── FUTEX_WAIT_BITSET: value mismatch returns EAGAIN ────────── */
    uint32_t wait_val = 10;
    errno = 0;
    ret = futex_op(&wait_val, FUTEX_WAIT_BITSET, 99, NULL, NULL, FUTEX_BITSET_MATCH_ANY);
    if (ret != -1 || errno != EAGAIN) {
        printf("CONTRACT_FAIL futex_wait_bitset_eagain: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_wait_bitset_eagain: ok\n");

    /* ── FUTEX_WAIT_BITSET: bitset=0 returns EINVAL ──────────────── */
    errno = 0;
    ret = futex_op(&wait_val, FUTEX_WAIT_BITSET, 10, NULL, NULL, 0);
    if (ret != -1 || errno != EINVAL) {
        printf("CONTRACT_FAIL futex_wait_bitset_zero: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_wait_bitset_zero: ok\n");

    /* ── FUTEX_WAKE with no waiters returns 0 ────────────────────── */
    ret = futex_op(&futex1, FUTEX_WAKE, 1, NULL, NULL, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL futex_wake_empty: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_wake_empty: ok\n");

    /* ── FUTEX_PRIVATE_FLAG is stripped correctly ─────────────────── */
    ret = futex_op(&futex1, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
    if (ret != 0) {
        printf("CONTRACT_FAIL futex_wake_private: ret=%d errno=%d\n", ret, errno);
        return 1;
    }
    printf("futex_wake_private: ok\n");

    printf("CONTRACT_PASS\n");
    return 0;
}
