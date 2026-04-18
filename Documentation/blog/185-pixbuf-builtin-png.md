## Blog 185: gdk-pixbuf, builtin PNG, and the empty loaders.cache entry

**Date:** 2026-04-19

Third XFCE userspace bug of the session. Looking at the test-xfce log
during a 4/4 run that I was celebrating — turns out "4/4 running
processes" was lying; xfdesktop had already crashed and respawned. The
session-log dump showed:

```
(xfdesktop:56): GLib-GObject-CRITICAL **: g_object_unref: assertion 'G_IS_OBJECT (object)' failed
(xfdesktop:56): GdkPixbuf-CRITICAL **: gdk_pixbuf_get_width: assertion 'GDK_IS_PIXBUF (pixbuf)' failed
(xfdesktop:56): GdkPixbuf-CRITICAL **: gdk_pixbuf_scale_simple: assertion 'GDK_IS_PIXBUF (src)' failed
** (xfdesktop:56): WARNING **: Unable to find fallback icon
(xfdesktop:56): Gtk-WARNING **: drawing failure for widget 'XfdesktopIconView': NULL pointer
Wnck:ERROR:../libwnck/xutils.c:1507:default_icon_at_size: assertion failed: (base)
Bail out! ...
```

Classic cascade: something returned a NULL `GdkPixbuf *`, xfdesktop
didn't check, passed it to `gdk_pixbuf_scale_simple`, assertion fired,
chain continued, libwnck finally ran out of graceful fallbacks and
aborted.

## Which image couldn't load

`xfdesktop` loads PNG icons constantly — the desktop wallpaper, the
folder icons on the desktop, the trash icon, and the fallback icon
when any of those fail. If *every* PNG load returns NULL, the
fallback path fails too.  That's exactly what happened.

## The loader is present; the cache isn't

`gdk-pixbuf` supports multiple image formats via pluggable loader
modules under `/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/`. Looking at
our Alpine rootfs:

```
libpixbufloader-ani.so    libpixbufloader-bmp.so    libpixbufloader-gif.so
libpixbufloader-icns.so   libpixbufloader-ico.so    libpixbufloader-pnm.so
libpixbufloader-qtif.so   libpixbufloader_svg.so    libpixbufloader-tga.so
libpixbufloader-tiff.so   libpixbufloader-xbm.so    libpixbufloader-xpm.so
```

No `libpixbufloader-png.so`. No `libpixbufloader-jpeg.so`. But:

```
$ strings libgdk_pixbuf-2.0.so.0 | grep gdk_pixbuf__png
gdk_pixbuf__png_image_load_increment
gdk_pixbuf__png_image_stop_load
gdk_pixbuf__png_image_begin_load
```

**PNG and JPEG are built into `libgdk_pixbuf-2.0.so.0` as static
modules, not shipped as separate `.so` plugins.**  Alpine's gdk-pixbuf
build uses `--enable-included-loaders=png,jpeg`.  Other formats stay
pluggable.

The kicker: the runtime still consults
`/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache` to know *which formats
it supports*. For a built-in loader, the cache format is:

```
""
"png" 6 "gdk-pixbuf" "The PNG image format" "LGPL"
"image/png" "png" ""
```

An empty first line (`""`) instead of a module path marks the entry
as built-in.  The runtime reads this as "I have a PNG loader; don't
`dlopen` anything, just use `gdk_pixbuf__png_image_begin_load`."

## Our loaders.cache was auto-generated from .so files only

`tools/build-alpine-xfce.py` generates `loaders.cache` by globbing
the loaders directory:

```python
modules = sorted(loader_so_dir.glob("*.so"))
for mod in modules:
    rel_path = "/" + str(mod.relative_to(root))
    name = mod.stem.replace("libpixbufloader-", "").replace("libpixbufloader_", "")
    f.write(f'"{rel_path}"\n')
    f.write(f'"{name}" 5 "gdk-pixbuf" "{name} image" "LGPL"\n')
    ...
```

No glob match for PNG (no `libpixbufloader-png.so`), no cache entry,
no runtime support. Every `gdk_pixbuf_new_from_file("*.png")` returns
NULL.

Why this hadn't surfaced in earlier tests: kxserver-visible and
test-x11-visible only used simple rendering (`xsetroot -solid`). Full
XFCE is the first workload that loads PNG icons.

## The fix

Emit the built-in PNG + JPEG entries explicitly in the generated
cache:

```python
# Alpine builds PNG + JPEG as BUILTIN (statically linked into
# libgdk_pixbuf-2.0.so). There's no .so for them, but the cache still
# needs explicit entries — the empty first line marks built-in.
f.write('""\n')
f.write('"png" 6 "gdk-pixbuf" "The PNG image format" "LGPL"\n')
f.write('"image/png" "png" ""\n\n')
f.write('""\n')
f.write('"jpeg" 5 "gdk-pixbuf" "The JPEG image format" "LGPL"\n')
f.write('"image/jpeg" "jpeg" ""\n\n')
```

Rebuilt the image, re-ran test-xfce.

## Result

10-run sample after the fix (1 hung, 9 completed):

| metric | before (PNG fix) | after (PNG fix) |
|---|---|---|
| xfdesktop SIGSEGV | every run (silent respawn) | 2/10 |
| xfce4-session SIGSEGV | 3/10 | 3/10 |
| Thunar SIGSEGV | 3/10 | 0 |
| xfwm4 SIGSEGV | 0 | 1/10 |
| kernel panics | 0 | 0 |
| score distribution | many silent xfdesktop respawns hidden behind "4/4 running" | 4/4 ×4, 3/4 ×2, 2/4 ×2, 1/4 ×1, hung ×1 |
| mean score (completed runs) | 3.0/4 (falsified by silent respawns) | 3.0/4 (accurate) |

The visible score didn't jump, but the *shape* of the failures changed —
Thunar went to 0 because it was crashing on the same icon-load path.
The remaining 2/10 xfdesktop SIGSEGVs are a different bug (fault
address and stack are distinct from the pixbuf cascade); they're on
the list to investigate next.

The "before" column reflects what we now know from log-dumping. The
old metric "4/4 running processes" was reading survivors, not
originals, and xfdesktop was dying on every start before Alpine's
respawn loop replaced it.

## The debugging path

This one wasn't found via the strace-diff harness — it would have
needed xfdesktop to actually run, which it can't do off-target.
Instead it came from:

1. Dumping `/tmp/xfce-session.log` from inside the chroot after the
   test's process-check. (Added one block of code to `test_xfce.c`.)
2. Reading the GLib-GObject-CRITICAL cascade and asking "what
   returned NULL first?"
3. `strings libgdk_pixbuf-2.0.so.0 | grep png` to confirm PNG was
   actually supported.
4. Comparing the written `loaders.cache` to what Alpine's normal
   tooling generates.

Total time: about 15 minutes, most of it re-reading gdk-pixbuf
documentation to remember that `""` is the built-in marker. A lesson:
when a userspace app crashes with "NULL wasn't allowed here", always
ask what made it NULL, not just how to dodge the crash.

## Tally so far this XFCE-userspace session

1. poll → POLLNVAL on invalid fd (kernel fix, [blog 184](184-poll-pollnval.md))
2. gdk-pixbuf loaders.cache missing PNG/JPEG (rootfs fix, this post)
3. TBD — the remaining xfce4-session NULL deref at ip=0xa0006ac00
4. TBD — the remaining xfdesktop 2/10 SIGSEGVs (different from pixbuf path)
5. TBD — xfwm4 1/10 SIGSEGV (new, likely surfaced because xfdesktop
   no longer dies first and xfwm4 now has time to hit its own bug)
