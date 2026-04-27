/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/init.h>
 *
 * `module_init(fn)` aliases an `init_module` symbol to the user's
 * init function — that's the symbol Kevlar's K1 loader looks up
 * by name.
 *
 * `__init` / `__exit` / `__initdata` / `__exitdata` are no-ops in
 * K7 (Linux strips these into separate sections that get freed
 * after init; Kevlar leaves them in `.text` permanently).
 */
#ifndef _LINUX_INIT_H
#define _LINUX_INIT_H

#define __init
#define __exit
#define __initdata
#define __exitdata
#define __initconst
#define __exitconst

/* Real Linux declares this as a function pointer typedef; K7
 * doesn't need the type for hello-world but provides it for
 * any module that references it. */
typedef int (*initcall_t)(void);
typedef void (*exitcall_t)(void);

/* `module_init(initfn)` → `int init_module(void) __alias("initfn")`
 *
 * Linux 6.12's expansion adds two extra clauses:
 *   - `static inline initcall_t __maybe_unused __inittest(void)
 *      { return initfn; }` — type-checks initfn matches initcall_t
 *   - `___ADDRESSABLE(init_module, __initdata)` — keeps the symbol
 *      from being optimized away
 *
 * K7's simplified form drops both — sufficient for our loader. */
#define module_init(initfn) \
    int init_module(void) __attribute__((alias(#initfn)))

#define module_exit(exitfn) \
    void cleanup_module(void) __attribute__((alias(#exitfn)))

#endif /* _LINUX_INIT_H */
