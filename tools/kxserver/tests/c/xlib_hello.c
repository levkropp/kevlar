/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Phase 12.1 raw Xlib hello-world.
 *
 * Opens the display, creates a 320x120 window, maps it, draws "Hello
 * kxserver" with XDrawString using the server's default font, runs
 * an event loop that exits on the first Expose or after a timeout.
 *
 * Deliberately uses XDrawString (NOT Xft) so this exercises the
 * core X11 font path from Phase 6, separate from the RENDER/Xft
 * path that dmenu will stress next.
 *
 * Build: gcc -Wall -O2 xlib_hello.c -lX11 -o xlib_hello
 */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static const char *MSG = "Hello kxserver";

int main(int argc, char **argv) {
    const char *display_name = NULL;
    int timeout_ms = 1000;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-display") && i + 1 < argc) {
            display_name = argv[++i];
        } else if (!strcmp(argv[i], "-timeout") && i + 1 < argc) {
            timeout_ms = atoi(argv[++i]);
        }
    }

    Display *dpy = XOpenDisplay(display_name);
    if (!dpy) {
        fprintf(stderr, "XOpenDisplay failed\n");
        return 1;
    }
    int screen = DefaultScreen(dpy);
    Window root = RootWindow(dpy, screen);
    unsigned long white = WhitePixel(dpy, screen);
    unsigned long black = BlackPixel(dpy, screen);

    Window win = XCreateSimpleWindow(
        dpy, root,
        100, 100, 320, 120,
        1, black, white
    );
    XSelectInput(dpy, win, ExposureMask | KeyPressMask | StructureNotifyMask);

    /* ICCCM WM_NAME so a real WM can put something in the title bar. */
    XStoreName(dpy, win, "kxserver hello");

    XMapWindow(dpy, win);

    /* Default GC uses the server's default font. */
    GC gc = XCreateGC(dpy, win, 0, NULL);
    XSetForeground(dpy, gc, black);

    struct timespec start;
    clock_gettime(CLOCK_MONOTONIC, &start);

    int saw_expose = 0;
    while (1) {
        while (XPending(dpy)) {
            XEvent ev;
            XNextEvent(dpy, &ev);
            switch (ev.type) {
            case Expose:
                XDrawString(dpy, win, gc, 20, 50, MSG, strlen(MSG));
                XDrawString(dpy, win, gc, 20, 80, "(core X11 path)", 15);
                XFlush(dpy);
                saw_expose = 1;
                break;
            case ConfigureNotify:
                break;
            case MapNotify:
                break;
            case KeyPress:
                goto done;
            }
        }
        /* Drive the loop to the server every 10ms so request round-
         * trips don't starve. */
        usleep(10 * 1000);
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        long elapsed_ms = (now.tv_sec - start.tv_sec) * 1000
                        + (now.tv_nsec - start.tv_nsec) / 1000000;
        if (elapsed_ms > timeout_ms) {
            break;
        }
    }
done:
    if (!saw_expose) {
        fprintf(stderr, "xlib_hello: no Expose received within %d ms\n", timeout_ms);
    }
    XFreeGC(dpy, gc);
    XDestroyWindow(dpy, win);
    XCloseDisplay(dpy);
    return saw_expose ? 0 : 2;
}
