/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause */
/* Kevlar kABI K3 — Module-side header for K3 demo .ko files.
 *
 * K3 adds the Linux device-model spine: struct device,
 * struct device_driver, struct bus_type, struct platform_device,
 * struct platform_driver.  Layouts are Kevlar-shape (opaque
 * `_kevlar_inner` slots); Linux struct-layout faithfulness defers
 * to a future milestone (K6+) when prebuilt Linux modules first
 * need to load.
 */
#ifndef KEVLAR_KABI_K3_H
#define KEVLAR_KABI_K3_H

#include "kevlar_kabi_k2.h"

/* ── kref ───────────────────────────────────────────────────── */
struct kref {
    int refcount;  /* AtomicI32 in Rust; we just expose the bytes */
};

extern void kref_init(struct kref *k);
extern void kref_get(struct kref *k);
extern int  kref_put(struct kref *k, void (*release)(struct kref *));
extern unsigned int kref_read(const struct kref *k);

/* ── kobject ────────────────────────────────────────────────── */
struct kobject { void *_kevlar_inner; };

extern void kobject_init(struct kobject *k, const void *ktype);
extern struct kobject *kobject_get(struct kobject *k);
extern void kobject_put(struct kobject *k);
extern int  kobject_set_name(struct kobject *k, const char *name);
extern int  kobject_add(struct kobject *k, struct kobject *parent,
                        const char *name);
extern void kobject_del(struct kobject *k);

/* ── struct device / driver / bus ───────────────────────────── */
struct bus_type;
struct device_driver;
struct device;

struct device {
    void                       *_kevlar_inner;
    struct device              *parent;
    const struct bus_type      *bus;
    struct device_driver       *driver;
    void                       *driver_data;
    const char                 *init_name;
};

struct device_driver {
    const char                 *name;
    const struct bus_type      *bus;
    int                       (*probe)(struct device *);
    int                       (*remove)(struct device *);
    void                       *_kevlar_inner;
};

struct bus_type {
    const char                 *name;
    int (*match)(struct device *, const struct device_driver *);
    void                       *_kevlar_inner;
};

extern void device_initialize(struct device *d);
extern int  device_add(struct device *d);
extern int  device_register(struct device *d);
extern void device_unregister(struct device *d);
extern struct device *get_device(struct device *d);
extern void put_device(struct device *d);
extern void  dev_set_drvdata(struct device *d, void *data);
extern void *dev_get_drvdata(const struct device *d);

extern int  driver_register(struct device_driver *drv);
extern void driver_unregister(struct device_driver *drv);
extern int  bus_register(struct bus_type *bus);
extern void bus_unregister(struct bus_type *bus);

/* ── platform devices + drivers ─────────────────────────────── */
struct platform_device {
    const char                 *name;
    int                         id;
    struct device               dev;
    void                       *_kevlar_inner;
};

struct platform_driver {
    int  (*probe)(struct platform_device *);
    int  (*remove)(struct platform_device *);
    struct device_driver        driver;
};

extern struct bus_type platform_bus_type;

extern int  platform_device_register(struct platform_device *pdev);
extern void platform_device_unregister(struct platform_device *pdev);
extern int  platform_driver_register(struct platform_driver *pdrv);
extern void platform_driver_unregister(struct platform_driver *pdrv);
extern int  __platform_driver_register(struct platform_driver *pdrv,
                                       const void *module);
extern void platform_set_drvdata(struct platform_device *pdev, void *data);
extern void *platform_get_drvdata(const struct platform_device *pdev);

#endif /* KEVLAR_KABI_K3_H */
