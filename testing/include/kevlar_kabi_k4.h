/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K4 — Module-side header for K4 demo .ko files.
 *
 * K4 adds Linux's char-device + file_operations surface, so a
 * loaded module can register /dev/<name> backed by callbacks.
 */
#ifndef KEVLAR_KABI_K4_H
#define KEVLAR_KABI_K4_H

#include "kevlar_kabi_k3.h"

typedef long       ssize_t;
typedef long long  loff_t;
typedef unsigned int dev_t;

struct file;
struct inode;

struct file {
    void           *_kevlar_inner;
    void           *private_data;
    loff_t          f_pos;
    unsigned int    f_flags;
    unsigned int    _pad;
};

struct inode {
    void           *_kevlar_inner;
    unsigned int    i_rdev;
    unsigned int    _pad;
    loff_t          i_size;
};

struct file_operations {
    void           *owner;  /* struct module * — unused in K4 */
    loff_t   (*llseek)(struct file *, loff_t, int);
    ssize_t  (*read)(struct file *, char *, size_t, loff_t *);
    ssize_t  (*write)(struct file *, const char *, size_t, loff_t *);
    long     (*unlocked_ioctl)(struct file *, unsigned int, unsigned long);
    unsigned int (*poll)(struct file *, void *);
    int      (*mmap)(struct file *, void *);
    int      (*open)(struct inode *, struct file *);
    int      (*release)(struct inode *, struct file *);
};

struct cdev {
    void                       *_kevlar_inner;
    const struct file_operations *ops;
    dev_t                       dev;
    unsigned int                count;
};

extern int  alloc_chrdev_region(dev_t *dev, unsigned baseminor,
                                unsigned count, const char *name);
extern int  register_chrdev_region(dev_t first, unsigned count,
                                   const char *name);
extern void unregister_chrdev_region(dev_t first, unsigned count);

extern void cdev_init(struct cdev *cdev,
                      const struct file_operations *fops);
extern int  cdev_add(struct cdev *cdev, dev_t dev, unsigned count);
extern void cdev_del(struct cdev *cdev);

extern int  register_chrdev(unsigned major, const char *name,
                            const struct file_operations *fops);
extern void unregister_chrdev(unsigned major, const char *name);

/* user-pointer access (K4: kernel-buffer staging — see usercopy.rs).
 * Linux convention: returns "bytes NOT copied" — 0 = full success. */
extern unsigned long copy_to_user(void *to, const void *from, unsigned long n);
extern unsigned long copy_from_user(void *to, const void *from, unsigned long n);
extern unsigned long clear_user(void *to, unsigned long n);
extern unsigned long strnlen_user(const char *s, unsigned long n);

#endif /* KEVLAR_KABI_K4_H */
