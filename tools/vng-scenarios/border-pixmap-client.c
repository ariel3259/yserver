/*
 * A purpose-written client for X11 tiled window borders (CWBorderPixmap).
 *
 * The awesome smoke cannot validate this path — awesome makes zero
 * `border-pixmap` uses — so the #133 design spec says outright that tiled
 * borders ship untested without a client like this one.
 *
 * The window is override-redirect and no WM runs in the scenario, so its
 * geometry is exactly what was asked for and every ring pixel has a
 * predictable value. The tile encodes its own coordinates: a 4x4 grid of
 * 16x16 cells, cell (col,row) coloured R = col*64+32, G = row*64+32, B = 128,
 * so a single sampled pixel says which tile cell it came from.
 *
 * The prediction the scanout is then checked against — the whole point of the
 * exercise — is the TILE PHASE. Xorg aligns a border tile to the window's
 * CONTENT origin, not to the outer corner of the border:
 *
 *     mi/miexpose.c:461   tile_x_off = pWin->drawable.x;
 *     dix/window.c:888    pWin->drawable.x = pParent->drawable.x + x + bw;
 *
 * and CreateWindow's x,y is the OUTER corner. So with x=100, y=100, bw=16 the
 * content origin is (116,116), a ring pixel at absolute (ax,ay) samples tile
 * ((ax-116) mod 64, (ay-116) mod 64), and the top-left ring pixel (100,100)
 * samples tile (48,48) — cell (3,3), i.e. #e0e080. Aligning to the outer
 * corner instead would put cell (0,0) there.
 *
 *     cc -O1 -o border-pixmap-client border-pixmap-client.c -lX11
 *     ./border-pixmap-client            # runs until killed
 */

#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#define WIN_X 100
#define WIN_Y 100
#define WIN_W 200
#define WIN_H 100
#define BORDER 16
#define TILE 64
#define CELL 16

/* A second window with a solid border pixel, for contrast in the same frame:
 * it pins that CWBorderPixel still works when CWBorderPixmap exists, and it
 * gives the pixel scan a known-solid ring to compare the tiled one against. */
#define WIN2_X 500
#define WIN2_Y 100
#define BORDER2_PIXEL 0x00ff00ff /* magenta; nothing else in the frame is */

int main(void)
{
    Display *dpy = XOpenDisplay(NULL);
    if (!dpy) {
        fprintf(stderr, "border-pixmap-client: cannot open display\n");
        return 1;
    }
    int screen = DefaultScreen(dpy);
    Window root = RootWindow(dpy, screen);
    unsigned depth = (unsigned) DefaultDepth(dpy, screen);

    Pixmap tile = XCreatePixmap(dpy, root, TILE, TILE, depth);
    GC gc = XCreateGC(dpy, tile, 0, NULL);
    for (int row = 0; row < TILE / CELL; row++) {
        for (int col = 0; col < TILE / CELL; col++) {
            unsigned long r = (unsigned long) (col * 64 + 32);
            unsigned long g = (unsigned long) (row * 64 + 32);
            XSetForeground(dpy, gc, (r << 16) | (g << 8) | 128u);
            XFillRectangle(dpy, tile, gc, col * CELL, row * CELL, CELL, CELL);
        }
    }

    XSetWindowAttributes attr;
    attr.background_pixel = 0x00000000; /* black content, so the ring stands out */
    attr.border_pixmap = tile;
    attr.override_redirect = True;
    attr.event_mask = ExposureMask;
    unsigned long mask = CWBackPixel | CWBorderPixmap | CWOverrideRedirect | CWEventMask;

    Window tiled = XCreateWindow(dpy, root, WIN_X, WIN_Y, WIN_W, WIN_H, BORDER,
                                 (int) depth, InputOutput,
                                 DefaultVisual(dpy, screen), mask, &attr);

    attr.border_pixel = BORDER2_PIXEL;
    mask = CWBackPixel | CWBorderPixel | CWOverrideRedirect | CWEventMask;
    Window solid = XCreateWindow(dpy, root, WIN2_X, WIN2_Y, WIN_W, WIN_H, BORDER,
                                 (int) depth, InputOutput,
                                 DefaultVisual(dpy, screen), mask, &attr);

    XMapWindow(dpy, tiled);
    XMapWindow(dpy, solid);
    XSync(dpy, False);

    /* Report what to check, so the scenario's expectations and the client's
     * geometry cannot drift apart. */
    printf("tiled outer=%d,%d %dx%d content=%d,%d %dx%d bw=%d tile=%dx%d cell=%d\n",
           WIN_X, WIN_Y, WIN_W + 2 * BORDER, WIN_H + 2 * BORDER,
           WIN_X + BORDER, WIN_Y + BORDER, WIN_W, WIN_H, BORDER, TILE, TILE, CELL);
    printf("solid outer=%d,%d %dx%d border_pixel=%06lx\n",
           WIN2_X, WIN2_Y, WIN_W + 2 * BORDER, WIN_H + 2 * BORDER,
           (unsigned long) (BORDER2_PIXEL & 0xffffffu));
    fflush(stdout);

    /* The border is server-painted; the client only has to stay alive and
     * keep answering, so the connection is not dropped before the capture. */
    for (;;) {
        while (XPending(dpy)) {
            XEvent ev;
            XNextEvent(dpy, &ev);
        }
        usleep(100000);
    }
}
