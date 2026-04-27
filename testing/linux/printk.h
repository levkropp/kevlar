/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K7 compat: <linux/printk.h>
 *
 * Wraps the existing variadic printk shim from K6.  The KERN_LEVEL
 * preambles match Linux's "\x01" + level-digit format; the K6
 * formatter strips them silently.
 */
#ifndef _LINUX_PRINTK_H
#define _LINUX_PRINTK_H

extern int printk(const char *fmt, ...);

#define KERN_SOH      "\x01"
#define KERN_EMERG    KERN_SOH "0"
#define KERN_ALERT    KERN_SOH "1"
#define KERN_CRIT     KERN_SOH "2"
#define KERN_ERR      KERN_SOH "3"
#define KERN_WARNING  KERN_SOH "4"
#define KERN_NOTICE   KERN_SOH "5"
#define KERN_INFO     KERN_SOH "6"
#define KERN_DEBUG    KERN_SOH "7"
#define KERN_DEFAULT  KERN_SOH "d"

#define pr_emerg(fmt, ...)   printk(KERN_EMERG   fmt, ##__VA_ARGS__)
#define pr_alert(fmt, ...)   printk(KERN_ALERT   fmt, ##__VA_ARGS__)
#define pr_crit(fmt, ...)    printk(KERN_CRIT    fmt, ##__VA_ARGS__)
#define pr_err(fmt, ...)     printk(KERN_ERR     fmt, ##__VA_ARGS__)
#define pr_warn(fmt, ...)    printk(KERN_WARNING fmt, ##__VA_ARGS__)
#define pr_warning           pr_warn  /* Linux legacy alias */
#define pr_notice(fmt, ...)  printk(KERN_NOTICE  fmt, ##__VA_ARGS__)
#define pr_info(fmt, ...)    printk(KERN_INFO    fmt, ##__VA_ARGS__)
#define pr_debug(fmt, ...)   printk(KERN_DEBUG   fmt, ##__VA_ARGS__)
#define pr_cont(fmt, ...)    printk(fmt, ##__VA_ARGS__)

#endif /* _LINUX_PRINTK_H */
