// Minimal repro for the "fullscreen override-redirect window not composited"
// bug (cinnamon-screensaver lock not covering under muffin on yserver).
//
// Creates ONE fullscreen, override-redirect, solid-red window, maps it,
// raises it, and holds for ~20s. Under a working compositor it must cover
// the whole screen. If the desktop shows through, the bug reproduces with a
// single known xid (printed to stdout) — no screensaver nondeterminism.
//
// Build: cc tools/or-fullscreen-repro.c -o /tmp/or-repro -lX11
// Run  : DISPLAY=:7 /tmp/or-repro
#include <X11/Xlib.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    Display *d = XOpenDisplay(NULL);
    if (!d) { fprintf(stderr, "cannot open display\n"); return 1; }
    int s = DefaultScreen(d);
    Window root = RootWindow(d, s);
    int w = DisplayWidth(d, s), h = DisplayHeight(d, s);

    XSetWindowAttributes a;
    a.override_redirect = True;                 // bypass the WM, like a locker
    a.background_pixel = 0xFFFF0000;            // opaque red
    a.event_mask = ExposureMask;
    Window win = XCreateWindow(
        d, root, 0, 0, w, h, 0,
        CopyFromParent, InputOutput, CopyFromParent,
        CWOverrideRedirect | CWBackPixel | CWEventMask, &a);

    printf("repro window = 0x%lx  (%dx%d, override-redirect, opaque red)\n",
           win, w, h);
    fflush(stdout);

    XMapRaised(d, win);
    XFlush(d);

    // Repaint red on every expose, hold ~20s.
    GC gc = XCreateGC(d, win, 0, NULL);
    XSetForeground(d, gc, 0xFF0000);
    for (int i = 0; i < 200; i++) {
        while (XPending(d)) {
            XEvent e; XNextEvent(d, &e);
            if (e.type == Expose) { XFillRectangle(d, win, gc, 0, 0, w, h); }
        }
        XFillRectangle(d, win, gc, 0, 0, w, h);
        XFlush(d);
        usleep(100000);
    }
    XDestroyWindow(d, win);
    XCloseDisplay(d);
    return 0;
}
