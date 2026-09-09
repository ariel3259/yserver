/*
 * Issue #135 discriminator: is a RENDER Composite whose SOURCE is a Picture on
 * the root with subwindow-mode=IncludeInferiors able to see the root's
 * children?
 *
 * maim's whole capture is one such Composite (traced 2026-09-09), and it comes
 * back as bare backdrop while `import`, which does a plain XGetImage on the
 * root, is pixel-exact. `GetImage` on the root is special-cased to read the
 * composited scanout (backend.rs:21552); the composite SOURCE path never
 * consults the picture's `subwindow_mode`, which is stored but only ever read
 * for a DESTINATION clip decision (`dst_picture_clip_by_children`).
 *
 * Deliberately NO XCompositeRedirectSubwindows — maim calls it first, and this
 * client exists to show whether the source gap depends on that at all.
 *
 * Writes two PPMs from the SAME instant:
 *   composite.ppm - via CreatePicture(root, IncludeInferiors) -> Composite -> GetImage
 *   getimage.ppm  - via XGetImage(root) directly, the route that works
 * A visible child window painted CHILD_COLOUR is the thing both should show.
 *
 *     cc -O1 -o render-root-source-client render-root-source-client.c -lX11 -lXrender
 */

#include <X11/Xlib.h>
#include <X11/extensions/Xrender.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#define CHILD_X 200
#define CHILD_Y 150
#define CHILD_W 300
#define CHILD_H 200
#define CHILD_COLOUR 0x00ff00ff /* magenta; nothing else on screen is */

static void write_ppm(const char *path, XImage *img)
{
    FILE *f = fopen(path, "wb");
    if (!f) {
        fprintf(stderr, "cannot write %s\n", path);
        return;
    }
    fprintf(f, "P6\n%d %d\n255\n", img->width, img->height);
    for (int y = 0; y < img->height; y++) {
        for (int x = 0; x < img->width; x++) {
            unsigned long p = XGetPixel(img, x, y);
            fputc((int) ((p >> 16) & 0xff), f);
            fputc((int) ((p >> 8) & 0xff), f);
            fputc((int) (p & 0xff), f);
        }
    }
    fclose(f);
}

int main(void)
{
    /* Retry: the harness waits for the socket to EXIST, and yserver binds it
     * a moment before its core loop starts accepting, so a client launched
     * immediately can lose that race (observed 2026-09-09 — this client
     * failed while an xwininfo six seconds later succeeded). */
    Display *dpy = NULL;
    for (int attempt = 0; attempt < 100 && !dpy; attempt++) {
        dpy = XOpenDisplay(NULL);
        if (!dpy)
            usleep(100000);
    }
    if (!dpy) {
        fprintf(stderr, "cannot open display\n");
        return 1;
    }
    int screen = DefaultScreen(dpy);
    Window root = RootWindow(dpy, screen);
    Visual *visual = DefaultVisual(dpy, screen);
    unsigned depth = (unsigned) DefaultDepth(dpy, screen);

    int major = 0, minor = 0;
    if (!XRenderQueryVersion(dpy, &major, &minor)) {
        fprintf(stderr, "no RENDER\n");
        return 1;
    }
    printf("RENDER %d.%d\n", major, minor);

    XWindowAttributes wa;
    XGetWindowAttributes(dpy, root, &wa);
    int W = wa.width, H = wa.height;
    printf("root %dx%d depth %u\n", W, H, depth);

    /* A visible child of the root — the thing IncludeInferiors must include. */
    XSetWindowAttributes swa;
    swa.background_pixel = CHILD_COLOUR;
    swa.override_redirect = True;
    Window child = XCreateWindow(dpy, root, CHILD_X, CHILD_Y, CHILD_W, CHILD_H, 0,
                                 (int) depth, InputOutput, visual,
                                 CWBackPixel | CWOverrideRedirect, &swa);
    XMapWindow(dpy, child);
    XSync(dpy, False);
    sleep(2); /* let the server compose it */
    printf("child %dx%d+%d+%d colour %06x\n", CHILD_W, CHILD_H, CHILD_X, CHILD_Y,
           CHILD_COLOUR & 0xffffffu);

    /* The route maim takes, minus the redirect. */
    XRenderPictureAttributes pa;
    pa.subwindow_mode = IncludeInferiors;
    Picture src = XRenderCreatePicture(dpy, root, XRenderFindVisualFormat(dpy, visual),
                                       CPSubwindowMode, &pa);
    Pixmap pix = XCreatePixmap(dpy, root, (unsigned) W, (unsigned) H, 32);
    Picture dst = XRenderCreatePicture(
        dpy, pix, XRenderFindStandardFormat(dpy, PictStandardARGB32), 0, NULL);
    XRenderColor clear = {0, 0, 0, 0};
    XRectangle whole = {0, 0, (unsigned short) W, (unsigned short) H};
    XRenderFillRectangles(dpy, PictOpSrc, dst, &clear, &whole, 1);
    XRenderComposite(dpy, PictOpSrc, src, None, dst, 0, 0, 0, 0, 0, 0,
                     (unsigned) W, (unsigned) H);
    XSync(dpy, False);

    XImage *via_composite = XGetImage(dpy, pix, 0, 0, (unsigned) W, (unsigned) H,
                                      AllPlanes, ZPixmap);
    if (via_composite) {
        write_ppm("composite.ppm", via_composite);
        XDestroyImage(via_composite);
    }

    /* The route `import` takes, as an in-run control. */
    XImage *via_getimage =
        XGetImage(dpy, root, 0, 0, (unsigned) W, (unsigned) H, AllPlanes, ZPixmap);
    if (via_getimage) {
        write_ppm("getimage.ppm", via_getimage);
        XDestroyImage(via_getimage);
    }
    printf("wrote composite.ppm and getimage.ppm\n");
    fflush(stdout);
    XSync(dpy, False);
    for (;;)
        sleep(1);
}
