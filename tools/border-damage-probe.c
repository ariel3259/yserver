/* border-damage-probe — does the BORDER RING of a redirected window turn into
 *                       protocol DamageNotify, and where?
 *
 * Raised by the #143 branch, which now reports the border annulus
 * (Xorg's `borderClip - winSize`) as protocol damage. That is up to four extra
 * DamageNotify events per resize of a bordered redirected window, it is
 * wire-visible, and an xts sweep is too blunt and too non-deterministic to gate
 * it. This measures it directly and deterministically instead.
 *
 * WHAT XORG DOES, from the source:
 *
 *   composite/compalloc.c, compReallocPixmap(): the backing pixmap is
 *   reallocated whenever the BORDERED extent changes, in either direction
 *   (`pix_w = w + (bw << 1); ... if (pix_w != pOld->drawable.width || ...)`),
 *   and the replacement is installed with compSetPixmap(pWin, pNew, bw).
 *
 *   composite/compwindow.c, compSetPixmapVisitWindow(): whenever that happens
 *   with a non-zero border width it queues
 *
 *       QueueWorkProc(compRepaintBorder, serverClient, ...)
 *
 *   and compRepaintBorder() paints exactly
 *
 *       RegionSubtract(&exposed, &pWindow->borderClip, &pWindow->winSize);
 *       pWindow->drawable.pScreen->PaintWindow(pWindow, &exposed, PW_BORDER);
 *
 *   i.e. the annulus, and nothing else. Note that it is a WORK PROC: it runs at
 *   the next dispatch idle, not synchronously inside the resize, so the probe
 *   waits a fixed budget rather than expecting the events with the reply.
 *
 *   dix/window.c, ChangeWindowAttributes(): the same annulus is repainted when
 *   the border CONTENTS bit is present —
 *
 *       if (((vmaskCopy & (CWBorderPixel | CWBorderPixmap)) || borderRelative)
 *           && pWin->viewable && HasBorder(pWin))
 *
 *   — on the BIT being present, not on the value having changed. Setting the
 *   border pixel to the value it already had still repaints.
 *
 *   composite/compwindow.c, compCopyWindow(): a MOVE of a redirected window
 *   also damages the annulus, and this probe's phase 5 originally got that
 *   wrong. mi/miwindow.c, miMoveWindow() hands CopyWindow the window's OLD
 *   `borderClip` — which includes the border — as prgnSrc. For a redirected
 *   window whose backing pixmap travels with it, `cw->pOldPixmap` is NULL and
 *   the pixmap origin shift cancels the window origin shift, so the
 *   `ptOldOrg.x != pWin->drawable.x` test fails and compCopyWindow takes the
 *   else branch:
 *
 *       DamageDamageRegion(&pWin->drawable, prgnSrc);
 *
 *   i.e. the WHOLE bordered extent, border included, is reported as damage —
 *   not "nothing", which is what a naive reading of compReallocPixmap's early
 *   return suggests. MEASURED, not inferred: Xephyr 21.1.24 answers phase 5
 *   with a single box `x:-4 y:-4 w:328 h:238`, annulus.covered=4464/4464,
 *   FULLY_COVERED=1, inside also fully covered. A move is therefore an
 *   EX_ANNULUS_FULL phase, not a control.
 *
 *   miext/damage/damage.c, damageRegionAppend(): the damage region is clipped
 *   against the window's `borderClip`, which INCLUDES the border, and is then
 *   translated by `-pDrawable->x / -pDrawable->y`, where drawable->x is the
 *   INSIDE origin. So the annulus comes out at NEGATIVE coordinates, and
 *   damageext/damageext.c copies those box coordinates into `area` verbatim.
 *   For w x h with border bw the four strips are
 *
 *       top    (-bw, -bw, w + 2bw, bw)
 *       left   (-bw,   0, bw,      h )
 *       right  (  w,   0, bw,      h )
 *       bottom (-bw,   h, w + 2bw, bw)
 *
 *   which is exactly how a y-banded region decomposes that shape. `area.x` and
 *   `area.y` are signed shorts; this probe prints them as signed ints, because
 *   the negative origin is the whole point and an unsigned print would lose it.
 *
 * NO COMPOSITOR AND NO WINDOW MANAGER ARE REQUIRED. The window is
 * override-redirect, so nothing reparents it and the client's own requests are
 * the only configures it ever sees.
 *
 * WHY RedirectWindow AND NOT RedirectSubwindows. RedirectSubwindows on the root
 * would redirect every other client's top-level as well, so the output would
 * depend on whatever else happens to be running — that destroys diffability.
 * Only ONE manual redirect per window is allowed, so if a compositor already
 * holds it the redirect fails with BadAccess (error code 10) and the probe says
 * so and exits 1 rather than printing nonsense.
 *
 * SIX PHASES, one window, in order:
 *
 *   0 paint         the client fills the whole window   (control: damage works
 *                                                        at all, and the border
 *                                                        is NOT touched)
 *   1 shrink        300x200 -> 260x170
 *   2 grow          260x170 -> 320x230
 *   3 border_pixel  XSetWindowBorder to a new pixel
 *   4 border_width  XSetWindowBorderWidth 2 -> 4
 *   5 move          +7,+5, nothing else  (the bordered extent does not change,
 *                                         so no border REPAINT is queued — but
 *                                         compCopyWindow still damages the old
 *                                         borderClip wholesale, so the ring
 *                                         must come out fully covered anyway.
 *                                         See compCopyWindow above; this is
 *                                         where the probe used to expect
 *                                         `no_annulus` and was simply wrong.)
 *
 * Border width 2 is the measured case from #143. Nothing is painted in phases
 * 1..5: every rectangle reported there is the server's own doing.
 *
 * HOW COVERAGE IS DECIDED. Every reported rectangle is rasterised, clamped, into
 * a (w + 2bw) x (h + 2bw) grid whose origin is (-bw, -bw), using the geometry
 * the SERVER reports after the phase — never what was asked for, so a server
 * that declined an operation is not also blamed for the damage. The probe then
 * reports, per phase, how many cells of each of the four strips were covered,
 * how many of the four canonical strip rectangles arrived VERBATIM, and how
 * much of the client area was covered. `exact_rects` is the sharp
 * discriminator: a server that reports one big bounding box covers the annulus
 * without ever emitting the four strips.
 *
 * OUTPUT is one `key=value` fact per line, in a fixed order, with the same
 * number of lines whatever the server answers: run the SAME binary under
 * yserver and under stock Xorg/Xephyr and `diff` the two outputs. Expectations
 * are printed as facts (`.expect.*`, `.annulus.expect.*`) next to what actually
 * arrived. XIDs live on their own `.xid=` lines. Nothing host-, pid-, time- or
 * DISPLAY-dependent is printed.
 *
 * Every X call is guarded and a non-fatal error handler is installed, so a
 * server that rejects a request produces a line saying so rather than an abort.
 * Nothing is dereferenced that was not returned.
 *
 * Build: gcc -O1 -Wall -Wextra -o /tmp/border-damage-probe \
 *            tools/border-damage-probe.c -lX11 -lXcomposite -lXdamage -lXfixes
 * Run:   DISPLAY=:7 /tmp/border-damage-probe > /tmp/yserver.txt
 *        DISPLAY=:9 /tmp/border-damage-probe > /tmp/xorg.txt
 *        diff -u /tmp/xorg.txt /tmp/yserver.txt
 *        (an explicit display may also be passed as argv[1])
 *
 * Exit: 0 = every phase matched the expectation encoded above,
 *       2 = at least one phase disagreed,
 *       1 = the probe could not run at all (no display, no Composite/Damage/
 *           XFixes, the window never became viewable, the redirect was
 *           refused).
 */

/* nanosleep(), under -std=c11 as well as the default gnu dialect. */
#define _POSIX_C_SOURCE 200809L

#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/Xcomposite.h>
#include <X11/extensions/Xdamage.h>
#include <X11/extensions/Xfixes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* ---- constants -------------------------------------------------------- */

#define BORDER_WIDTH 2  /* the measured #143 case */
#define BORDER_WIDTH2 4 /* what phase 4 changes it to */
#define ORIGIN_X 16
#define ORIGIN_Y 16
#define BASE_W 300
#define BASE_H 200
#define SHRINK_W 260
#define SHRINK_H 170
#define GROW_W 320
#define GROW_H 230
#define MOVE_DX 7
#define MOVE_DY 5

#define MIN_SCREEN_W (ORIGIN_X + MOVE_DX + GROW_W + 2 * BORDER_WIDTH2 + 8)
#define MIN_SCREEN_H (ORIGIN_Y + MOVE_DY + GROW_H + 2 * BORDER_WIDTH2 + 8)

#define C_PAINT 0x00204080u
#define C_BG 0x00c02040u
#define C_BORDER 0x00008080u
#define C_BORDER2 0x00ff00ffu

#define MAX_DAMAGE 24 /* rectangles reported per phase; the rest are counted */
#define SETTLE_MS 700 /* compRepaintBorder is a WorkProc, so allow for idle */
#define WAIT_MS 3000

/* phase operations */
#define OP_PAINT 0
#define OP_RESIZE 1
#define OP_BORDER_PIXEL 2
#define OP_BORDER_WIDTH 3
#define OP_MOVE 4

/* what a phase is expected to do to the annulus */
/* EX_NO_ANNULUS is currently unused by any phase — the move phase used to
 * claim it and the oracle refuted that. It is kept because it is the right
 * expectation for any phase that genuinely must not touch the ring, and
 * because dropping it would silently renumber the others. */
#define EX_NO_ANNULUS 0   /* no reported rectangle may touch the ring   */
#define EX_ANNULUS_FULL 1 /* every cell of the ring must be covered     */
#define EX_INSIDE_FULL 2  /* every cell of the client area, ring free   */

struct phase {
    const char *name;
    int op;
    int w, h;   /* size after the phase   */
    int bw;     /* border width after it  */
    int x, y;   /* position after it      */
    int expect; /* EX_*                   */
};

static const struct phase PHASES[] = {
    { "paint", OP_PAINT, BASE_W, BASE_H, BORDER_WIDTH, ORIGIN_X, ORIGIN_Y,
      EX_INSIDE_FULL },
    { "shrink", OP_RESIZE, SHRINK_W, SHRINK_H, BORDER_WIDTH, ORIGIN_X, ORIGIN_Y,
      EX_ANNULUS_FULL },
    { "grow", OP_RESIZE, GROW_W, GROW_H, BORDER_WIDTH, ORIGIN_X, ORIGIN_Y,
      EX_ANNULUS_FULL },
    { "border_pixel", OP_BORDER_PIXEL, GROW_W, GROW_H, BORDER_WIDTH, ORIGIN_X,
      ORIGIN_Y, EX_ANNULUS_FULL },
    { "border_width", OP_BORDER_WIDTH, GROW_W, GROW_H, BORDER_WIDTH2, ORIGIN_X,
      ORIGIN_Y, EX_ANNULUS_FULL },
    /* Not a "no damage" control: compCopyWindow damages the whole old
     * borderClip on a move, ring included. Measured on Xephyr 21.1.24. */
    { "move", OP_MOVE, GROW_W, GROW_H, BORDER_WIDTH2, ORIGIN_X + MOVE_DX,
      ORIGIN_Y + MOVE_DY, EX_ANNULUS_FULL },
};
#define NPHASE ((int) (sizeof PHASES / sizeof PHASES[0]))

static const char *ex_name(int e)
{
    switch (e) {
    case EX_NO_ANNULUS:
        return "no_annulus";
    case EX_ANNULUS_FULL:
        return "annulus_fully_covered";
    default:
        return "client_area_fully_covered";
    }
}

static const char *op_name(int o)
{
    switch (o) {
    case OP_PAINT:
        return "paint";
    case OP_RESIZE:
        return "resize";
    case OP_BORDER_PIXEL:
        return "border_pixel";
    case OP_BORDER_WIDTH:
        return "border_width";
    default:
        return "move";
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

/* ---- damage collection ------------------------------------------------ */

struct rect {
    int x, y, w, h;
};

struct dcoll {
    int n;    /* DamageNotify events seen for our drawable */
    int nrec; /* rectangles recorded (<= MAX_DAMAGE)       */
    struct rect r[MAX_DAMAGE];
    int more[MAX_DAMAGE];
    int level[MAX_DAMAGE];
    int have_union;
    int ux1, uy1, ux2, uy2;
    int negative_origin;
    int geom_w, geom_h;
    int nconfigure;
    int cfg_w, cfg_h, cfg_x, cfg_y;
};

static void dcoll_init(struct dcoll *c)
{
    memset(c, 0, sizeof *c);
    for (int i = 0; i < MAX_DAMAGE; i++) {
        c->r[i].x = c->r[i].y = c->r[i].w = c->r[i].h = -1;
        c->more[i] = -1;
        c->level[i] = -1;
    }
    c->geom_w = c->geom_h = -1;
    c->cfg_w = c->cfg_h = c->cfg_x = c->cfg_y = -1;
}

/* Read for a fixed budget, so both servers get the same wall-clock allowance
 * and a server that says nothing produces zeroes rather than a hang. */
static void collect(Display *dpy, Window win, int dmg_base, struct dcoll *c,
                    int ms)
{
    int waited = 0;
    dcoll_init(c);
    for (;;) {
        while (XEventsQueued(dpy, QueuedAfterFlush) > 0) {
            XEvent e;
            XNextEvent(dpy, &e);
            if (e.type == dmg_base + XDamageNotify) {
                XDamageNotifyEvent *d = (XDamageNotifyEvent *) &e;
                if (d->drawable != win)
                    continue;
                int rx = (int) d->area.x;
                int ry = (int) d->area.y;
                int rw = (int) d->area.width;
                int rh = (int) d->area.height;
                if (c->nrec < MAX_DAMAGE) {
                    c->r[c->nrec].x = rx;
                    c->r[c->nrec].y = ry;
                    c->r[c->nrec].w = rw;
                    c->r[c->nrec].h = rh;
                    c->more[c->nrec] = d->more ? 1 : 0;
                    c->level[c->nrec] = d->level;
                    c->nrec++;
                }
                if (rx < 0 || ry < 0)
                    c->negative_origin++;
                if (!c->have_union) {
                    c->have_union = 1;
                    c->ux1 = rx;
                    c->uy1 = ry;
                    c->ux2 = rx + rw;
                    c->uy2 = ry + rh;
                } else {
                    if (rx < c->ux1)
                        c->ux1 = rx;
                    if (ry < c->uy1)
                        c->uy1 = ry;
                    if (rx + rw > c->ux2)
                        c->ux2 = rx + rw;
                    if (ry + rh > c->uy2)
                        c->uy2 = ry + rh;
                }
                c->geom_w = (int) d->geometry.width;
                c->geom_h = (int) d->geometry.height;
                c->n++;
            } else if (e.type == ConfigureNotify &&
                       e.xconfigure.window == win) {
                c->cfg_w = e.xconfigure.width;
                c->cfg_h = e.xconfigure.height;
                c->cfg_x = e.xconfigure.x;
                c->cfg_y = e.xconfigure.y;
                c->nconfigure++;
            }
        }
        if (waited >= ms)
            return;
        nap_ms(20);
        waited += 20;
    }
}

/* ---- coverage --------------------------------------------------------- */

/* A grid over the whole bordered extent, origin (-bw, -bw). */
struct grid {
    int ok;
    int bw, w, h;   /* the window's own size, and its border width */
    int gw, gh;     /* grid dimensions = w + 2bw, h + 2bw          */
    unsigned char *cell;
};

static void grid_free(struct grid *g)
{
    if (g->cell)
        free(g->cell);
    g->cell = NULL;
    g->ok = 0;
}

static int grid_init(struct grid *g, int w, int h, int bw)
{
    memset(g, 0, sizeof *g);
    if (w <= 0 || h <= 0 || bw < 0)
        return 0;
    g->w = w;
    g->h = h;
    g->bw = bw;
    g->gw = w + 2 * bw;
    g->gh = h + 2 * bw;
    if (g->gw <= 0 || g->gh <= 0)
        return 0;
    g->cell = calloc((size_t) g->gw * (size_t) g->gh, 1);
    if (!g->cell)
        return 0;
    g->ok = 1;
    return 1;
}

/* Mark a reported rectangle, clamped to the grid. Anything outside the
 * bordered extent is silently dropped here and counted separately. */
static void grid_mark(struct grid *g, const struct rect *r)
{
    if (!g->ok || r->w <= 0 || r->h <= 0)
        return;
    int x1 = r->x + g->bw, y1 = r->y + g->bw;
    int x2 = x1 + r->w, y2 = y1 + r->h;
    if (x1 < 0)
        x1 = 0;
    if (y1 < 0)
        y1 = 0;
    if (x2 > g->gw)
        x2 = g->gw;
    if (y2 > g->gh)
        y2 = g->gh;
    for (int y = y1; y < y2; y++)
        for (int x = x1; x < x2; x++)
            g->cell[(size_t) y * (size_t) g->gw + (size_t) x] = 1;
}

/* Count covered cells of a rectangle given in REPORTED coordinates. */
static void grid_count(const struct grid *g, const struct rect *r, long *cells,
                       long *covered)
{
    *cells = 0;
    *covered = 0;
    if (!g->ok || r->w <= 0 || r->h <= 0)
        return;
    int x1 = r->x + g->bw, y1 = r->y + g->bw;
    int x2 = x1 + r->w, y2 = y1 + r->h;
    if (x1 < 0)
        x1 = 0;
    if (y1 < 0)
        y1 = 0;
    if (x2 > g->gw)
        x2 = g->gw;
    if (y2 > g->gh)
        y2 = g->gh;
    for (int y = y1; y < y2; y++)
        for (int x = x1; x < x2; x++) {
            (*cells)++;
            if (g->cell[(size_t) y * (size_t) g->gw + (size_t) x])
                (*covered)++;
        }
}

static void annulus_strips(int w, int h, int bw, struct rect out[4])
{
    out[0].x = -bw;
    out[0].y = -bw;
    out[0].w = w + 2 * bw;
    out[0].h = bw; /* top    */
    out[1].x = -bw;
    out[1].y = 0;
    out[1].w = bw;
    out[1].h = h; /* left   */
    out[2].x = w;
    out[2].y = 0;
    out[2].w = bw;
    out[2].h = h; /* right  */
    out[3].x = -bw;
    out[3].y = h;
    out[3].w = w + 2 * bw;
    out[3].h = bw; /* bottom */
}

static int rect_eq(const struct rect *a, const struct rect *b)
{
    return a->x == b->x && a->y == b->y && a->w == b->w && a->h == b->h;
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

    printf("probe=border-damage-probe\n");
    printf("probe.format=1\n");
    printf("probe.issue=143\n");
    printf("probe.border_width=%d\n", BORDER_WIDTH);
    printf("probe.border_width2=%d\n", BORDER_WIDTH2);
    printf("probe.redirect_target=window\n");
    printf("probe.redirect_mode=manual\n");
    printf("probe.damage_level=raw_rectangles\n");
    printf("probe.phases=%d\n", NPHASE);
    printf("probe.max_damage_reported=%d\n", MAX_DAMAGE);
    printf("probe.origin.x=%d\n", ORIGIN_X);
    printf("probe.origin.y=%d\n", ORIGIN_Y);
    printf("probe.base.width=%d\n", BASE_W);
    printf("probe.base.height=%d\n", BASE_H);
    printf("probe.shrink.width=%d\n", SHRINK_W);
    printf("probe.shrink.height=%d\n", SHRINK_H);
    printf("probe.grow.width=%d\n", GROW_W);
    printf("probe.grow.height=%d\n", GROW_H);
    printf("probe.move.dx=%d\n", MOVE_DX);
    printf("probe.move.dy=%d\n", MOVE_DY);
    printf("probe.color.paint=0x%08x\n", C_PAINT);
    printf("probe.color.bg=0x%08x\n", C_BG);
    printf("probe.color.border=0x%08x\n", C_BORDER);
    printf("probe.color.border2=0x%08x\n", C_BORDER2);

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

    /* ---- the three extensions, all required --------------------------- */
    int cev = 0, cerr = 0;
    int have_comp = XCompositeQueryExtension(dpy, &cev, &cerr) ? 1 : 0;
    int cmaj = -1, cmin = -1;
    if (have_comp && !XCompositeQueryVersion(dpy, &cmaj, &cmin)) {
        cmaj = -1;
        cmin = -1;
    }
    printf("composite.present=%d\n", have_comp);
    printf("composite.major=%d\n", cmaj);
    printf("composite.minor=%d\n", cmin);

    int dev = 0, derr = 0;
    int have_dmg = XDamageQueryExtension(dpy, &dev, &derr) ? 1 : 0;
    int dmaj = -1, dmin = -1;
    if (have_dmg && !XDamageQueryVersion(dpy, &dmaj, &dmin)) {
        dmaj = -1;
        dmin = -1;
    }
    printf("damage.present=%d\n", have_dmg);
    printf("damage.major=%d\n", dmaj);
    printf("damage.minor=%d\n", dmin);

    int fev = 0, ferr = 0;
    int have_fix = XFixesQueryExtension(dpy, &fev, &ferr) ? 1 : 0;
    int fmaj = -1, fmin = -1;
    if (have_fix && !XFixesQueryVersion(dpy, &fmaj, &fmin)) {
        fmaj = -1;
        fmin = -1;
    }
    printf("xfixes.present=%d\n", have_fix);
    printf("xfixes.major=%d\n", fmaj);
    printf("xfixes.minor=%d\n", fmin);

    if (!have_comp || !have_dmg || !have_fix || !big_enough) {
        printf("probe.result=unavailable\n");
        XCloseDisplay(dpy);
        return 1;
    }

    /* ---- the window --------------------------------------------------- */
    XSetWindowAttributes attr;
    memset(&attr, 0, sizeof attr);
    unsigned long mask = CWOverrideRedirect | CWEventMask | CWBackPixel |
                         CWBorderPixel | CWBitGravity;
    attr.override_redirect = True; /* no WM required, and none may meddle */
    attr.event_mask = StructureNotifyMask | ExposureMask;
    attr.background_pixel = C_BG;
    attr.border_pixel = C_BORDER;
    attr.bit_gravity = ForgetGravity;

    long emark = g_err_total;
    Window win = XCreateWindow(dpy, root, ORIGIN_X, ORIGIN_Y, BASE_W, BASE_H,
                               BORDER_WIDTH, DefaultDepth(dpy, screen),
                               InputOutput, DefaultVisual(dpy, screen), mask,
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
    int mapped = wait_map(dpy, win, WAIT_MS);
    printf("win.mapnotify=%d\n", mapped);

    XWindowAttributes wa;
    memset(&wa, 0, sizeof wa);
    emark = g_err_total;
    Status wst = XGetWindowAttributes(dpy, win, &wa);
    (void) errs_since(dpy, &emark);
    printf("win.attrs.ok=%d\n", wst ? 1 : 0);
    printf("win.map_state_viewable=%d\n",
           (wst && wa.map_state == IsViewable) ? 1 : 0);

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
        /* BadAccess (10) means a compositor already holds the manual
         * redirect on this window and the probe cannot speak. */
        printf("probe.result=unavailable\n");
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    /* ---- damage, raw so that every rectangle is visible ---------------- */
    emark = g_err_total;
    Damage dmg = XDamageCreate(dpy, win, XDamageReportRawRectangles);
    e = errs_since(dpy, &emark);
    printf("damage.create.errors=%ld\n", e);
    printf("damage.xid=0x%08lx\n", (unsigned long) dmg);
    printf("damage.create.ok=%d\n", (e == 0 && dmg != None) ? 1 : 0);
    if (e || dmg == None) {
        printf("probe.result=unavailable\n");
        XCompositeUnredirectWindow(dpy, win, CompositeRedirectManual);
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    GC gc = XCreateGC(dpy, win, 0, NULL);
    printf("gc.ok=%d\n", gc ? 1 : 0);
    if (!gc) {
        printf("probe.result=unavailable\n");
        XDamageDestroy(dpy, dmg);
        XCompositeUnredirectWindow(dpy, win, CompositeRedirectManual);
        XDestroyWindow(dpy, win);
        XCloseDisplay(dpy);
        return 1;
    }

    int failures = 0;

    for (int p = 0; p < NPHASE; p++) {
        char pp[32];
        snprintf(pp, sizeof pp, "phase.%d", p);
        printf("%s.name=%s\n", pp, PHASES[p].name);
        printf("%s.op=%s\n", pp, op_name(PHASES[p].op));
        printf("%s.req.width=%d\n", pp, PHASES[p].w);
        printf("%s.req.height=%d\n", pp, PHASES[p].h);
        printf("%s.req.border=%d\n", pp, PHASES[p].bw);
        printf("%s.req.x=%d\n", pp, PHASES[p].x);
        printf("%s.req.y=%d\n", pp, PHASES[p].y);
        printf("%s.expect=%s\n", pp, ex_name(PHASES[p].expect));

        drain(dpy);

        struct dcoll c;
        switch (PHASES[p].op) {
        case OP_PAINT:
            XSetForeground(dpy, gc, C_PAINT);
            XFillRectangle(dpy, win, gc, 0, 0, (unsigned) PHASES[p].w,
                           (unsigned) PHASES[p].h);
            break;
        case OP_RESIZE:
            XResizeWindow(dpy, win, (unsigned) PHASES[p].w,
                          (unsigned) PHASES[p].h);
            break;
        case OP_BORDER_PIXEL:
            XSetWindowBorder(dpy, win, C_BORDER2);
            break;
        case OP_BORDER_WIDTH:
            XSetWindowBorderWidth(dpy, win, (unsigned) PHASES[p].bw);
            break;
        default:
            XMoveWindow(dpy, win, PHASES[p].x, PHASES[p].y);
            break;
        }
        XFlush(dpy);
        collect(dpy, win, dev, &c, SETTLE_MS);

        /* The server's own idea of the window, which is what the annulus is
         * computed from — never what was asked for. */
        Window rr;
        int gx = 0, gy = 0;
        unsigned uw = 0, uh = 0, ubw = 0, ud = 0;
        emark = g_err_total;
        Status gst = XGetGeometry(dpy, win, &rr, &gx, &gy, &uw, &uh, &ubw, &ud);
        (void) errs_since(dpy, &emark);
        int win_w = gst ? (int) uw : -1;
        int win_h = gst ? (int) uh : -1;
        int win_bw = gst ? (int) ubw : -1;
        printf("%s.win.geometry.ok=%d\n", pp, gst ? 1 : 0);
        printf("%s.win.width=%d\n", pp, win_w);
        printf("%s.win.height=%d\n", pp, win_h);
        printf("%s.win.border=%d\n", pp, win_bw);
        printf("%s.win.x=%d\n", pp, gst ? gx : -1);
        printf("%s.win.y=%d\n", pp, gst ? gy : -1);
        printf("%s.win.honoured=%d\n", pp,
               (win_w == PHASES[p].w && win_h == PHASES[p].h &&
                win_bw == PHASES[p].bw && gst && gx == PHASES[p].x &&
                gy == PHASES[p].y)
                   ? 1
                   : 0);

        printf("%s.configurenotify.n=%d\n", pp, c.nconfigure);
        printf("%s.configurenotify.width=%d\n", pp, c.cfg_w);
        printf("%s.configurenotify.height=%d\n", pp, c.cfg_h);
        printf("%s.configurenotify.x=%d\n", pp, c.cfg_x);
        printf("%s.configurenotify.y=%d\n", pp, c.cfg_y);

        printf("%s.damage.n=%d\n", pp, c.n);
        printf("%s.damage.reported=%d\n", pp, c.nrec);
        printf("%s.damage.truncated=%d\n", pp, c.n > c.nrec ? 1 : 0);
        printf("%s.damage.negative_origin_rects=%d\n", pp, c.negative_origin);
        printf("%s.damage.geom.width=%d\n", pp, c.geom_w);
        printf("%s.damage.geom.height=%d\n", pp, c.geom_h);
        if (c.have_union)
            printf("%s.damage.union=x:%d y:%d w:%d h:%d\n", pp, c.ux1, c.uy1,
                   c.ux2 - c.ux1, c.uy2 - c.uy1);
        else
            printf("%s.damage.union=none\n", pp);
        for (int k = 0; k < MAX_DAMAGE; k++) {
            if (k < c.nrec)
                printf("%s.damage.%02d=x:%d y:%d w:%d h:%d level:%d more:%d\n",
                       pp, k, c.r[k].x, c.r[k].y, c.r[k].w, c.r[k].h,
                       c.level[k], c.more[k]);
            else
                printf("%s.damage.%02d=none\n", pp, k);
        }

        /* ---- coverage against the annulus at the CURRENT geometry ------ */
        struct rect strips[4];
        static const char *STRIP_NAME[4] = { "top", "left", "right", "bottom" };
        int have_geom = (win_w > 0 && win_h > 0 && win_bw >= 0) ? 1 : 0;
        if (have_geom)
            annulus_strips(win_w, win_h, win_bw, strips);
        else
            memset(strips, 0, sizeof strips);
        for (int s = 0; s < 4; s++) {
            if (have_geom)
                printf("%s.annulus.expect.%s=x:%d y:%d w:%d h:%d\n", pp,
                       STRIP_NAME[s], strips[s].x, strips[s].y, strips[s].w,
                       strips[s].h);
            else
                printf("%s.annulus.expect.%s=unavailable\n", pp, STRIP_NAME[s]);
        }

        struct grid g;
        int grid_ok = have_geom ? grid_init(&g, win_w, win_h, win_bw) : 0;
        if (!grid_ok)
            memset(&g, 0, sizeof g);
        if (grid_ok)
            for (int k = 0; k < c.nrec; k++)
                grid_mark(&g, &c.r[k]);
        printf("%s.grid.ok=%d\n", pp, grid_ok);
        printf("%s.grid.width=%d\n", pp, grid_ok ? g.gw : -1);
        printf("%s.grid.height=%d\n", pp, grid_ok ? g.gh : -1);

        long a_cells = 0, a_cov = 0;
        int exact = 0;
        for (int s = 0; s < 4; s++) {
            long cells = 0, cov = 0;
            if (grid_ok)
                grid_count(&g, &strips[s], &cells, &cov);
            printf("%s.annulus.strip.%s.cells=%ld\n", pp, STRIP_NAME[s], cells);
            printf("%s.annulus.strip.%s.covered=%ld\n", pp, STRIP_NAME[s], cov);
            printf("%s.annulus.strip.%s.full=%d\n", pp, STRIP_NAME[s],
                   (cells > 0 && cov == cells) ? 1 : 0);
            int seen = 0;
            for (int k = 0; k < c.nrec; k++)
                if (rect_eq(&c.r[k], &strips[s]))
                    seen = 1;
            printf("%s.annulus.strip.%s.exact_rect=%d\n", pp, STRIP_NAME[s],
                   seen);
            if (seen)
                exact++;
            a_cells += cells;
            a_cov += cov;
        }
        printf("%s.annulus.cells=%ld\n", pp, a_cells);
        printf("%s.annulus.covered=%ld\n", pp, a_cov);
        printf("%s.annulus.exact_rects=%d\n", pp, exact);
        int annulus_full = (a_cells > 0 && a_cov == a_cells) ? 1 : 0;
        printf("%s.annulus.FULLY_COVERED=%d\n", pp, annulus_full);
        printf("%s.annulus.UNTOUCHED=%d\n", pp, a_cov == 0 ? 1 : 0);

        struct rect inside = { 0, 0, have_geom ? win_w : 0,
                               have_geom ? win_h : 0 };
        long i_cells = 0, i_cov = 0;
        if (grid_ok)
            grid_count(&g, &inside, &i_cells, &i_cov);
        printf("%s.inside.cells=%ld\n", pp, i_cells);
        printf("%s.inside.covered=%ld\n", pp, i_cov);
        printf("%s.inside.FULLY_COVERED=%d\n", pp,
               (i_cells > 0 && i_cov == i_cells) ? 1 : 0);

        int match = 0;
        if (grid_ok) {
            switch (PHASES[p].expect) {
            case EX_NO_ANNULUS:
                match = (a_cov == 0) ? 1 : 0;
                break;
            case EX_ANNULUS_FULL:
                match = annulus_full;
                break;
            default:
                match = (i_cells > 0 && i_cov == i_cells && a_cov == 0) ? 1 : 0;
                break;
            }
        }
        printf("%s.MATCH=%d\n", pp, match);
        if (!match)
            failures++;

        grid_free(&g);
    }

    XDamageDestroy(dpy, dmg);
    XFreeGC(dpy, gc);
    emark = g_err_total;
    XCompositeUnredirectWindow(dpy, win, CompositeRedirectManual);
    (void) errs_since(dpy, &emark);
    XDestroyWindow(dpy, win);
    XSync(dpy, False);
    XCloseDisplay(dpy);

    printf("result.phases_checked=%d\n", NPHASE);
    printf("result.phases_failed=%d\n", failures);
    printf("result.x_errors_total=%ld\n", g_err_total);
    printf("result.last_error.code=%d\n", g_err_code);
    printf("result.last_error.major=%d\n", g_err_major);
    printf("result.last_error.minor=%d\n", g_err_minor);
    printf("probe.result=%s\n", failures ? "MISMATCH" : "ok");
    return failures ? 2 : 0;
}
