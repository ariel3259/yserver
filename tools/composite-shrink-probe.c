/* composite-shrink-probe — does a SHRINKING redirected window get a correctly
 *                          sized backing pixmap?
 *
 * Raised by issue #143. Measured on hardware: a redirected window shrank
 * 1278 -> 1276 with border_width 2 and its composite backing pixmap stayed
 * 1282x708 — verbatim the PREVIOUS configure's outer extent (1278 + 2*2) —
 * where the correct answer is 1280x708 (1276 + 2*2). The visible consequence
 * was a 19-row band of alpha 0 in the backing: the client painted the new,
 * smaller window, and the columns/rows of the over-large pixmap that the new
 * geometry no longer covers were never written by anybody.
 *
 * Xorg reallocates whenever the BORDERED extent differs, in either direction —
 * compReallocPixmap(), ../xserver/composite/compalloc.c:698:
 *
 *     pix_w = w + (bw << 1);
 *     pix_h = h + (bw << 1);
 *     if (pix_w != pOld->drawable.width || pix_h != pOld->drawable.height) {
 *         pNew = compNewPixmap(pWin, pix_x, pix_y, pix_w, pix_h);
 *
 * — note `!=`, not `>`: a shrink reallocates exactly like a grow. The
 * replacement is then fully re-seeded from the parent with IncludeInferiors
 * (compNewPixmap, compalloc.c:539-605), so no part of it is left undefined.
 *
 * WHY THE BORDER IS NON-ZERO HERE. The arithmetic that goes wrong is on the
 * BORDERED extent, w + 2*bw. With border_width 0 the bordered extent equals
 * the window extent and a server that tracks the wrong one of the two looks
 * correct. 2 px is the measured case, so 2 px is what this uses. The backing
 * pixmap therefore includes the border ring: window pixel (0,0) is pixmap
 * pixel (bw,bw), and pixmap columns/rows [0,bw) and [extent-bw,extent) are
 * border, not client area.
 *
 * WHY RedirectWindow AND NOT RedirectSubwindows. RedirectSubwindows on the
 * root would redirect every other client's top-level as well, which is
 * antisocial on a shared server and makes the output depend on what else
 * happens to be running — it would destroy diffability. compAllocPixmap() and
 * compReallocPixmap() operate per-window either way, so redirecting our own
 * window exercises exactly the same code with nothing else in the frame.
 * MANUAL mode, because manual is what a real compositor uses and because it
 * stops the server from painting the window into its parent behind our back;
 * everything this probe reads comes out of the backing pixmap. Note that only
 * ONE manual redirect per window is allowed (compalloc.c), so if a compositor
 * has somehow already claimed it the redirect fails with BadAccess and the
 * probe says so rather than reporting nonsense.
 *
 * NO COMPOSITOR AND NO WINDOW MANAGER ARE NEEDED. The window is
 * override-redirect, so nothing reparents it and the client's own
 * XResizeWindow is the only configure it ever sees. Damage and XFixes are NOT
 * used: nothing here waits on a damage report, and every geometry fact is a
 * synchronous round trip.
 *
 * SIX PHASES, one window, in order; each is a resize followed by a fresh
 * XCompositeNameWindowPixmap and an XGetGeometry on it:
 *
 *   0 initial            400x300   expect backing 404x304
 *   1 shrink_w_2         398x300   expect 402x304   <- the measured case, -2 wide
 *   2 shrink_h_2         398x298   expect 402x302
 *   3 shrink_both_big    300x220   expect 304x224
 *   4 grow_both          420x320   expect 424x324   (control: growth is the
 *                                                    direction that works)
 *   5 shrink_after_grow  416x316   expect 420x320   (a shrink immediately
 *                                                    after a grow, which is
 *                                                    where "keep the previous
 *                                                    extent" is loudest)
 *
 * THE CORE ASSERTION, per phase, is
 *
 *     pixmap.width  == win.width  + 2*win.border
 *     pixmap.height == win.height + 2*win.border
 *
 * using the server's OWN reported window geometry, not what we asked for, so a
 * server that declined the resize is not also blamed for the pixmap.
 *
 * Each phase reads the backing pixmap TWICE, because the two readings answer
 * different questions:
 *
 *   pre  — straight after the resize, before the client repaints. Xorg re-seeds
 *          the replacement from the parent here, so `pre` says whether the
 *          backing was rebuilt or merely relabelled.
 *   post — after the client refills the WHOLE new window plus a marker
 *          sub-rect. This is the symptom: if the backing is larger than the
 *          window, the client cannot reach the surplus, and it shows up as a
 *          band that is neither paint, nor sub-rect, nor background, nor
 *          border.
 *
 * HOW "NEVER WRITTEN" IS DETERMINED, honestly. The window uses a depth-32
 * TrueColor (ARGB) visual, so XGetImage returns the alpha channel and all four
 * painted colours are fully opaque (alpha 0xff). A pixel of 0x00000000 is
 * therefore alpha 0 AND black, which is not any colour this probe or the
 * server's border/background code puts there. That is evidence of "nothing
 * wrote it", not proof: a server is free to explicitly clear a pixmap to
 * zero, and this cannot tell that apart from fresh untouched memory. The
 * counter is named `zero` for that reason, never `undefined`. If no depth-32
 * visual exists the probe falls back to the default depth, prints
 * probe.alpha_readable=0, and the `zero` class degrades to plain black — say
 * so when reading the output.
 *
 * OUTPUT is one `key=value` fact per line in a fixed order, with the same
 * number of lines whatever the server answers: run the SAME binary under
 * yserver and under stock Xorg/Xephyr and `diff` the two outputs. Sampled rows
 * and columns are taken at coordinates derived from the EXPECTED extent, so
 * they are identical on both servers and only the classification differs; a
 * sample that falls outside the pixmap the server actually handed us prints
 * -1 rather than changing the line count. XIDs vary run to run and live on
 * their own `.xid=` lines, out of the way of the lines worth diffing. Nothing
 * host-, pid-, time- or DISPLAY-dependent is printed.
 *
 * Every dereference is guarded and a non-fatal X error handler is installed,
 * so a server that rejects a request produces a line saying so rather than an
 * abort. The probe never dereferences an XImage it did not get.
 *
 * Build: gcc -O1 -Wall -Wextra -o /tmp/composite-shrink-probe \
 *            tools/composite-shrink-probe.c -lX11 -lXcomposite -lXext
 * Run:   DISPLAY=:7 /tmp/composite-shrink-probe > /tmp/yserver.txt
 *        DISPLAY=:0 /tmp/composite-shrink-probe > /tmp/xorg.txt
 *        diff -u /tmp/xorg.txt /tmp/yserver.txt
 *        (an explicit display may also be passed as argv[1])
 *
 * Exit: 0 = every phase's backing matches the bordered extent,
 *       2 = at least one phase disagrees (the bug),
 *       1 = the probe could not run at all (no display, no Composite, the
 *           window never mapped, the redirect was refused).
 */

/* nanosleep(), under -std=c11 as well as the default gnu dialect. */
#define _POSIX_C_SOURCE 200809L

#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/Xcomposite.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ---- constants -------------------------------------------------------- */

#define BORDER_WIDTH 2 /* the measured case; see the header comment */
#define ORIGIN_X 16
#define ORIGIN_Y 16

/* All four are fully opaque and mutually distinct, and none of them is 0. */
#define C_PAINT 0xff204080u  /* client fill of the whole window          */
#define C_SUB 0xff80c020u    /* client fill of a marker sub-rect         */
#define C_BG 0xffc02040u     /* window background_pixel                  */
#define C_BORDER 0xff008080u /* window border_pixel                      */

#define SUB_X 8
#define SUB_Y 8
#define SUB_W 40
#define SUB_H 40

/* The largest bordered extent any phase asks for is grow_both, 420x320 plus
 * two 2px borders, at (ORIGIN_X, ORIGIN_Y). A screen smaller than that would
 * clip the window's clipList, and while the BACKING pixmap's size does not
 * depend on the screen, an unviewable window is not a case this probe has
 * anything useful to say about. Guarded explicitly, the way
 * resize-expose-probe.c and border-damage-probe.c do, so a too-small server
 * exits 1 rather than printing confusing numbers. */
#define MAX_PHASE_W 420
#define MAX_PHASE_H 320
#define MIN_SCREEN_W (ORIGIN_X + MAX_PHASE_W + 2 * BORDER_WIDTH + 8)
#define MIN_SCREEN_H (ORIGIN_Y + MAX_PHASE_H + 2 * BORDER_WIDTH + 8)

#define NSAMPLE 12 /* sampled rows, and sampled columns, per scan */

#define CL_PAINT 0
#define CL_SUB 1
#define CL_BG 2
#define CL_BORDER 3
#define CL_ZERO 4
#define CL_OTHER 5
#define NCLASS 6

#define WAIT_MS 3000

struct phase {
    const char *name;
    int w, h;
};

static const struct phase PHASES[] = {
    { "initial", 400, 300 },
    { "shrink_w_2", 398, 300 },
    { "shrink_h_2", 398, 298 },
    { "shrink_both_big", 300, 220 },
    { "grow_both", 420, 320 },
    { "shrink_after_grow", 416, 316 },
};
#define NPHASE ((int) (sizeof PHASES / sizeof PHASES[0]))

/* ---- error handling --------------------------------------------------- */

static long g_err_total;
static int g_err_code;  /* last error_code   */
static int g_err_major; /* last request_code */
static int g_err_minor; /* last minor_code   */

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

/* Errors raised since the last call, after forcing a round trip. */
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

/* Wait up to `ms` for an event of `type` on `win`, discarding anything else.
 * Returns 1 if it arrived. Never blocks forever, so a server that simply does
 * not answer produces a "=0" line instead of a hung probe. */
static int wait_event(Display *dpy, int type, Window win, XEvent *out, int ms)
{
    int waited = 0;
    for (;;) {
        while (XEventsQueued(dpy, QueuedAfterFlush) > 0) {
            XEvent e;
            XNextEvent(dpy, &e);
            if (e.type != type)
                continue;
            if (type == MapNotify && e.xmap.window != win)
                continue;
            if (type == ConfigureNotify && e.xconfigure.window != win)
                continue;
            if (out)
                *out = e;
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

/* ---- pixel classification --------------------------------------------- */

static unsigned long g_pixmask = 0xffffffffu;

static int classify(unsigned long px)
{
    px &= g_pixmask;
    if (px == (C_PAINT & g_pixmask))
        return CL_PAINT;
    if (px == (C_SUB & g_pixmask))
        return CL_SUB;
    if (px == (C_BG & g_pixmask))
        return CL_BG;
    if (px == (C_BORDER & g_pixmask))
        return CL_BORDER;
    if (px == 0)
        return CL_ZERO;
    return CL_OTHER;
}

struct scan {
    int ok;
    int w, h; /* the region actually read = the whole pixmap  */
    long tot[NCLASS];
    int zrow_first, zrow_last, zrow_n;
    int zcol_first, zcol_last, zcol_n;
    int orow_first, orow_last; /* rows containing an `other` pixel */
    int rowy[NSAMPLE];
    int rowv[NSAMPLE][NCLASS]; /* -1 == that row is outside the pixmap */
    int colx[NSAMPLE];
    int colv[NSAMPLE][NCLASS];
};

/* Sample coordinates come from the EXPECTED extent, never the actual one, so
 * both servers print the same y/x values and only the counts can differ. */
static void sample_coords(int expect_w, int expect_h, int *xs, int *ys)
{
    for (int i = 0; i < NSAMPLE; i++) {
        int dw = expect_w > 1 ? expect_w - 1 : 0;
        int dh = expect_h > 1 ? expect_h - 1 : 0;
        xs[i] = (int) ((long) i * dw / (NSAMPLE - 1));
        ys[i] = (int) ((long) i * dh / (NSAMPLE - 1));
    }
}

static void scan_init(struct scan *s)
{
    memset(s, 0, sizeof *s);
    s->zrow_first = s->zrow_last = -1;
    s->zcol_first = s->zcol_last = -1;
    s->orow_first = s->orow_last = -1;
    for (int i = 0; i < NSAMPLE; i++)
        for (int c = 0; c < NCLASS; c++) {
            s->rowv[i][c] = -1;
            s->colv[i][c] = -1;
        }
}

/* Read the WHOLE pixmap (always a legal region, so XGetImage cannot be asked
 * for something out of bounds) and classify it. pix_w/pix_h are what the
 * server reported for this pixmap; expect_w/expect_h drive the sampling. */
static void scan_pixmap(Display *dpy, Pixmap pix, int pix_w, int pix_h,
                        int expect_w, int expect_h, struct scan *s)
{
    scan_init(s);
    sample_coords(expect_w, expect_h, s->colx, s->rowy);

    if (pix == None || pix_w <= 0 || pix_h <= 0)
        return;

    /* The region is the whole pixmap, so XGetImage is never asked for
     * anything out of bounds. It can still fail, and then s->ok stays 0. */
    XImage *img = XGetImage(dpy, pix, 0, 0, (unsigned) pix_w, (unsigned) pix_h,
                            AllPlanes, ZPixmap);
    XSync(dpy, False);
    if (!img || !img->data) {
        if (img)
            XDestroyImage(img);
        return;
    }

    /* The image may legitimately come back smaller than asked for; trust the
     * XImage's own dimensions and nothing else. */
    int iw = img->width, ih = img->height;
    if (iw > pix_w)
        iw = pix_w;
    if (ih > pix_h)
        ih = pix_h;
    if (iw <= 0 || ih <= 0) {
        XDestroyImage(img);
        return;
    }
    s->ok = 1;
    s->w = iw;
    s->h = ih;

    char *colhas = calloc((size_t) iw, 1);
    for (int y = 0; y < ih; y++) {
        int rowzero = 0, rowother = 0;
        for (int x = 0; x < iw; x++) {
            int c = classify(XGetPixel(img, x, y));
            s->tot[c]++;
            if (c == CL_ZERO) {
                rowzero = 1;
                if (colhas)
                    colhas[x] = 1;
            } else if (c == CL_OTHER) {
                rowother = 1;
            }
        }
        if (rowzero) {
            if (s->zrow_first < 0)
                s->zrow_first = y;
            s->zrow_last = y;
            s->zrow_n++;
        }
        if (rowother) {
            if (s->orow_first < 0)
                s->orow_first = y;
            s->orow_last = y;
        }
    }
    if (colhas) {
        for (int x = 0; x < iw; x++)
            if (colhas[x]) {
                if (s->zcol_first < 0)
                    s->zcol_first = x;
                s->zcol_last = x;
                s->zcol_n++;
            }
        free(colhas);
    }

    for (int i = 0; i < NSAMPLE; i++) {
        int y = s->rowy[i];
        if (y >= 0 && y < ih) {
            for (int c = 0; c < NCLASS; c++)
                s->rowv[i][c] = 0;
            for (int x = 0; x < iw; x++)
                s->rowv[i][classify(XGetPixel(img, x, y))]++;
        }
        int x = s->colx[i];
        if (x >= 0 && x < iw) {
            for (int c = 0; c < NCLASS; c++)
                s->colv[i][c] = 0;
            for (int yy = 0; yy < ih; yy++)
                s->colv[i][classify(XGetPixel(img, x, yy))]++;
        }
    }

    XDestroyImage(img);
}

static void print_scan(const char *pfx, const struct scan *s)
{
    printf("%s.ok=%d\n", pfx, s->ok);
    printf("%s.region.width=%d\n", pfx, s->ok ? s->w : -1);
    printf("%s.region.height=%d\n", pfx, s->ok ? s->h : -1);
    printf("%s.count.paint=%ld\n", pfx, s->ok ? s->tot[CL_PAINT] : -1);
    printf("%s.count.sub=%ld\n", pfx, s->ok ? s->tot[CL_SUB] : -1);
    printf("%s.count.bg=%ld\n", pfx, s->ok ? s->tot[CL_BG] : -1);
    printf("%s.count.border=%ld\n", pfx, s->ok ? s->tot[CL_BORDER] : -1);
    printf("%s.count.zero=%ld\n", pfx, s->ok ? s->tot[CL_ZERO] : -1);
    printf("%s.count.other=%ld\n", pfx, s->ok ? s->tot[CL_OTHER] : -1);
    printf("%s.zero.rows=%d\n", pfx, s->zrow_n);
    printf("%s.zero.row.first=%d\n", pfx, s->zrow_first);
    printf("%s.zero.row.last=%d\n", pfx, s->zrow_last);
    printf("%s.zero.cols=%d\n", pfx, s->zcol_n);
    printf("%s.zero.col.first=%d\n", pfx, s->zcol_first);
    printf("%s.zero.col.last=%d\n", pfx, s->zcol_last);
    printf("%s.other.row.first=%d\n", pfx, s->orow_first);
    printf("%s.other.row.last=%d\n", pfx, s->orow_last);
    for (int i = 0; i < NSAMPLE; i++)
        printf("%s.row.%02d=y:%d paint:%d sub:%d bg:%d border:%d zero:%d "
               "other:%d\n",
               pfx, i, s->rowy[i], s->rowv[i][CL_PAINT], s->rowv[i][CL_SUB],
               s->rowv[i][CL_BG], s->rowv[i][CL_BORDER], s->rowv[i][CL_ZERO],
               s->rowv[i][CL_OTHER]);
    for (int i = 0; i < NSAMPLE; i++)
        printf("%s.col.%02d=x:%d paint:%d sub:%d bg:%d border:%d zero:%d "
               "other:%d\n",
               pfx, i, s->colx[i], s->colv[i][CL_PAINT], s->colv[i][CL_SUB],
               s->colv[i][CL_BG], s->colv[i][CL_BORDER], s->colv[i][CL_ZERO],
               s->colv[i][CL_OTHER]);
}

/* Placeholders, so a phase that could not run has exactly the same shape as
 * one that did. */
static void print_scan_unavailable(const char *pfx, int expect_w, int expect_h)
{
    struct scan s;
    scan_init(&s);
    sample_coords(expect_w, expect_h, s.colx, s.rowy);
    print_scan(pfx, &s);
}

/* ---- main ------------------------------------------------------------- */

int main(int argc, char **argv)
{
    const char *display = NULL;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--display") == 0 && i + 1 < argc)
            display = argv[++i];
        else if (argv[i][0] != '-' && !display)
            display = argv[i];
        else {
            fprintf(stderr, "usage: %s [--display DPY | DPY]\n", argv[0]);
            return 1;
        }
    }

    printf("probe=composite-shrink-probe\n");
    printf("probe.format=1\n");
    printf("probe.issue=143\n");
    printf("probe.border_width=%d\n", BORDER_WIDTH);
    printf("probe.redirect_target=window\n");
    printf("probe.redirect_mode=manual\n");
    printf("probe.phases=%d\n", NPHASE);
    printf("probe.samples=%d\n", NSAMPLE);
    printf("probe.color.paint=0x%08x\n", C_PAINT);
    printf("probe.color.sub=0x%08x\n", C_SUB);
    printf("probe.color.bg=0x%08x\n", C_BG);
    printf("probe.color.border=0x%08x\n", C_BORDER);
    printf("probe.subrect=x:%d y:%d w:%d h:%d\n", SUB_X, SUB_Y, SUB_W, SUB_H);

    XSetErrorHandler(err_handler);
    XSetIOErrorHandler(io_handler);
    printf("probe.ioerror=0\n");

    Display *dpy = XOpenDisplay(display);
    if (!dpy) {
        printf("x.connect=fail\n");
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

    /* ---- Composite, required ------------------------------------------ */
    int cev = 0, cerr = 0;
    int have_comp = XCompositeQueryExtension(dpy, &cev, &cerr) ? 1 : 0;
    printf("composite.present=%d\n", have_comp);
    int cmaj = 0, cmin = 0;
    if (have_comp) {
        /* Both are pure out-parameters (Xcomposite.h:66); libXcomposite
         * sends its own compiled-in version and returns the negotiated one. */
        cmaj = -1;
        cmin = -1;
        if (!XCompositeQueryVersion(dpy, &cmaj, &cmin)) {
            cmaj = -1;
            cmin = -1;
        }
    } else {
        cmaj = -1;
        cmin = -1;
    }
    printf("composite.major=%d\n", cmaj);
    printf("composite.minor=%d\n", cmin);
    /* NameWindowPixmap arrived in Composite 0.2. */
    printf("composite.name_window_pixmap_supported=%d\n",
           (cmaj > 0 || (cmaj == 0 && cmin >= 2)) ? 1 : 0);
    if (!have_comp || cmaj < 0 || (cmaj == 0 && cmin < 2) || !big_enough) {
        printf("probe.result=unavailable\n");
        XCloseDisplay(dpy);
        return 1;
    }

    /* ---- the window --------------------------------------------------- */
    XVisualInfo vinfo;
    Visual *vis;
    int depth;
    int alpha_readable;
    if (XMatchVisualInfo(dpy, screen, 32, TrueColor, &vinfo)) {
        vis = vinfo.visual;
        depth = 32;
        alpha_readable = 1;
        g_pixmask = 0xffffffffu;
    } else {
        vis = DefaultVisual(dpy, screen);
        depth = DefaultDepth(dpy, screen);
        alpha_readable = 0;
        g_pixmask = 0x00ffffffu;
    }
    printf("probe.visual.depth=%d\n", depth);
    printf("probe.alpha_readable=%d\n", alpha_readable);

    XSetWindowAttributes attr;
    unsigned long mask = CWOverrideRedirect | CWEventMask | CWBackPixel |
                         CWBorderPixel | CWBitGravity;
    memset(&attr, 0, sizeof attr);
    attr.override_redirect = True; /* no WM required, and none may interfere */
    attr.event_mask = StructureNotifyMask | ExposureMask;
    attr.background_pixel = C_BG;
    attr.border_pixel = C_BORDER;
    /* ForgetGravity: the server is free to discard the old contents on a
     * resize, which is what leaves the question "what IS in the backing now?"
     * entirely to the server and makes the `pre` reading meaningful. */
    attr.bit_gravity = ForgetGravity;
    Colormap cmap = None;
    if (depth == 32) {
        cmap = XCreateColormap(dpy, root, vis, AllocNone);
        attr.colormap = cmap;
        mask |= CWColormap;
    }

    long emark = g_err_total;
    Window win = XCreateWindow(dpy, root, ORIGIN_X, ORIGIN_Y,
                               (unsigned) PHASES[0].w, (unsigned) PHASES[0].h,
                               BORDER_WIDTH, depth, InputOutput, vis, mask,
                               &attr);
    long e = errs_since(dpy, &emark);
    printf("win.create.errors=%ld\n", e);
    printf("win.xid=0x%08lx\n", (unsigned long) win);
    if (e || win == None) {
        printf("probe.result=unavailable\n");
        XCloseDisplay(dpy);
        return 1;
    }

    XMapWindow(dpy, win);
    XFlush(dpy);
    int mapped = wait_event(dpy, MapNotify, win, NULL, WAIT_MS);
    printf("win.mapnotify=%d\n", mapped);
    /* MapNotify is the server telling us the window is viewable, which is what
     * ProcCompositeNameWindowPixmap requires (compext.c:241). Confirm it with
     * an independent round trip rather than trusting the event alone. */
    Window rr;
    int gx = 0, gy = 0;
    unsigned gw = 0, gh = 0, gbw = 0, gdepth = 0;
    emark = g_err_total;
    Status gst = XGetGeometry(dpy, win, &rr, &gx, &gy, &gw, &gh, &gbw, &gdepth);
    (void) errs_since(dpy, &emark);
    printf("win.geometry.ok=%d\n", gst ? 1 : 0);
    printf("win.width=%u\n", gst ? gw : 0u);
    printf("win.height=%u\n", gst ? gh : 0u);
    printf("win.border=%u\n", gst ? gbw : 0u);
    printf("win.depth=%u\n", gst ? gdepth : 0u);
    XWindowAttributes wa;
    memset(&wa, 0, sizeof wa);
    emark = g_err_total;
    Status wst = XGetWindowAttributes(dpy, win, &wa);
    (void) errs_since(dpy, &emark);
    printf("win.attrs.ok=%d\n", wst ? 1 : 0);
    printf("win.map_state_viewable=%d\n",
           (wst && wa.map_state == IsViewable) ? 1 : 0);
    /* No WM is assumed. If one did reparent us, say so — it changes nothing
     * this probe measures, but it explains any geometry surprise. */
    Window rroot = None, rparent = None, *rkids = NULL;
    unsigned nkids = 0;
    int reparented = -1;
    emark = g_err_total;
    if (XQueryTree(dpy, win, &rroot, &rparent, &rkids, &nkids)) {
        reparented = (rparent != None && rparent != root) ? 1 : 0;
        if (rkids)
            XFree(rkids);
    }
    (void) errs_since(dpy, &emark);
    printf("win.reparented=%d\n", reparented);

    if (!(wst && wa.map_state == IsViewable)) {
        printf("probe.result=unavailable\n");
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    /* ---- redirect ------------------------------------------------------ */
    emark = g_err_total;
    XCompositeRedirectWindow(dpy, win, CompositeRedirectManual);
    e = errs_since(dpy, &emark);
    printf("redirect.errors=%ld\n", e);
    printf("redirect.last_error_code=%d\n", e ? g_err_code : 0);
    printf("redirect.ok=%d\n", e ? 0 : 1);
    if (e) {
        /* BadAccess (10) means something else already holds the manual
         * redirect — a compositor is running and this probe cannot speak. */
        printf("probe.result=unavailable\n");
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    GC gc = XCreateGC(dpy, win, 0, NULL);
    printf("gc.ok=%d\n", gc ? 1 : 0);
    if (!gc) {
        printf("probe.result=unavailable\n");
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    /* Paint before the first measurement, so phase 0's `pre` already has
     * client content in it and every later `pre` is comparable. */
    XSetForeground(dpy, gc, C_PAINT);
    XFillRectangle(dpy, win, gc, 0, 0, (unsigned) PHASES[0].w,
                   (unsigned) PHASES[0].h);
    XSetForeground(dpy, gc, C_SUB);
    XFillRectangle(dpy, win, gc, SUB_X, SUB_Y, SUB_W, SUB_H);
    drain(dpy);

    /* ---- the phases ---------------------------------------------------- */
    Pixmap prev_pix = None;
    int prev_w = -1, prev_h = -1;
    int failures = 0;

    for (int p = 0; p < NPHASE; p++) {
        char pfx[64];
        snprintf(pfx, sizeof pfx, "phase.%d", p);
        printf("%s.name=%s\n", pfx, PHASES[p].name);
        printf("%s.req.width=%d\n", pfx, PHASES[p].w);
        printf("%s.req.height=%d\n", pfx, PHASES[p].h);

        int configured = 1;
        if (p > 0) {
            drain(dpy);
            XResizeWindow(dpy, win, (unsigned) PHASES[p].w,
                          (unsigned) PHASES[p].h);
            XFlush(dpy);
            configured = wait_event(dpy, ConfigureNotify, win, NULL, WAIT_MS);
        }
        printf("%s.configurenotify=%d\n", pfx, configured);
        XSync(dpy, False);

        /* The server's own idea of the window, which is what the pixmap is
         * judged against. */
        emark = g_err_total;
        gw = gh = gbw = gdepth = 0;
        gst = XGetGeometry(dpy, win, &rr, &gx, &gy, &gw, &gh, &gbw, &gdepth);
        (void) errs_since(dpy, &emark);
        int win_w = gst ? (int) gw : -1;
        int win_h = gst ? (int) gh : -1;
        int win_bw = gst ? (int) gbw : -1;
        printf("%s.win.geometry.ok=%d\n", pfx, gst ? 1 : 0);
        printf("%s.win.width=%d\n", pfx, win_w);
        printf("%s.win.height=%d\n", pfx, win_h);
        printf("%s.win.border=%d\n", pfx, win_bw);
        printf("%s.win.depth=%d\n", pfx, gst ? (int) gdepth : -1);
        printf("%s.win.resize_honoured=%d\n", pfx,
               (win_w == PHASES[p].w && win_h == PHASES[p].h) ? 1 : 0);

        /* compalloc.c:695-698 — the bordered extent, on both axes. */
        int expect_w = (win_w >= 0 && win_bw >= 0) ? win_w + 2 * win_bw : -1;
        int expect_h = (win_h >= 0 && win_bw >= 0) ? win_h + 2 * win_bw : -1;
        printf("%s.expect.width=%d\n", pfx, expect_w);
        printf("%s.expect.height=%d\n", pfx, expect_h);

        /* Does the PREVIOUS phase's named pixmap still report its old size?
         * Xorg allocates a replacement and leaves the old object alone, so the
         * old XID keeps the old extent. A server that resizes the backing in
         * place instead shows the new extent here — a direct discriminator
         * between "reallocated" and "relabelled". */
        int prev_now_w = -1, prev_now_h = -1, prev_alive = 0;
        if (prev_pix != None) {
            emark = g_err_total;
            unsigned pw = 0, ph = 0, pbw = 0, pd = 0;
            Status pst =
                XGetGeometry(dpy, prev_pix, &rr, &gx, &gy, &pw, &ph, &pbw, &pd);
            long pe = errs_since(dpy, &emark);
            if (pst && !pe) {
                prev_alive = 1;
                prev_now_w = (int) pw;
                prev_now_h = (int) ph;
            }
        }
        printf("%s.prev.alive=%d\n", pfx, prev_alive);
        printf("%s.prev.was.width=%d\n", pfx, prev_w);
        printf("%s.prev.was.height=%d\n", pfx, prev_h);
        printf("%s.prev.now.width=%d\n", pfx, prev_now_w);
        printf("%s.prev.now.height=%d\n", pfx, prev_now_h);
        printf("%s.prev.mutated_in_place=%d\n", pfx,
               (prev_alive && prev_w > 0 &&
                (prev_now_w != prev_w || prev_now_h != prev_h))
                   ? 1
                   : 0);

        /* ---- the backing pixmap, freshly named ------------------------- */
        emark = g_err_total;
        Pixmap pix = XCompositeNameWindowPixmap(dpy, win);
        long ne = errs_since(dpy, &emark);
        printf("%s.name.errors=%ld\n", pfx, ne);
        printf("%s.name.last_error_code=%d\n", pfx, ne ? g_err_code : 0);
        printf("%s.pixmap.xid=0x%08lx\n", pfx, (unsigned long) pix);

        int pix_w = -1, pix_h = -1, pix_d = -1, pix_bw = -1, pix_ok = 0;
        if (pix != None && !ne) {
            emark = g_err_total;
            unsigned pw = 0, ph = 0, pbw = 0, pd = 0;
            Status pst =
                XGetGeometry(dpy, pix, &rr, &gx, &gy, &pw, &ph, &pbw, &pd);
            long pe = errs_since(dpy, &emark);
            if (pst && !pe) {
                pix_ok = 1;
                pix_w = (int) pw;
                pix_h = (int) ph;
                pix_bw = (int) pbw;
                pix_d = (int) pd;
            }
        }
        printf("%s.pixmap.ok=%d\n", pfx, pix_ok);
        printf("%s.pixmap.width=%d\n", pfx, pix_w);
        printf("%s.pixmap.height=%d\n", pfx, pix_h);
        printf("%s.pixmap.depth=%d\n", pfx, pix_d);
        /* Pixmaps have no border; printed so a server inventing one is loud. */
        printf("%s.pixmap.border=%d\n", pfx, pix_bw);

        /* ===== THE CORE ASSERTION ===================================== */
        int extent_ok =
            (pix_ok && expect_w > 0 && pix_w == expect_w && pix_h == expect_h)
                ? 1
                : 0;
        printf("%s.delta.width=%d\n", pfx,
               (pix_ok && expect_w > 0) ? pix_w - expect_w : 0);
        printf("%s.delta.height=%d\n", pfx,
               (pix_ok && expect_h > 0) ? pix_h - expect_h : 0);
        printf("%s.EXTENT_OK=%d\n", pfx, extent_ok);
        if (!extent_ok)
            failures++;

        /* `pre`: the backing as the server left it, before the client
         * repaints. */
        char sp[80];
        snprintf(sp, sizeof sp, "%s.pre", pfx);
        if (pix_ok) {
            struct scan s;
            scan_pixmap(dpy, pix, pix_w, pix_h, expect_w > 0 ? expect_w : 1,
                        expect_h > 0 ? expect_h : 1, &s);
            print_scan(sp, &s);
        } else {
            print_scan_unavailable(sp, expect_w > 0 ? expect_w : 1,
                                   expect_h > 0 ? expect_h : 1);
        }

        /* The client now repaints the WHOLE window it believes it has, plus
         * the marker sub-rect. Anything in the backing that this cannot reach
         * is the bug's visible half. */
        if (win_w > 0 && win_h > 0) {
            XSetForeground(dpy, gc, C_PAINT);
            XFillRectangle(dpy, win, gc, 0, 0, (unsigned) win_w,
                           (unsigned) win_h);
            XSetForeground(dpy, gc, C_SUB);
            XFillRectangle(dpy, win, gc, SUB_X, SUB_Y, SUB_W, SUB_H);
        }
        XSync(dpy, False);

        snprintf(sp, sizeof sp, "%s.post", pfx);
        if (pix_ok) {
            struct scan s;
            scan_pixmap(dpy, pix, pix_w, pix_h, expect_w > 0 ? expect_w : 1,
                        expect_h > 0 ? expect_h : 1, &s);
            print_scan(sp, &s);
        } else {
            print_scan_unavailable(sp, expect_w > 0 ? expect_w : 1,
                                   expect_h > 0 ? expect_h : 1);
        }

        if (prev_pix != None)
            XFreePixmap(dpy, prev_pix);
        prev_pix = pix;
        prev_w = pix_w;
        prev_h = pix_h;
        drain(dpy);
    }

    if (prev_pix != None)
        XFreePixmap(dpy, prev_pix);
    emark = g_err_total;
    XCompositeUnredirectWindow(dpy, win, CompositeRedirectManual);
    (void) errs_since(dpy, &emark);
    XFreeGC(dpy, gc);
    XDestroyWindow(dpy, win);
    if (cmap != None)
        XFreeColormap(dpy, cmap);
    XSync(dpy, False);
    XCloseDisplay(dpy);

    printf("result.phases_failed=%d\n", failures);
    printf("result.x_errors_total=%ld\n", g_err_total);
    printf("probe.result=%s\n", failures ? "BUG" : "ok");
    return failures ? 2 : 0;
}
