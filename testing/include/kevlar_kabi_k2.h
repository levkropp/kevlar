/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K2 — Module-side header for K2 demo .ko files.
 *
 * K2 ships opaque struct types + extern function prototypes for the
 * Linux primitives the K2 surface implements (kmalloc/wait_queue/
 * completion/work_struct/...).  Linux struct-layout faithfulness is
 * deferred to K3+; for K2 the demo modules and the kernel agree on
 * these Kevlar-specific shapes.
 */
#ifndef KEVLAR_KABI_K2_H
#define KEVLAR_KABI_K2_H

#ifndef NULL
#define NULL ((void *)0)
#endif

typedef unsigned long size_t;
typedef int int32_t;
typedef unsigned int uint32_t;
typedef unsigned long long uint64_t;
typedef long long int64_t;

/* printk — K1 export. Format-string-only; %-tokens are ignored. */
extern void printk(const char *fmt);

/* ── Allocators ──────────────────────────────────────────────── */
extern void *kmalloc(size_t size, uint32_t gfp);
extern void *kzalloc(size_t size, uint32_t gfp);
extern void *kcalloc(size_t n, size_t size, uint32_t gfp);
extern void *krealloc(void *ptr, size_t new_size, uint32_t gfp);
extern void  kfree(void *ptr);
extern void *vmalloc(size_t size);
extern void *vzalloc(size_t size);
extern void  vfree(void *ptr);
extern void *kvmalloc(size_t size, uint32_t gfp);
extern void *kvzalloc(size_t size, uint32_t gfp);
extern void  kvfree(void *ptr);

/* ── Scheduler ──────────────────────────────────────────────── */
extern void *kabi_current(void);
extern int   kabi_current_pid(void);
extern void  kabi_current_comm(char *buf, size_t len);
extern void  msleep(uint32_t ms);
extern void  schedule(void);
extern int   cond_resched(void);
extern int64_t schedule_timeout(int64_t ticks);

/* ── wait_queue_head ────────────────────────────────────────── */
struct wait_queue_head { void *_kevlar_inner; };
typedef struct wait_queue_head wait_queue_head_t;

extern void init_waitqueue_head(struct wait_queue_head *wq);
extern void destroy_waitqueue_head(struct wait_queue_head *wq);
extern void wake_up(struct wait_queue_head *wq);
extern void wake_up_all(struct wait_queue_head *wq);
extern void wake_up_interruptible(struct wait_queue_head *wq);
extern void wake_up_interruptible_all(struct wait_queue_head *wq);
/* Sleep until cond(arg) returns non-zero. Returns 0 on success,
 * -EINTR (-4) on signal interruption. */
extern int  kabi_wait_event(struct wait_queue_head *wq,
                            int (*cond)(void *), void *arg);

/* ── completion ─────────────────────────────────────────────── */
struct completion { void *_kevlar_inner; };

extern void init_completion(struct completion *c);
extern void destroy_completion(struct completion *c);
extern void complete(struct completion *c);
extern void complete_all(struct completion *c);
extern void wait_for_completion(struct completion *c);

/* ── work_struct ────────────────────────────────────────────── */
struct work_struct {
    void *_kevlar_inner;
    void (*func)(struct work_struct *);
};

extern void kabi_init_work(struct work_struct *w,
                           void (*func)(struct work_struct *));
extern int  schedule_work(struct work_struct *w);
extern int  flush_work(struct work_struct *w);
extern int  cancel_work_sync(struct work_struct *w);

/* ── .modinfo metadata ──────────────────────────────────────── */
/* Linux's __MODULE_INFO emits NUL-terminated `key=value` strings
 * into the .modinfo section.  Each macro invocation creates a
 * uniquely-named static const char[] in .modinfo, with size
 * implicitly the string length + 1. */
#define __MODULE_INFO_PASTE(a, b) a##b
#define __MODULE_INFO_UNIQUE(line) __MODULE_INFO_PASTE(__module_info_, line)
#define MODULE_INFO(tag, val)                                            \
    static const char __MODULE_INFO_UNIQUE(__LINE__)[]                   \
        __attribute__((section(".modinfo"), aligned(1), used)) =         \
        #tag "=" val

#define MODULE_LICENSE(s)     MODULE_INFO(license, s)
#define MODULE_AUTHOR(s)      MODULE_INFO(author, s)
#define MODULE_DESCRIPTION(s) MODULE_INFO(description, s)
#define MODULE_VERSION(s)     MODULE_INFO(version, s)

#endif /* KEVLAR_KABI_K2_H */
