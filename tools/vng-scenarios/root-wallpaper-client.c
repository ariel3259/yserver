/*
 * Set a wallpaper the way a wallpaper setter does: paint a pattern into a
 * pixmap the size of the WHOLE root, install it as the root's background
 * pixmap, and clear the root. One drawable, one paint, spanning every output.
 *
 * That is the straddling paint under suspicion for the dual-head staleness
 * where one output shows the wallpaper and the other keeps the backdrop
 * (jos, 2026-09-09: "right screen HAD the wallpaper, left didn't, and mouse
 * smeared the wallpaper behind the grey root"). `gears.wallpaper` in awesome
 * takes this same route — background pixmap plus clear — which is why a plain
 * XFillRectangle on the root is not a faithful stand-in.
 *
 * The pattern is decodable per position, so a stale region can be told apart
 * from a wrongly-offset one: each 64x64 cell is
 * R = 40 + 24*(col%9), G = 40 + 24*(row%9), B = 200 - 20*((col+row)%9).
 * Nothing in it is the backdrop grey (0x505050) or black.
 *
 *     cc -O1 -o root-wallpaper-client root-wallpaper-client.c -lX11
 */

#include <X11/Xlib.h>
#include <stdio.h>
#include <unistd.h>

#define CELL 64

int main(void)
{
    Display *dpy = XOpenDisplay(NULL);
    if (!dpy) {
        fprintf(stderr, "root-wallpaper-client: cannot open display\n");
        return 1;
    }
    int screen = DefaultScreen(dpy);
    Window root = RootWindow(dpy, screen);
    unsigned depth = (unsigned) DefaultDepth(dpy, screen);

    XWindowAttributes wa;
    if (!XGetWindowAttributes(dpy, root, &wa)) {
        fprintf(stderr, "root-wallpaper-client: no root geometry\n");
        return 1;
    }
    printf("root %dx%d depth %u\n", wa.width, wa.height, depth);

    Pixmap wall = XCreatePixmap(dpy, root, (unsigned) wa.width,
                                (unsigned) wa.height, depth);
    GC gc = XCreateGC(dpy, wall, 0, NULL);
    for (int y = 0; y < wa.height; y += CELL) {
        for (int x = 0; x < wa.width; x += CELL) {
            int col = x / CELL, row = y / CELL;
            unsigned long r = (unsigned long) (40 + 24 * (col % 9));
            unsigned long g = (unsigned long) (40 + 24 * (row % 9));
            unsigned long b = (unsigned long) (200 - 20 * ((col + row) % 9));
            XSetForeground(dpy, gc, (r << 16) | (g << 8) | b);
            XFillRectangle(dpy, wall, gc, x, y, CELL, CELL);
        }
    }

    /* The wallpaper-setter route. */
    XSetWindowBackgroundPixmap(dpy, root, wall);
    XClearWindow(dpy, root);
    XSync(dpy, False);
    printf("root background pixmap installed and cleared\n");
    fflush(stdout);

    /* A wallpaper setter normally frees the pixmap and exits; the root keeps
     * the storage alive. Do the same, then stay connected so the scenario can
     * tell the client apart from a crash. */
    XFreePixmap(dpy, wall);
    XSync(dpy, False);
    for (;;)
        sleep(1);
}
