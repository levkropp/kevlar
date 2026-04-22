// TLB-shootdown stress test — exercises the exact kernel code paths
// that the blog-199 and in-progress munmap/mprotect/madvise/vm.rs
// TLB-deadlock fix targets.  Runs in <10 seconds, replaces multi-run
// test-xfce validation for rapid iteration.
//
// Shape: one process, two threads, ONE shared Vm:
//   Thread A: tight loop of mprotect(PROT_NONE) → mprotect(RW) →
//     madvise(MADV_DONTNEED) on rolling regions within a
//     pre-allocated arena. Each syscall issues a cross-CPU TLB
//     flush. The arena itself is never munmapped, so the reader
//     doesn't race with VMA deletion.
//   Thread B: tight loop of writing to random addresses in the
//     arena.  Each write might page-fault (if MADV_DONTNEED
//     cleared the PTE) or might hit PROT_NONE transiently.
//     PROT_NONE windows use SIGSEGV-catch to resume; that tests
//     the page-fault-with-signal-delivery path as well.
//
// Under -smp 2 this reproduces the deadlock pattern from blog 199
// (mmu writer holds Vm lock across flush_tlb_remote while remote
// CPU's page-fault handler spins on Vm lock_no_irq with IF=0).
// Healthy run: DONE prints with read/write/fault counts.
// Broken kernel: NMI watchdog fires, KERNEL_PTR_LEAK appears, or
// test never reaches DONE.
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <unistd.h>

#ifndef MADV_DONTNEED
#define MADV_DONTNEED 4
#endif

#define ARENA_BYTES  (4 * 1024 * 1024)   // 4 MiB, spans multiple PTs
#define REGION_BYTES (64 * 1024)         // mprotect/madvise granule
#define NREGIONS     (ARENA_BYTES / REGION_BYTES)
#define WRITER_ITERS 50000
#define READER_ITERS 200000

static uint8_t *arena;
static _Atomic(int) stop_flag;

// Per-thread SIGSEGV jumpbuf for the reader. The reader uses
// sigsetjmp + siglongjmp so an intentional PROT_NONE fault
// doesn't kill the process.
static __thread sigjmp_buf segv_jmp;
static __thread _Atomic(int) segv_armed;

static void segv_handler(int sig, siginfo_t *si, void *ctx) {
    (void)sig; (void)si; (void)ctx;
    if (atomic_load_explicit(&segv_armed, memory_order_acquire)) {
        siglongjmp(segv_jmp, 1);
    }
    // Not armed — let the default handler run (will core/print).
    _exit(128 + sig);
}

static uint32_t xrand(uint32_t *state) {
    uint32_t x = *state;
    x ^= x << 13; x ^= x >> 17; x ^= x << 5;
    *state = x;
    return x;
}

static void *writer_thread(void *_arg) {
    (void)_arg;
    uint32_t rng = 0xabcd1234;
    for (uint32_t i = 0; i < WRITER_ITERS; i++) {
        uint32_t region = xrand(&rng) % NREGIONS;
        uint8_t *p = arena + region * REGION_BYTES;
        uint32_t op = xrand(&rng) % 4;
        switch (op) {
            case 0:
                // mprotect cycle: RW → NONE → RW. Two TLB flushes.
                mprotect(p, REGION_BYTES, PROT_NONE);
                mprotect(p, REGION_BYTES, PROT_READ | PROT_WRITE);
                break;
            case 1:
                // madvise(DONTNEED) — drops PTEs, next access zero-faults.
                madvise(p, REGION_BYTES, MADV_DONTNEED);
                break;
            case 2:
                // Write a pattern so the reader has something valid to see.
                for (uint32_t k = 0; k < REGION_BYTES; k += 4096) {
                    ((volatile uint32_t *)(p + k))[0] = i;
                }
                break;
            case 3:
                // Double-madvise, then write — forces demand-fault cycle.
                madvise(p, REGION_BYTES, MADV_DONTNEED);
                ((volatile uint32_t *)p)[0] = i;
                break;
        }
    }
    atomic_store_explicit(&stop_flag, 1, memory_order_release);
    return NULL;
}

static void *reader_thread(void *_arg) {
    (void)_arg;
    uint32_t rng = 0x9876feed;
    uint64_t reads = 0, writes = 0, faults = 0;

    struct sigaction sa = {
        .sa_sigaction = segv_handler,
        .sa_flags = SA_SIGINFO | SA_NODEFER,
    };
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, NULL);

    for (uint64_t i = 0; i < READER_ITERS; i++) {
        if (atomic_load_explicit(&stop_flag, memory_order_acquire)) break;
        uint32_t off = xrand(&rng) % (ARENA_BYTES - 4);
        volatile uint32_t *p = (volatile uint32_t *)(arena + (off & ~3u));

        atomic_store_explicit(&segv_armed, 1, memory_order_release);
        if (sigsetjmp(segv_jmp, 1) == 0) {
            if ((i & 1) == 0) {
                // Read.
                uint32_t v = *p;
                reads++;
                // KERNEL_PTR_LEAK check: any value with top kernel bits
                // set is suspect. Report as a test-level failure so the
                // harness catches it in a grep.
                if ((v & 0xfff80000u) == 0xfff80000u) {
                    printf("FAIL reader saw kernel-VA-looking value %08x at arena+%u\n",
                           v, off & ~3u);
                }
            } else {
                // Write.
                *p = (uint32_t)i;
                writes++;
            }
        } else {
            // SIGSEGV (PROT_NONE transient) — count and continue.
            faults++;
        }
        atomic_store_explicit(&segv_armed, 0, memory_order_release);
    }
    printf("reader: reads=%llu writes=%llu faults=%llu\n",
           (unsigned long long)reads,
           (unsigned long long)writes,
           (unsigned long long)faults);
    return NULL;
}

int main(void) {
    printf("=== TLB-shootdown stress test ===\n");
    printf("  arena: %d bytes, %d regions × %d bytes\n",
           ARENA_BYTES, NREGIONS, REGION_BYTES);
    printf("  writer: %d iters, reader: %d iters\n",
           WRITER_ITERS, READER_ITERS);
    fflush(stdout);

    arena = mmap(NULL, ARENA_BYTES, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (arena == MAP_FAILED) {
        printf("FAIL mmap arena: errno=%d\n", errno); return 1;
    }

    // Touch every page so they're all mapped before the stress starts.
    for (int i = 0; i < ARENA_BYTES; i += 4096) arena[i] = 0;

    struct timeval t0, t1;
    gettimeofday(&t0, NULL);

    pthread_t w, r;
    pthread_create(&w, NULL, writer_thread, NULL);
    pthread_create(&r, NULL, reader_thread, NULL);
    pthread_join(w, NULL);
    pthread_join(r, NULL);

    gettimeofday(&t1, NULL);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_usec - t0.tv_usec) / 1000;

    munmap(arena, ARENA_BYTES);
    printf("DONE %ld ms — no deadlock, no kernel panic\n", ms);
    fflush(stdout);
    return 0;
}
