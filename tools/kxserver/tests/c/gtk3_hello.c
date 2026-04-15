/* SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
 *
 * Phase 12.3 minimal GTK3 hello-world.  Creates a GtkWindow with a
 * GtkLabel inside and runs the main loop for a fixed timeout, then
 * cleanly exits.  Forces the X11 backend so we go through Xlib/Xft
 * instead of Wayland.
 *
 * Build:
 *   gcc gtk3_hello.c $(pkg-config --cflags --libs gtk+-3.0) -o gtk3_hello
 *
 * Run:
 *   GDK_BACKEND=x11 DISPLAY=:N ./gtk3_hello -timeout 1500
 */
#include <gtk/gtk.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

static int timeout_ms = 1500;

static gboolean quit_main(gpointer data) {
    (void)data;
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

int main(int argc, char **argv) {
    /* Force X11; without this GTK picks Wayland on a Wayland session. */
    setenv("GDK_BACKEND", "x11", 1);

    /* Parse our own args so gtk_init doesn't choke on -timeout. */
    int gtk_argc = 1;
    char *gtk_argv[2] = { argv[0], NULL };
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-timeout") && i + 1 < argc) {
            timeout_ms = atoi(argv[++i]);
        }
    }
    char **pargv = gtk_argv;
    gtk_init(&gtk_argc, &pargv);

    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(win), "kxserver gtk3");
    gtk_window_set_default_size(GTK_WINDOW(win), 320, 120);
    gtk_window_move(GTK_WINDOW(win), 100, 100);

    GtkWidget *label = gtk_label_new("Hello kxserver\n(GTK3 + Xft path)");
    gtk_container_add(GTK_CONTAINER(win), label);

    gtk_widget_show_all(win);

    g_timeout_add(timeout_ms, quit_main, NULL);
    gtk_main();
    gtk_widget_destroy(win);
    return 0;
}
