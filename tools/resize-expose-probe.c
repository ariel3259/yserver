/* resize-expose-probe — which Expose events does a resize produce, and what is
 *                       left in the pixels afterwards?
 *
 * ORIGINALLY raised by the wezterm ctrl-+ report (2026-09-15, e27 over XDMCP):
 * growing a depth-32 background-None window left the newly exposed bottom strip
 * fully transparent, so the shell prompt in it was invisible. That first
 * version measured the GROW case only and established two things, which this
 * version must not lose:
 *
 *   - for ForgetGravity our grow-Expose is RIGHT (the whole new window);
 *   - for NorthWestGravity our grow-Expose is WRONG — we send a full-window
 *     Expose where Xorg sends only the narrow new strip.
 *
 * EXTENDED (2026-09-17) for the #143 branch, which now fires the Expose gate on
 * ANY resize instead of only on a grow. That is one extra Expose per shrink,
 * and it is wire-visible, so it needs a sharper gate than an xts sweep.
 *
 * WHAT XORG DOES, from the source rather than from spec prose:
 *
 *   mi/miwindow.c, miResizeWindow(): while the window was viewable it does
 *
 *       RegionCopy(&pWin->valdata->after.exposed, &pWin->clipList);
 *
 *   UNCONDITIONALLY — there is no grow/shrink distinction anywhere in the
 *   function. The only thing that ever removes area from `after.exposed` is the
 *   bit-gravity subtraction further down,
 *
 *       if (g == pWin->bitGravity)
 *           RegionSubtract(&pWin->valdata->after.exposed,
 *                          &pWin->valdata->after.exposed, gravitate[g]);
 *
 *   and `gravitate[pWin->bitGravity]` is seeded from `oldWinClip`, which is
 *   only created at all when
 *
 *       if (pWin->bitGravity != ForgetGravity)
 *
 *   So under the DEFAULT ForgetGravity nothing is ever subtracted and the whole
 *   new window is exposed in BOTH directions. Under NorthWestGravity the old
 *   area is subtracted, so a grow exposes only the new strip and a shrink
 *   exposes nothing at all.
 *
 *   A pure MOVE does not go through miResizeWindow at all: dix routes it to
 *   miMoveWindow(), which copies the whole old borderClip to the new position
 *   and never touches `after.exposed`. An unobscured window therefore gets NO
 *   Expose from a move, whatever its bit gravity. That is this probe's control.
 *
 *   BACKGROUND None DOES NOT SUPPRESS THE EXPOSE. miHandleExposures() calls
 *
 *       pWin->drawable.pScreen->PaintWindow(pWin, prgn, PW_BACKGROUND);
 *       if (clientInterested)
 *           miSendExposures(pWin, exposures, ...);
 *
 *   and the `case None: return;` that skips the fill lives INSIDE miPaintWindow
 *   (mi/miexpose.c), after the exposures have already been decided. The window
 *   is not painted; the client is still told. This probe measures that rather
 *   than asserting it, because it has been got wrong from prose before.
 *
 * THE MATRIX. Five windows, each taken through three phases on its own, so the
 * axes the task cares about are all covered and the depth axis from the first
 * version survives:
 *
 *   W0  depth 32, background None,  Forget     <- wezterm's exact shape
 *   W1  depth 32, background PIXEL, Forget
 *   W2  default depth, background None, Forget <- the depth control
 *   W3  depth 32, background None,  NorthWest
 *   W4  depth 32, background PIXEL, NorthWest  <- added, completes the matrix
 *
 *   phase grow    300x200 -> 300x262   (+62 rows; the wezterm strip was
 *                                       818 -> 880). Height-only, so the
 *                                       NorthWest expectation is a single
 *                                       unambiguous strip and not an L.
 *   phase shrink  300x262 -> 260x170   (both axes, so "nothing" and "the whole
 *                                       new window" cannot be confused)
 *   phase move    +7,+5, no size change (control: expect no Expose)
 *
 * ONE WINDOW AT A TIME, at a fixed origin. The first version put four windows
 * side by side at x = 60 + i*340, which runs off the right edge of anything
 * smaller than about 1700 px: the clipList would then be clipped by the screen
 * and the Expose rectangles would depend on the screen size, which destroys the
 * whole point of diffing two servers. Sequential windows at (16,16) need only a
 * small screen and overlap nothing.
 *
 * NO REPAINT AFTER A RESIZE — deliberately, and inherited from the first
 * version. wezterm was observed reading the NEW geometry and then painting only
 * the OLD height, so what matters is what the server leaves on screen while the
 * client is behind. The window is painted once, before the grow; the
 * `pixel.new_strip` reading in the GROW phase is therefore the one that answers
 * the original question. The later phases' pixel readings are cumulative and
 * are reported for completeness, not as an assertion.
 *
 * The paint colour is fully opaque (alpha 0xff) so that a depth-32 readback of
 * 0x00000000 is unambiguously "alpha 0 and black" — the transparent-strip
 * symptom — and gets its own `zero` class. The first version painted with
 * alpha 0, which made the symptom and the paint indistinguishable.
 *
 * Every window is override-redirect and NO WINDOW MANAGER IS REQUIRED: the
 * client's own XResizeWindow/XMoveWindow is the only configure it ever sees. If
 * something did reparent us it is reported on its own line.
 *
 * OUTPUT is one `key=value` fact per line, in a fixed order, with the same
 * number of lines whatever the server answers: run the SAME binary under
 * yserver and under stock Xorg/Xephyr and `diff` the two outputs. Expectations
 * are printed as facts (`.expect.*`) next to what actually arrived, so a
 * disagreement is readable without trusting the probe's own verdict. XIDs live
 * on their own `.xid=` lines, out of the way of the lines worth diffing.
 * Nothing host-, pid-, time- or DISPLAY-dependent is printed.
 *
 * Every X call is guarded and a non-fatal error handler is installed, so a
 * server that rejects a request produces a line saying so rather than an abort.
 * No XImage is dereferenced that was not returned.
 *
 * Build: gcc -O1 -Wall -Wextra -o /tmp/resize-expose-probe \
 *            tools/resize-expose-probe.c -lX11
 * Run:   DISPLAY=:7 /tmp/resize-expose-probe > /tmp/yserver.txt
 *        DISPLAY=:9 /tmp/resize-expose-probe > /tmp/xorg.txt
 *        diff -u /tmp/xorg.txt /tmp/yserver.txt
 *        (an explicit display may also be passed as argv[1]; `--hold N` is
 *        accepted and ignored, for compatibility with older invocations)
 *        tools/vng-scenarios/resize-expose.sh runs this under vng and keeps
 *        the probe's stdout and exit status as its artifacts.
 *
 * Exit: 0 = every phase matched the expectation encoded above,
 *       2 = at least one phase disagreed,
 *       1 = the probe could not run at all (no display, no depth-32 visual,
 *           screen too small, the window never became viewable).
 */

/* nanosleep(), under -std=c11 as well as the default gnu dialect. */
#define _POSIX_C_SOURCE 200809L

#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ---- geometry --------------------------------------------------------- */

#define ORIGIN_X 16
#define ORIGIN_Y 16
#define BASE_W 300
#define BASE_H 200
#define GROW_DH 62 /* the wezterm strip was 818 -> 880 */
#define GROW_W BASE_W
#define GROW_H (BASE_H + GROW_DH)
#define SHRINK_W 260
#define SHRINK_H 170
#define MOVE_DX 7
#define MOVE_DY 5

/* Everything this probe draws must stay on screen, or the clipList is clipped
 * and the Expose rectangles stop being comparable between servers. */
#define MIN_SCREEN_W (ORIGIN_X + MOVE_DX + BASE_W + 8)
#define MIN_SCREEN_H (ORIGIN_Y + MOVE_DY + GROW_H + 8)

/* Both fully opaque and mutually distinct, and neither is 0. */
#define C_PAINT 0xffc02040u /* client fill of the whole window before growing */
#define C_BG 0xff20c040u    /* background_pixel of the bgPixel windows        */

#define PEEK_X 10
#define PEEK_Y 10
#define PEEK_STRIP_Y (BASE_H + GROW_DH / 2)

#define MAX_EXPOSE 8 /* rectangles reported per phase; the rest are counted */
#define SETTLE_MS 500
#define WAIT_MS 3000

/* ---- the matrix ------------------------------------------------------- */

struct spec {
    const char *name;
    int want32;   /* 1 = depth-32 TrueColor, 0 = the screen default */
    int bg_pixel; /* 1 = background_pixel, 0 = background None      */
    int gravity;  /* bit gravity, always set explicitly             */
};

/* W0..W3 are the first version's four windows, names unchanged so old output
 * still reads across. W4 is new and completes gravity x background. Note that
 * the first version left bit gravity unset on W0..W2; the X11 default IS
 * ForgetGravity, so setting it explicitly is the same window, only louder. */
static const struct spec SPECS[] = {
    { "d32-bgNone-forget", 1, 0, ForgetGravity }, /* wezterm's shape */
    { "d32-bgPixel-forget", 1, 1, ForgetGravity },
    { "d24-bgNone-forget", 0, 0, ForgetGravity }, /* the depth control */
    { "d32-bgNone-NW", 1, 0, NorthWestGravity },
    { "d32-bgPixel-NW", 1, 1, NorthWestGravity },
};
#define NWIN ((int) (sizeof SPECS / sizeof SPECS[0]))

#define OP_RESIZE 0
#define OP_MOVE 1

struct phase {
    const char *name;
    int op;
    int w, h; /* the size the window should have after the phase */
    int x, y; /* the position it should have after the phase     */
};

static const struct phase PHASES[] = {
    { "grow", OP_RESIZE, GROW_W, GROW_H, ORIGIN_X, ORIGIN_Y },
    { "shrink", OP_RESIZE, SHRINK_W, SHRINK_H, ORIGIN_X, ORIGIN_Y },
    { "move", OP_MOVE, SHRINK_W, SHRINK_H, ORIGIN_X + MOVE_DX,
      ORIGIN_Y + MOVE_DY },
};
#define NPHASE ((int) (sizeof PHASES / sizeof PHASES[0]))

/* What the window measured `base_w` x `base_h` before each phase. */
static void phase_prev_size(int p, int *pw, int *ph)
{
    if (p == 0) {
        *pw = BASE_W;
        *ph = BASE_H;
    } else {
        *pw = PHASES[p - 1].w;
        *ph = PHASES[p - 1].h;
    }
}

/* ---- expectation kinds ------------------------------------------------ */

#define EK_NONE 0  /* no Expose at all                     */
#define EK_FULL 1  /* the whole new window                 */
#define EK_STRIP 2 /* only the area the old window did not cover */

static const char *ek_name(int k)
{
    switch (k) {
    case EK_NONE:
        return "none";
    case EK_FULL:
        return "full_new_window";
    default:
        return "new_area_only";
    }
}

/* ---- error handling --------------------------------------------------- */

static long g_err_total;
static int g_err_code;
static int g_err_major;
static int g_err_minor;

static int err_handler(Display *dpy, XErrorEvent *e)
{
    (void) dpy;
    g_err_total++;
    if (e) {
        g_err_code = e->error_code;
        g_err_major = e->request_code;
        g_err_minor = e->minor_code;
    }
    return 0;
}

static int io_handler(Display *dpy)
{
    (void) dpy;
    printf("probe.ioerror_fatal=1\n");
    fflush(stdout);
    _Exit(1);
}

static long errs_since(Display *dpy, long *mark)
{
    long before = *mark;
    XSync(dpy, False);
    *mark = g_err_total;
    return g_err_total - before;
}

/* ---- bounded waiting -------------------------------------------------- */

static void nap_ms(int ms)
{
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (long) (ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

static int wait_map(Display *dpy, Window win, int ms)
{
    int waited = 0;
    for (;;) {
        while (XEventsQueued(dpy, QueuedAfterFlush) > 0) {
            XEvent e;
            XNextEvent(dpy, &e);
            if (e.type == MapNotify && e.xmap.window == win)
                return 1;
        }
        if (waited >= ms)
            return 0;
        nap_ms(20);
        waited += 20;
    }
}

static void drain(Display *dpy)
{
    XSync(dpy, False);
    while (XEventsQueued(dpy, QueuedAlready) > 0) {
        XEvent e;
        XNextEvent(dpy, &e);
    }
}

/* ---- event collection ------------------------------------------------- */

struct ecoll {
    int nexpose;
    int nconfigure;
    int nrec; /* rectangles actually recorded (<= MAX_EXPOSE) */
    int x[MAX_EXPOSE], y[MAX_EXPOSE], w[MAX_EXPOSE], h[MAX_EXPOSE];
    int count[MAX_EXPOSE];
    int have_union;
    int ux1, uy1, ux2, uy2;
    int first_before_configure;
    char order[33];
    int cfg_w, cfg_h, cfg_x, cfg_y;
};

static void ecoll_init(struct ecoll *c)
{
    memset(c, 0, sizeof *c);
    for (int i = 0; i < MAX_EXPOSE; i++) {
        c->x[i] = c->y[i] = c->w[i] = c->h[i] = c->count[i] = -1;
    }
    c->cfg_w = c->cfg_h = c->cfg_x = c->cfg_y = -1;
}

static void ecoll_push_char(struct ecoll *c, char ch)
{
    size_t n = strlen(c->order);
    if (n + 1 < sizeof c->order) {
        c->order[n] = ch;
        c->order[n + 1] = '\0';
    }
}

/* Read for a fixed budget, so the same wall-clock allowance is given to both
 * servers and a server that says nothing produces zeroes, not a hang. */
static void collect(Display *dpy, Window win, struct ecoll *c, int ms)
{
    int waited = 0;
    ecoll_init(c);
    for (;;) {
        while (XEventsQueued(dpy, QueuedAfterFlush) > 0) {
            XEvent e;
            XNextEvent(dpy, &e);
            if (e.type == Expose && e.xexpose.window == win) {
                int ex = e.xexpose.x, ey = e.xexpose.y;
                int ew = e.xexpose.width, eh = e.xexpose.height;
                if (c->nrec < MAX_EXPOSE) {
                    c->x[c->nrec] = ex;
                    c->y[c->nrec] = ey;
                    c->w[c->nrec] = ew;
                    c->h[c->nrec] = eh;
                    c->count[c->nrec] = e.xexpose.count;
                    c->nrec++;
                }
                if (!c->have_union) {
                    c->have_union = 1;
                    c->ux1 = ex;
                    c->uy1 = ey;
                    c->ux2 = ex + ew;
                    c->uy2 = ey + eh;
                } else {
                    if (ex < c->ux1)
                        c->ux1 = ex;
                    if (ey < c->uy1)
                        c->uy1 = ey;
                    if (ex + ew > c->ux2)
                        c->ux2 = ex + ew;
                    if (ey + eh > c->uy2)
                        c->uy2 = ey + eh;
                }
                if (c->nconfigure == 0)
                    c->first_before_configure = 1;
                ecoll_push_char(c, 'E');
                c->nexpose++;
            } else if (e.type == ConfigureNotify &&
                       e.xconfigure.window == win) {
                c->cfg_w = e.xconfigure.width;
                c->cfg_h = e.xconfigure.height;
                c->cfg_x = e.xconfigure.x;
                c->cfg_y = e.xconfigure.y;
                ecoll_push_char(c, 'C');
                c->nconfigure++;
            }
        }
        if (waited >= ms)
            return;
        nap_ms(20);
        waited += 20;
    }
}

/* ---- pixel classification --------------------------------------------- */

static unsigned long g_pixmask = 0xffffffffu;

#define CL_UNAVAILABLE 0
#define CL_PAINT 1
#define CL_BG 2
#define CL_ZERO 3
#define CL_OTHER 4

static const char *cl_name(int c)
{
    switch (c) {
    case CL_PAINT:
        return "paint";
    case CL_BG:
        return "bg";
    case CL_ZERO:
        return "zero";
    case CL_OTHER:
        return "other";
    default:
        return "unavailable";
    }
}

static int classify(unsigned long px)
{
    px &= g_pixmask;
    if (px == (C_PAINT & g_pixmask))
        return CL_PAINT;
    if (px == (C_BG & g_pixmask))
        return CL_BG;
    if (px == 0)
        return CL_ZERO;
    return CL_OTHER;
}

/* Read one pixel, but only if it is provably inside the window the server says
 * we have; an XGetImage outside the drawable is a BadMatch, not a measurement.
 * Returns 1 and sets *out on success. */
static int peek(Display *dpy, Window win, int x, int y, int win_w, int win_h,
                unsigned long *out)
{
    if (win_w <= 0 || win_h <= 0 || x < 0 || y < 0 || x >= win_w || y >= win_h)
        return 0;
    long mark = g_err_total;
    XImage *img = XGetImage(dpy, win, x, y, 1, 1, AllPlanes, ZPixmap);
    long e = errs_since(dpy, &mark);
    if (!img)
        return 0;
    if (e || !img->data || img->width < 1 || img->height < 1) {
        XDestroyImage(img);
        return 0;
    }
    *out = XGetPixel(img, 0, 0);
    XDestroyImage(img);
    return 1;
}

static void print_pixel(const char *key, int ok, unsigned long px)
{
    printf("%s.class=%s\n", key, ok ? cl_name(classify(px)) : "unavailable");
    if (ok)
        printf("%s.hex=0x%08lx\n", key, px & g_pixmask);
    else
        printf("%s.hex=unavailable\n", key);
}

/* ---- main ------------------------------------------------------------- */

int main(int argc, char **argv)
{
    const char *display = NULL;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--display") == 0 && i + 1 < argc)
            display = argv[++i];
        /* Accepted and ignored, for compatibility with older invocations:
         * the first version held the windows on screen after measuring. This
         * version takes each window down as soon as its three phases are
         * done, so there is nothing left to hold. */
        else if (strcmp(argv[i], "--hold") == 0 && i + 1 < argc)
            (void) argv[++i];
        else if (argv[i][0] != '-' && !display)
            display = argv[i];
        else {
            fprintf(stderr, "usage: %s [--display DPY | DPY] [--hold N]\n",
                    argv[0]);
            return 1;
        }
    }

    printf("probe=resize-expose-probe\n");
    printf("probe.format=2\n");
    printf("probe.issue=143\n");
    printf("probe.windows=%d\n", NWIN);
    printf("probe.phases=%d\n", NPHASE);
    printf("probe.max_expose_reported=%d\n", MAX_EXPOSE);
    printf("probe.origin.x=%d\n", ORIGIN_X);
    printf("probe.origin.y=%d\n", ORIGIN_Y);
    printf("probe.base.width=%d\n", BASE_W);
    printf("probe.base.height=%d\n", BASE_H);
    printf("probe.grow.width=%d\n", GROW_W);
    printf("probe.grow.height=%d\n", GROW_H);
    printf("probe.shrink.width=%d\n", SHRINK_W);
    printf("probe.shrink.height=%d\n", SHRINK_H);
    printf("probe.move.dx=%d\n", MOVE_DX);
    printf("probe.move.dy=%d\n", MOVE_DY);
    printf("probe.border_width=0\n");
    printf("probe.repaint_after_resize=0\n");
    printf("probe.color.paint=0x%08x\n", C_PAINT);
    printf("probe.color.bg=0x%08x\n", C_BG);
    printf("probe.peek.inside=x:%d y:%d\n", PEEK_X, PEEK_Y);
    printf("probe.peek.new_strip=x:%d y:%d\n", PEEK_X, PEEK_STRIP_Y);

    XSetErrorHandler(err_handler);
    XSetIOErrorHandler(io_handler);
    printf("probe.ioerror=0\n");

    Display *dpy = XOpenDisplay(display);
    if (!dpy) {
        printf("x.connect=fail\n");
        printf("probe.result=unavailable\n");
        return 1;
    }
    printf("x.connect=ok\n");

    int screen = DefaultScreen(dpy);
    Window root = RootWindow(dpy, screen);
    int sw = DisplayWidth(dpy, screen);
    int sh = DisplayHeight(dpy, screen);
    printf("x.screen.width=%d\n", sw);
    printf("x.screen.height=%d\n", sh);
    printf("x.screen.default_depth=%d\n", DefaultDepth(dpy, screen));
    printf("x.screen.min_required.width=%d\n", MIN_SCREEN_W);
    printf("x.screen.min_required.height=%d\n", MIN_SCREEN_H);
    int big_enough = (sw >= MIN_SCREEN_W && sh >= MIN_SCREEN_H) ? 1 : 0;
    printf("x.screen.big_enough=%d\n", big_enough);

    XVisualInfo vinfo;
    int have32 = XMatchVisualInfo(dpy, screen, 32, TrueColor, &vinfo) ? 1 : 0;
    printf("x.visual32.present=%d\n", have32);
    if (!have32 || !big_enough) {
        printf("probe.result=unavailable\n");
        XCloseDisplay(dpy);
        return 1;
    }
    Colormap cmap32 = XCreateColormap(dpy, root, vinfo.visual, AllocNone);

    int failures = 0;
    int unavailable = 0;

    for (int i = 0; i < NWIN; i++) {
        char wp[32];
        snprintf(wp, sizeof wp, "win.%d", i);
        printf("%s.name=%s\n", wp, SPECS[i].name);
        printf("%s.background=%s\n", wp, SPECS[i].bg_pixel ? "pixel" : "none");
        printf("%s.gravity=%s\n", wp,
               SPECS[i].gravity == ForgetGravity ? "forget" : "northwest");

        Visual *vis;
        int depth;
        if (SPECS[i].want32) {
            vis = vinfo.visual;
            depth = 32;
        } else {
            vis = DefaultVisual(dpy, screen);
            depth = DefaultDepth(dpy, screen);
        }
        g_pixmask = (depth == 32) ? 0xffffffffu : 0x00ffffffu;
        printf("%s.requested_depth=%d\n", wp, depth);
        printf("%s.alpha_readable=%d\n", wp, depth == 32 ? 1 : 0);

        XSetWindowAttributes attr;
        memset(&attr, 0, sizeof attr);
        unsigned long mask =
            CWOverrideRedirect | CWEventMask | CWBitGravity | CWBorderPixel;
        attr.override_redirect = True; /* no WM required, and none may meddle */
        attr.event_mask = ExposureMask | StructureNotifyMask;
        attr.bit_gravity = SPECS[i].gravity;
        attr.border_pixel = 0;
        if (SPECS[i].bg_pixel) {
            attr.background_pixel = C_BG;
            mask |= CWBackPixel;
        }
        if (depth == 32) {
            attr.colormap = cmap32;
            mask |= CWColormap;
        }

        long emark = g_err_total;
        Window win =
            XCreateWindow(dpy, root, ORIGIN_X, ORIGIN_Y, BASE_W, BASE_H, 0,
                          depth, InputOutput, vis, mask, &attr);
        long e = errs_since(dpy, &emark);
        printf("%s.create.errors=%ld\n", wp, e);
        printf("%s.xid=0x%08lx\n", wp, (unsigned long) win);

        int usable = (e == 0 && win != None) ? 1 : 0;
        int mapped = 0, viewable = 0, reparented = -1;
        if (usable) {
            XMapWindow(dpy, win);
            XFlush(dpy);
            mapped = wait_map(dpy, win, WAIT_MS);
            XWindowAttributes wa;
            memset(&wa, 0, sizeof wa);
            emark = g_err_total;
            Status wst = XGetWindowAttributes(dpy, win, &wa);
            (void) errs_since(dpy, &emark);
            viewable = (wst && wa.map_state == IsViewable) ? 1 : 0;
            Window rroot = None, rparent = None, *rkids = NULL;
            unsigned nkids = 0;
            emark = g_err_total;
            if (XQueryTree(dpy, win, &rroot, &rparent, &rkids, &nkids)) {
                reparented = (rparent != None && rparent != root) ? 1 : 0;
                if (rkids)
                    XFree(rkids);
            }
            (void) errs_since(dpy, &emark);
        }
        printf("%s.mapnotify=%d\n", wp, mapped);
        printf("%s.viewable=%d\n", wp, viewable);
        printf("%s.reparented=%d\n", wp, reparented);

        int win_w = -1, win_h = -1, win_bw = -1, win_depth = -1;
        if (usable && viewable) {
            Window rr;
            int gx = 0, gy = 0;
            unsigned gw = 0, gh = 0, gbw = 0, gd = 0;
            emark = g_err_total;
            Status gst =
                XGetGeometry(dpy, win, &rr, &gx, &gy, &gw, &gh, &gbw, &gd);
            (void) errs_since(dpy, &emark);
            if (gst) {
                win_w = (int) gw;
                win_h = (int) gh;
                win_bw = (int) gbw;
                win_depth = (int) gd;
            }
        }
        printf("%s.depth=%d\n", wp, win_depth);
        printf("%s.width=%d\n", wp, win_w);
        printf("%s.height=%d\n", wp, win_h);
        printf("%s.border=%d\n", wp, win_bw);

        int live = (usable && viewable && win_w > 0) ? 1 : 0;
        printf("%s.usable=%d\n", wp, live);
        if (!live)
            unavailable = 1;

        GC gc = None;
        if (live) {
            gc = XCreateGC(dpy, win, 0, NULL);
            if (gc) {
                /* One paint, before any resize; see the header. */
                XSetForeground(dpy, gc, C_PAINT);
                XFillRectangle(dpy, win, gc, 0, 0, BASE_W, BASE_H);
            }
        }
        printf("%s.gc.ok=%d\n", wp, gc ? 1 : 0);
        drain(dpy);

        for (int p = 0; p < NPHASE; p++) {
            char pp[64];
            snprintf(pp, sizeof pp, "%s.%s", wp, PHASES[p].name);
            int prev_w, prev_h;
            phase_prev_size(p, &prev_w, &prev_h);

            printf("%s.op=%s\n", pp,
                   PHASES[p].op == OP_MOVE ? "move" : "resize");
            printf("%s.prev.width=%d\n", pp, prev_w);
            printf("%s.prev.height=%d\n", pp, prev_h);
            printf("%s.req.width=%d\n", pp, PHASES[p].w);
            printf("%s.req.height=%d\n", pp, PHASES[p].h);
            printf("%s.req.x=%d\n", pp, PHASES[p].x);
            printf("%s.req.y=%d\n", pp, PHASES[p].y);

            /* The expectation, derived exactly as mi/miwindow.c derives it. */
            int ek, ex = 0, ey = 0, ew = 0, eh = 0;
            if (PHASES[p].op == OP_MOVE) {
                ek = EK_NONE;
            } else if (SPECS[i].gravity == ForgetGravity) {
                ek = EK_FULL;
                ex = 0;
                ey = 0;
                ew = PHASES[p].w;
                eh = PHASES[p].h;
            } else {
                /* new window minus the part the old window still covers */
                int kx = prev_w < PHASES[p].w ? prev_w : PHASES[p].w;
                int ky = prev_h < PHASES[p].h ? prev_h : PHASES[p].h;
                if (kx >= PHASES[p].w && ky >= PHASES[p].h) {
                    ek = EK_NONE;
                } else if (kx >= PHASES[p].w) {
                    ek = EK_STRIP;
                    ex = 0;
                    ey = ky;
                    ew = PHASES[p].w;
                    eh = PHASES[p].h - ky;
                } else if (ky >= PHASES[p].h) {
                    ek = EK_STRIP;
                    ex = kx;
                    ey = 0;
                    ew = PHASES[p].w - kx;
                    eh = PHASES[p].h;
                } else {
                    /* An L shape; only its bounding box is asserted. None of
                     * this probe's phases takes this path. */
                    ek = EK_STRIP;
                    ex = 0;
                    ey = 0;
                    ew = PHASES[p].w;
                    eh = PHASES[p].h;
                }
            }
            printf("%s.expect.kind=%s\n", pp, ek_name(ek));
            if (ek == EK_NONE)
                printf("%s.expect.union=none\n", pp);
            else
                printf("%s.expect.union=x:%d y:%d w:%d h:%d\n", pp, ex, ey, ew,
                       eh);
            printf("%s.expect.n_min=%d\n", pp, ek == EK_NONE ? 0 : 1);

            struct ecoll c;
            ecoll_init(&c);
            if (live) {
                drain(dpy);
                if (PHASES[p].op == OP_MOVE)
                    XMoveWindow(dpy, win, PHASES[p].x, PHASES[p].y);
                else
                    XResizeWindow(dpy, win, (unsigned) PHASES[p].w,
                                  (unsigned) PHASES[p].h);
                XFlush(dpy);
                collect(dpy, win, &c, SETTLE_MS);
            }

            /* The server's own idea of the window after the phase. */
            int gw2 = -1, gh2 = -1, gx2 = -1, gy2 = -1;
            if (live) {
                Window rr;
                int gx = 0, gy = 0;
                unsigned uw = 0, uh = 0, ubw = 0, ud = 0;
                emark = g_err_total;
                Status gst =
                    XGetGeometry(dpy, win, &rr, &gx, &gy, &uw, &uh, &ubw, &ud);
                (void) errs_since(dpy, &emark);
                if (gst) {
                    gw2 = (int) uw;
                    gh2 = (int) uh;
                    gx2 = gx;
                    gy2 = gy;
                }
            }
            printf("%s.win.width=%d\n", pp, gw2);
            printf("%s.win.height=%d\n", pp, gh2);
            printf("%s.win.x=%d\n", pp, gx2);
            printf("%s.win.y=%d\n", pp, gy2);
            printf("%s.win.honoured=%d\n", pp,
                   (gw2 == PHASES[p].w && gh2 == PHASES[p].h && gx2 == PHASES[p].x &&
                    gy2 == PHASES[p].y)
                       ? 1
                       : 0);

            printf("%s.configurenotify.n=%d\n", pp, c.nconfigure);
            printf("%s.configurenotify.width=%d\n", pp, c.cfg_w);
            printf("%s.configurenotify.height=%d\n", pp, c.cfg_h);
            printf("%s.configurenotify.x=%d\n", pp, c.cfg_x);
            printf("%s.configurenotify.y=%d\n", pp, c.cfg_y);

            printf("%s.expose.n=%d\n", pp, c.nexpose);
            printf("%s.expose.reported=%d\n", pp, c.nrec);
            printf("%s.expose.truncated=%d\n", pp,
                   c.nexpose > c.nrec ? 1 : 0);
            printf("%s.expose.first_before_configure=%d\n", pp,
                   c.first_before_configure);
            printf("%s.event_order=%s\n", pp, c.order[0] ? c.order : "-");
            if (c.have_union)
                printf("%s.expose.union=x:%d y:%d w:%d h:%d\n", pp, c.ux1,
                       c.uy1, c.ux2 - c.ux1, c.uy2 - c.uy1);
            else
                printf("%s.expose.union=none\n", pp);
            for (int k = 0; k < MAX_EXPOSE; k++) {
                if (k < c.nrec)
                    printf("%s.expose.%02d=x:%d y:%d w:%d h:%d count:%d\n", pp,
                           k, c.x[k], c.y[k], c.w[k], c.h[k], c.count[k]);
                else
                    printf("%s.expose.%02d=none\n", pp, k);
            }

            /* Does the union of what arrived equal the expectation? */
            int match;
            if (!live) {
                match = 0;
            } else if (ek == EK_NONE) {
                match = (c.nexpose == 0) ? 1 : 0;
            } else {
                match = (c.nexpose >= 1 && c.have_union && c.ux1 == ex &&
                         c.uy1 == ey && c.ux2 - c.ux1 == ew &&
                         c.uy2 - c.uy1 == eh)
                            ? 1
                            : 0;
            }
            printf("%s.MATCH=%d\n", pp, match);
            if (!match)
                failures++;

            /* The pixels the server left behind. Only the GROW phase's
             * new_strip reading answers the original wezterm question; the
             * window is never repainted, so later phases are cumulative. */
            unsigned long px = 0;
            int ok = live ? peek(dpy, win, PEEK_X, PEEK_Y, gw2, gh2, &px) : 0;
            char kbuf[96];
            snprintf(kbuf, sizeof kbuf, "%s.pixel.inside", pp);
            print_pixel(kbuf, ok, px);
            px = 0;
            ok = (live && p == 0)
                     ? peek(dpy, win, PEEK_X, PEEK_STRIP_Y, gw2, gh2, &px)
                     : 0;
            snprintf(kbuf, sizeof kbuf, "%s.pixel.new_strip", pp);
            print_pixel(kbuf, ok, px);
        }

        if (gc)
            XFreeGC(dpy, gc);
        if (usable && win != None)
            XDestroyWindow(dpy, win);
        drain(dpy);
    }

    XFreeColormap(dpy, cmap32);
    XSync(dpy, False);
    XCloseDisplay(dpy);

    printf("result.phases_checked=%d\n", NWIN * NPHASE);
    printf("result.phases_failed=%d\n", failures);
    printf("result.x_errors_total=%ld\n", g_err_total);
    printf("result.last_error.code=%d\n", g_err_code);
    printf("result.last_error.major=%d\n", g_err_major);
    printf("result.last_error.minor=%d\n", g_err_minor);
    if (unavailable) {
        printf("probe.result=unavailable\n");
        return 1;
    }
    printf("probe.result=%s\n", failures ? "MISMATCH" : "ok");
    return failures ? 2 : 0;
}
