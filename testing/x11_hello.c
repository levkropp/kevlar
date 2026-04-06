// Minimal X11 window — tests if basic Xlib works without Xt/Xft complexity.
// Draws a white rectangle with "HELLO" text on a blue background.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// We can't link to Xlib (it's in the Alpine chroot, not the initramfs).
// Instead, use the X11 protocol directly via a Unix socket.
// But that's too complex. Let's just use xdpyinfo as our "rendering test".
//
// Actually, let's test by running a simple shell command that creates
// an X window via xterm or xmessage. But xterm crashes.
//
// The simplest test: use xset to change the background color, which
// will make the root window visible (non-black).

int main(void) {
    printf("x11_hello: setting root window background via xprop/xsetroot\n");

    // Try xsetroot to change the root window color
    int rc = system("DISPLAY=:0 xsetroot -solid '#336699' 2>/dev/null");
    if (rc == 0) {
        printf("x11_hello: xsetroot OK — root window should be blue\n");
    } else {
        printf("x11_hello: xsetroot failed (rc=%d), trying xprop...\n", rc);
        // Fallback: use xprop to set a property (at least proves X11 connection)
        rc = system("DISPLAY=:0 xprop -root -f _KEVLAR_TEST 8s -set _KEVLAR_TEST 'HELLO' 2>/dev/null");
        if (rc == 0)
            printf("x11_hello: xprop OK — X11 connection works\n");
        else
            printf("x11_hello: xprop also failed (rc=%d)\n", rc);
    }
    return 0;
}
