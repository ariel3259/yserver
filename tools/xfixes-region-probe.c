/* xfixes-region-probe — the rect lists XFIXES and SHAPE hand back for region
 *                        requests, so the region code can be diffed against
 *                        Xorg's (dix/region.c = pixman).
 *
 * Deterministic: no XIDs, serials or timestamps are printed; one fact per
 * line. Run against Xephyr and yserver and diff; the Xephyr output is kept
 * as tools/xfixes-region-probe.xephyr.txt.
 *
 *   cc -o target/xfixes-region-probe tools/xfixes-region-probe.c \
 *      -lxcb -lxcb-xfixes -lxcb-shape
 *   DISPLAY=:N target/xfixes-region-probe
 *
 * Golden: `Xephyr :N -screen 800x600`. yserver: in vng, a one-line registry
 * for tools/vng-suite.sh --registry whose guest script compiles and runs this.
 *
 * Xorg facts this exercises (read from ../xserver):
 *   - XFIXES CreateRegion is RegionFromRects(CT_UNSORTED): validated, so
 *     coalesced; x2/y2 clamp at MAXSHORT.
 *   - SHAPE Rectangles keeps YXBanded input as given (no validation, no
 *     coalescing); other orderings are validated; a wrong ordering claim is
 *     BadMatch (VerifyRectOrder).
 *   - SHAPE RegionOperate on an UNSHAPED window: Union is a no-op, Intersect
 *     takes the source, Subtract starts from the default shape, Invert gives
 *     the empty region; on a shaped one Invert is source - current.
 *   - the input shape's default is the BOUNDING default (border included).
 *   - GetRectangles on an unshaped bounding/input shape reports
 *     [-bw,-bw w+bw x h+bw] (ProcShapeGetRectangles), not the real default. */
#include <stdio.h>
#include <stdlib.h>
#include <xcb/xcb.h>
#include <xcb/xfixes.h>
#include <xcb/shape.h>

static xcb_connection_t *c;
static xcb_window_t root;

typedef xcb_rectangle_t R;

static void print_rects(const char *tag, const R *r, int n) {
    printf("%s n=%d", tag, n);
    for (int i = 0; i < n; i++)
        printf(" [%d,%d %ux%u]", r[i].x, r[i].y, r[i].width, r[i].height);
    printf("\n");
}

static void check(const char *what, xcb_void_cookie_t ck) {
    xcb_generic_error_t *e = xcb_request_check(c, ck);
    if (e) {
        printf("%s ERR code=%d minor=%d\n", what, e->error_code, e->minor_code);
        free(e);
    }
}

static void fetch(const char *tag, xcb_xfixes_region_t reg) {
    xcb_generic_error_t *e = NULL;
    xcb_xfixes_fetch_region_reply_t *r =
        xcb_xfixes_fetch_region_reply(c, xcb_xfixes_fetch_region(c, reg), &e);
    if (!r) {
        printf("%s fetch ERR code=%d\n", tag, e ? e->error_code : -1);
        free(e);
        return;
    }
    printf("%s ext=[%d,%d %ux%u] ", tag, r->extents.x, r->extents.y, r->extents.width,
           r->extents.height);
    print_rects("", xcb_xfixes_fetch_region_rectangles(r),
                xcb_xfixes_fetch_region_rectangles_length(r));
    free(r);
}

static xcb_xfixes_region_t mk(const R *r, int n) {
    xcb_xfixes_region_t id = xcb_generate_id(c);
    check("create", xcb_xfixes_create_region_checked(c, id, n, r));
    return id;
}

static void destroy(xcb_xfixes_region_t r) { xcb_xfixes_destroy_region(c, r); }

/* --- region inputs --- */
static const R V_ADJ[] = {{0, 0, 10, 5}, {0, 5, 10, 5}};               /* vertically adjacent, equal span */
static const R OVL[] = {{0, 0, 10, 10}, {0, 5, 10, 10}};               /* overlapping */
static const R MULTI[] = {{20, 20, 10, 10}, {0, 0, 10, 10}, {5, 5, 10, 10}, {25, 0, 5, 30}};
static const R ZERO[] = {{0, 0, 0, 10}, {5, 5, 10, 0}, {1, 1, 2, 2}};  /* zero-size + one real */
static const R NEG[] = {{-10, -10, 15, 15}, {-5, 0, 5, 5}};
static const R H_ADJ[] = {{0, 0, 5, 5}, {5, 0, 5, 5}};                 /* horizontally touching */
static const R ELL[] = {{0, 0, 20, 5}, {0, 5, 5, 15}};
static const R BIG[] = {{0, 0, 30, 30}};
static const R HOLE[] = {{10, 10, 10, 10}};
static const R CLAMP[] = {{32760, 32760, 100, 100}};
static const R STRIPES[] = {{0, 0, 30, 2}, {0, 4, 30, 2}, {0, 8, 30, 2}};
static const R COLS[] = {{0, 0, 2, 10}, {4, 0, 2, 10}, {8, 0, 2, 10}};
static const R FRESH[] = {{10, 10, 10, 10}, {90, 70, 20, 20}};       /* one rect past the window's edge */

#define N(a) ((int)(sizeof(a) / sizeof((a)[0])))

static void xfixes_section(void) {
    printf("== xfixes create/fetch\n");
    struct { const char *n; const R *r; int k; } in[] = {
        {"v_adj", V_ADJ, N(V_ADJ)}, {"ovl", OVL, N(OVL)}, {"multi", MULTI, N(MULTI)},
        {"empty", NULL, 0}, {"zero", ZERO, N(ZERO)}, {"neg", NEG, N(NEG)},
        {"h_adj", H_ADJ, N(H_ADJ)}, {"ell", ELL, N(ELL)}, {"clamp", CLAMP, N(CLAMP)},
        {"stripes", STRIPES, N(STRIPES)}, {"cols", COLS, N(COLS)},
    };
    for (unsigned i = 0; i < sizeof(in) / sizeof(in[0]); i++) {
        xcb_xfixes_region_t r = mk(in[i].r, in[i].k);
        char tag[64];
        snprintf(tag, sizeof tag, "create.%s", in[i].n);
        fetch(tag, r);
        destroy(r);
    }

    printf("== xfixes ops\n");
    struct { const char *n; const R *a; int na; const R *b; int nb; } pairs[] = {
        {"vadj_halves", V_ADJ, 1, V_ADJ + 1, 1},
        {"big_hole", BIG, 1, HOLE, 1},
        {"multi_ovl", MULTI, N(MULTI), OVL, N(OVL)},
        {"multi_neg", MULTI, N(MULTI), NEG, N(NEG)},
        {"ell_big", ELL, N(ELL), BIG, 1},
        {"stripes_cols", STRIPES, N(STRIPES), COLS, N(COLS)},
        {"empty_big", NULL, 0, BIG, 1},
        {"hadj_halves", H_ADJ, 1, H_ADJ + 1, 1},
    };
    for (unsigned i = 0; i < sizeof(pairs) / sizeof(pairs[0]); i++) {
        xcb_xfixes_region_t a = mk(pairs[i].a, pairs[i].na);
        xcb_xfixes_region_t b = mk(pairs[i].b, pairs[i].nb);
        xcb_xfixes_region_t d = mk(NULL, 0);
        char tag[96];
        check("union", xcb_xfixes_union_region_checked(c, a, b, d));
        snprintf(tag, sizeof tag, "union.%s", pairs[i].n);
        fetch(tag, d);
        check("intersect", xcb_xfixes_intersect_region_checked(c, a, b, d));
        snprintf(tag, sizeof tag, "intersect.%s", pairs[i].n);
        fetch(tag, d);
        check("subtract", xcb_xfixes_subtract_region_checked(c, a, b, d));
        snprintf(tag, sizeof tag, "subtract.%s", pairs[i].n);
        fetch(tag, d);
        check("subtract", xcb_xfixes_subtract_region_checked(c, b, a, d));
        snprintf(tag, sizeof tag, "subtract_rev.%s", pairs[i].n);
        fetch(tag, d);
        destroy(a);
        destroy(b);
        destroy(d);
    }

    /* in place: dest == source */
    {
        xcb_xfixes_region_t a = mk(BIG, 1), b = mk(HOLE, 1);
        check("subtract_inplace", xcb_xfixes_subtract_region_checked(c, a, b, a));
        fetch("subtract_inplace.big_hole", a);
        check("union_inplace", xcb_xfixes_union_region_checked(c, a, b, a));
        fetch("union_inplace.back", a);
        destroy(a);
        destroy(b);
    }

    struct { const char *n; const R *r; int k; R bounds; } inv[] = {
        {"multi", MULTI, N(MULTI), {0, 0, 40, 40}},
        {"neg", NEG, N(NEG), {-20, -20, 30, 30}},
        {"empty", NULL, 0, {5, 5, 10, 10}},
        {"hole", HOLE, 1, {0, 0, 30, 30}},
        {"clamp", BIG, 1, {32000, 32000, 2000, 2000}},
        {"outside", HOLE, 1, {100, 100, 5, 5}},
    };
    for (unsigned i = 0; i < sizeof(inv) / sizeof(inv[0]); i++) {
        xcb_xfixes_region_t a = mk(inv[i].r, inv[i].k), d = mk(NULL, 0);
        char tag[64];
        check("invert", xcb_xfixes_invert_region_checked(c, a, inv[i].bounds, d));
        snprintf(tag, sizeof tag, "invert.%s", inv[i].n);
        fetch(tag, d);
        destroy(a);
        destroy(d);
    }

    {
        xcb_xfixes_region_t a = mk(MULTI, N(MULTI));
        check("translate", xcb_xfixes_translate_region_checked(c, a, 3, -4));
        fetch("translate.multi", a);
        check("translate", xcb_xfixes_translate_region_checked(c, a, -30, -30));
        fetch("translate.multi_neg", a);
        xcb_xfixes_region_t d = mk(NULL, 0);
        check("extents", xcb_xfixes_region_extents_checked(c, a, d));
        fetch("extents.multi", d);
        destroy(a);
        a = mk(NULL, 0);
        check("extents", xcb_xfixes_region_extents_checked(c, a, d));
        fetch("extents.empty", d);
        destroy(a);
        a = mk(V_ADJ, N(V_ADJ));
        check("copy", xcb_xfixes_copy_region_checked(c, a, d));
        fetch("copy.v_adj", d);
        check("set", xcb_xfixes_set_region_checked(c, d, N(OVL), OVL));
        fetch("set.ovl", d);
        destroy(a);
        destroy(d);
    }
}

static xcb_window_t mkwin(void) {
    xcb_window_t w = xcb_generate_id(c);
    xcb_create_window(c, XCB_COPY_FROM_PARENT, w, root, 10, 10, 100, 80, 3,
                      XCB_WINDOW_CLASS_INPUT_OUTPUT, XCB_COPY_FROM_PARENT, 0, NULL);
    return w;
}

static void get_shape(const char *tag, xcb_window_t w, uint8_t kind) {
    xcb_generic_error_t *e = NULL;
    xcb_shape_get_rectangles_reply_t *r =
        xcb_shape_get_rectangles_reply(c, xcb_shape_get_rectangles(c, w, kind), &e);
    if (!r) {
        printf("%s get ERR code=%d\n", tag, e ? e->error_code : -1);
        free(e);
        return;
    }
    char t[128];
    snprintf(t, sizeof t, "%s.k%d ordering=%d", tag, kind, r->ordering);
    print_rects(t, xcb_shape_get_rectangles_rectangles(r), xcb_shape_get_rectangles_rectangles_length(r));
    free(r);
}

static void get_all(const char *tag, xcb_window_t w) {
    for (uint8_t k = 0; k <= 2; k++)
        get_shape(tag, w, k);
    xcb_shape_query_extents_reply_t *q =
        xcb_shape_query_extents_reply(c, xcb_shape_query_extents(c, w), NULL);
    if (q) {
        printf("%s.extents bshaped=%d cshaped=%d b=[%d,%d %ux%u] c=[%d,%d %ux%u]\n", tag,
               q->bounding_shaped, q->clip_shaped, q->bounding_shape_extents_x,
               q->bounding_shape_extents_y, q->bounding_shape_extents_width,
               q->bounding_shape_extents_height, q->clip_shape_extents_x,
               q->clip_shape_extents_y, q->clip_shape_extents_width,
               q->clip_shape_extents_height);
        free(q);
    }
}

static void rects(const char *what, xcb_window_t w, uint8_t op, uint8_t kind, uint8_t ordering,
                  int16_t xo, int16_t yo, const R *r, int n) {
    check(what, xcb_shape_rectangles_checked(c, op, kind, ordering, w, xo, yo, n, r));
}

static void region_from_window(const char *tag, xcb_window_t w, uint8_t kind) {
    xcb_xfixes_region_t d = xcb_generate_id(c);
    check("from_window", xcb_xfixes_create_region_from_window_checked(c, d, w, kind));
    fetch(tag, d);
    destroy(d);
}

static void shape_section(void) {
    const uint8_t B = XCB_SHAPE_SK_BOUNDING, C = XCB_SHAPE_SK_CLIP, I = XCB_SHAPE_SK_INPUT;
    const uint8_t U = XCB_CLIP_ORDERING_UNSORTED, YX = XCB_CLIP_ORDERING_YX_SORTED,
                  BAND = XCB_CLIP_ORDERING_YX_BANDED;
    xcb_window_t w;

    printf("== shape unshaped\n");
    w = mkwin();
    get_all("unshaped", w);
    region_from_window("fromwin.unshaped.b", w, B);
    region_from_window("fromwin.unshaped.c", w, C);

    printf("== shape ops on unshaped windows\n");
    const struct { const char *n; uint8_t op; } ops[] = {
        {"set", XCB_SHAPE_SO_SET}, {"union", XCB_SHAPE_SO_UNION},
        {"intersect", XCB_SHAPE_SO_INTERSECT}, {"subtract", XCB_SHAPE_SO_SUBTRACT},
        {"invert", XCB_SHAPE_SO_INVERT},
    };
    for (unsigned i = 0; i < 5; i++) {
        for (uint8_t k = 0; k <= 2; k++) {
            xcb_window_t v = mkwin();
            char tag[64];
            snprintf(tag, sizeof tag, "fresh.%s.on%d", ops[i].n, k);
            rects(tag, v, ops[i].op, k, U, 0, 0, FRESH, N(FRESH));
            get_all(tag, v);
            xcb_destroy_window(c, v);
        }
    }

    printf("== shape set orderings\n");
    rects("set.multi", w, XCB_SHAPE_SO_SET, B, U, 0, 0, MULTI, N(MULTI));
    get_all("set.multi", w);
    region_from_window("fromwin.multi.b", w, B);
    rects("set.v_adj.unsorted", w, XCB_SHAPE_SO_SET, B, U, 0, 0, V_ADJ, N(V_ADJ));
    get_shape("set.v_adj.unsorted", w, B);
    rects("set.v_adj.banded", w, XCB_SHAPE_SO_SET, B, BAND, 0, 0, V_ADJ, N(V_ADJ));
    get_shape("set.v_adj.banded", w, B);
    region_from_window("fromwin.v_adj.banded", w, B);
    rects("set.h_adj.banded", w, XCB_SHAPE_SO_SET, B, BAND, 0, 0, H_ADJ, N(H_ADJ));
    get_shape("set.h_adj.banded", w, B);
    rects("set.h_adj.yxsorted", w, XCB_SHAPE_SO_SET, B, YX, 0, 0, H_ADJ, N(H_ADJ));
    get_shape("set.h_adj.yxsorted", w, B);
    rects("set.multi.yxsorted_bad", w, XCB_SHAPE_SO_SET, B, YX, 0, 0, MULTI, N(MULTI));
    rects("set.ovl.banded_bad", w, XCB_SHAPE_SO_SET, B, BAND, 0, 0, OVL, N(OVL));
    rects("set.ovl.ysorted", w, XCB_SHAPE_SO_SET, B, XCB_CLIP_ORDERING_Y_SORTED, 0, 0, OVL, N(OVL));
    get_shape("set.ovl.ysorted", w, B);
    rects("set.zero", w, XCB_SHAPE_SO_SET, B, U, 0, 0, ZERO, N(ZERO));
    get_shape("set.zero", w, B);
    rects("set.empty", w, XCB_SHAPE_SO_SET, B, U, 0, 0, NULL, 0);
    get_all("set.empty", w);
    rects("set.neg.off", w, XCB_SHAPE_SO_SET, B, U, 7, -3, NEG, N(NEG));
    get_shape("set.neg.off", w, B);

    printf("== shape ops on a shaped window\n");
    rects("big", w, XCB_SHAPE_SO_SET, B, U, 0, 0, BIG, 1);
    rects("sub.hole", w, XCB_SHAPE_SO_SUBTRACT, B, U, 0, 0, HOLE, 1);
    get_shape("sub.hole", w, B);
    rects("union.hole", w, XCB_SHAPE_SO_UNION, B, U, 0, 0, HOLE, 1);
    get_shape("union.hole", w, B);
    rects("union.multi", w, XCB_SHAPE_SO_UNION, B, U, 5, 5, MULTI, N(MULTI));
    get_shape("union.multi", w, B);
    rects("intersect.stripes", w, XCB_SHAPE_SO_INTERSECT, B, U, 0, 0, STRIPES, N(STRIPES));
    get_shape("intersect.stripes", w, B);
    rects("union.stripes_gaps", w, XCB_SHAPE_SO_UNION, B, U, 0, 2, STRIPES, N(STRIPES));
    get_shape("union.stripes_gaps", w, B);
    rects("invert.big", w, XCB_SHAPE_SO_INVERT, B, U, 0, 0, BIG, 1);
    get_shape("invert.big", w, B);
    rects("invert.ell", w, XCB_SHAPE_SO_INVERT, B, U, 0, 0, ELL, N(ELL));
    get_shape("invert.ell", w, B);
    rects("sub.all", w, XCB_SHAPE_SO_SUBTRACT, B, U, -100, -100, BIG, 1);
    get_shape("sub.all_offscreen", w, B);
    check("offset", xcb_shape_offset_checked(c, B, w, -4, 6));
    get_shape("offset.b", w, B);
    get_all("offset.all", w);

    printf("== clip and input kinds\n");
    rects("clip.set", w, XCB_SHAPE_SO_SET, C, U, 0, 0, MULTI, N(MULTI));
    rects("clip.sub", w, XCB_SHAPE_SO_SUBTRACT, C, U, 0, 0, HOLE, 1);
    get_shape("clip.sub", w, C);
    region_from_window("fromwin.clip", w, C);
    rects("input.union_unshaped", w, XCB_SHAPE_SO_UNION, I, U, 0, 0, HOLE, 1);
    get_shape("input.union_unshaped", w, I);
    rects("input.sub", w, XCB_SHAPE_SO_SUBTRACT, I, U, 0, 0, HOLE, 1);
    get_shape("input.sub", w, I);
    check("offset.c", xcb_shape_offset_checked(c, C, w, 2, 2));
    get_all("offset.c", w);

    printf("== combine from another window\n");
    xcb_window_t src = mkwin();
    rects("src.set", src, XCB_SHAPE_SO_SET, B, U, 0, 0, ELL, N(ELL));
    check("combine.union", xcb_shape_combine_checked(c, XCB_SHAPE_SO_UNION, B, B, w, 1, 1, src));
    get_shape("combine.union", w, B);
    check("combine.set_unshaped",
          xcb_shape_combine_checked(c, XCB_SHAPE_SO_SET, B, C, w, 0, 0, mkwin()));
    get_shape("combine.set_unshaped_clip", w, B);
    check("mask.none", xcb_shape_mask_checked(c, XCB_SHAPE_SO_SET, B, w, 0, 0, XCB_NONE));
    get_all("mask.none", w);
}

int main(void) {
    c = xcb_connect(NULL, NULL);
    if (xcb_connection_has_error(c)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    root = xcb_setup_roots_iterator(xcb_get_setup(c)).data->root;
    xcb_xfixes_query_version_reply_t *v =
        xcb_xfixes_query_version_reply(c, xcb_xfixes_query_version(c, 5, 0), NULL);
    xcb_shape_query_version_reply_t *s =
        xcb_shape_query_version_reply(c, xcb_shape_query_version(c), NULL);
    if (!v || !s) {
        fprintf(stderr, "XFIXES or SHAPE missing\n");
        return 1;
    }
    free(v);
    free(s);
    xfixes_section();
    shape_section();
    xcb_disconnect(c);
    return 0;
}
