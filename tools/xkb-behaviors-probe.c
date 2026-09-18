/* xkb-behaviors-probe — does XkbGetMap()/XkbGetUpdatedMap() leave
 *                       xkb->server->behaviors NULL?
 *
 * Raised by issue #150 (kaaduu, 2026-09-15): a native Barrier client
 * (barrierc 2.4.0) segfaults the instant its network handshake completes,
 * but only under yserver — the same binary on the same box is stable under
 * Xorg. GDB puts the fault in XWindowsKeyState.cpp:608
 *
 *     const XkbBehavior& b = m_xkb->server->behaviors[keycode];
 *
 * with keycode = 9 and a faulting address of 0x12. XkbBehavior is two bytes
 * (unsigned char type; unsigned char data — XKBstr.h:110), so 9 * 2 == 18 ==
 * 0x12 and the base pointer is NULL: libX11 never allocated the array, rather
 * than allocating a short or malformed one. XkbGetUpdatedMap() returned
 * Success regardless, so the client gets no signal that anything is missing.
 *
 * The suspected mechanism is a GetMap *reply header* difference, not a
 * difference in the section bodies. libX11's _XkbReadGetMapReply allocates
 * the server map from the reply's `present` mask alone:
 *
 *     if (!xkb->server) {
 *         mask = rep->present & XkbAllServerInfoMask;
 *         if (mask && (XkbAllocServerMap(xkb, mask, rep->totalActs) != Success))
 *
 * and XkbAllocServerMap only calloc()s `behaviors` under
 * `if (which & XkbKeyBehaviorsMask)`. _XkbReadKeyBehaviors is the only other
 * allocator and it is gated on `rep->totalKeyBehaviors > 0` — an empty
 * behaviors section allocates nothing. So a server that clears bit 5 of
 * `present` and reports zero behaviors leaves the array NULL forever.
 *
 * Xorg keeps bit 5 set even when it has no non-default behaviors to send:
 * XkbSizeKeyBehaviors (xkb/xkb.c) only strips the bit when the *server's own*
 * behaviors array is absent, and xkbInit.c allocates it with
 * XkbAllServerInfoMask, so it never is. totalKeyBehaviors may still be 0.
 *
 * This probe measures both halves, on two independent connections:
 *
 *   PART 1 (xcb, raw wire) — issues GetMap with Barrier's exact mask and
 *       prints the reply header verbatim: `present` decoded bit by bit, and
 *       the first/n/total triple for every section. No libX11 interpretation
 *       is involved; this is the byte-level answer.
 *
 *   PART 2 (Xlib) — mirrors Barrier's own call sequence exactly:
 *       XWindowsKeyState::init() calls
 *           XkbGetMap(display, ACTIONS|BEHAVIORS|ALL_CLIENT_INFO, UseCoreKbd)
 *       and XWindowsKeyState::updateKeyMap() then calls
 *           XkbGetUpdatedMap(display, <the same mask>, m_xkb)
 *       before walking min_key_code..max_key_code and dereferencing
 *       server->behaviors[keycode]. The probe does the same two calls and
 *       then reports whether that dereference would be safe — it never
 *       performs an unguarded one itself, so the probe cannot crash.
 *
 * OUTPUT is one `key=value` fact per line, in a fixed order, with the same
 * number of lines whatever the server answers: run the SAME binary under
 * yserver and under stock Xorg/Xephyr and `diff` the two outputs. Raw pointer
 * values are printed on their own `.ptr=` lines so they are available without
 * poisoning the diff; the NULL/non-null verdict is a separate line. Nothing
 * host-, pid-, time- or DISPLAY-dependent is printed.
 *
 * Read-only: nothing is mapped, grabbed, or changed.
 *
 * ORACLE NOTE — DO NOT USE Xephyr FOR THIS ONE. The probe makes TWO
 * connections (xcb for the wire half, Xlib for the barrier half), and Xephyr
 * forks xkbcomp on every client connect. The second connect then races the
 * first one's xkbcomp, and XOpenDisplay returns NULL with nothing on stderr,
 * so the run comes out as `xlib.connect=fail` and exit 1. MEASURED
 * 2026-09-17: 5 failures in 15 runs against Xephyr 21.1.24, 0 in 15 against
 * a real Xorg on the same box, and 0 in 15 for a two-connection minimal
 * reproducer that skips the XKB traffic. A single-connection client
 * (xdpyinfo) never fails on that Xephyr either. It is the nested server, not
 * this probe: take the Xorg half of the A/B from a real Xorg, or from the
 * `--server xorg` side of tools/vng-scenarios/xkb-behaviors-probe.sh.
 *
 * Build: gcc -O1 -o /tmp/xkb-behaviors-probe tools/xkb-behaviors-probe.c \
 *            -lX11 -lxcb -lxcb-xkb
 * Run:   DISPLAY=:7 /tmp/xkb-behaviors-probe > /tmp/yserver.txt
 *        DISPLAY=:0 /tmp/xkb-behaviors-probe > /tmp/xorg.txt
 *        diff -u /tmp/xorg.txt /tmp/yserver.txt
 *        (an explicit display may also be passed as argv[1])
 *
 * Exit: 0 = behaviors[9] safe, 2 = would segfault, 1 = probe could not run.
 */

#include <X11/XKBlib.h>
#include <X11/Xlib.h>
#include <X11/extensions/XKB.h>
#include <X11/extensions/XKBstr.h>
#include <stdio.h>
#include <stdlib.h>
#include <xcb/xcb.h>
#include <xcb/xkb.h>

/* Barrier's mask, verbatim from XWindowsKeyState::init() and
 * XWindowsKeyState::updateKeyMap(). Both call sites use this same value. */
#define BARRIER_MASK \
    (XkbKeyActionsMask | XkbKeyBehaviorsMask | XkbAllClientInfoMask)

/* The keycode in the reporter's backtrace. Barrier's loop starts at
 * min_key_code and skips keys with zero groups, so this is the first keycode
 * to reach XWindowsKeyState.cpp:608 on a normal keymap. */
#define PROBE_KEYCODE 9

#define NULLNESS(p) ((p) ? "non-null" : "NULL")

/* Decode a map-component mask bit by bit, one line per bit, always all
 * eight lines so the line count never varies between servers. */
static void print_mask_bits(const char *prefix, unsigned m)
{
    printf("%s=0x%04x\n", prefix, m & 0xffffu);
    printf("%s.KeyTypes=%d\n", prefix, (m & XkbKeyTypesMask) ? 1 : 0);
    printf("%s.KeySyms=%d\n", prefix, (m & XkbKeySymsMask) ? 1 : 0);
    printf("%s.ModifierMap=%d\n", prefix, (m & XkbModifierMapMask) ? 1 : 0);
    printf("%s.ExplicitComponents=%d\n", prefix,
           (m & XkbExplicitComponentsMask) ? 1 : 0);
    printf("%s.KeyActions=%d\n", prefix, (m & XkbKeyActionsMask) ? 1 : 0);
    printf("%s.KeyBehaviors=%d\n", prefix, (m & XkbKeyBehaviorsMask) ? 1 : 0);
    printf("%s.VirtualMods=%d\n", prefix, (m & XkbVirtualModsMask) ? 1 : 0);
    printf("%s.VirtualModMap=%d\n", prefix, (m & XkbVirtualModMapMask) ? 1 : 0);
}

/* Every wire fact, printed with placeholders when the request failed, so the
 * output has the same shape either way. */
static void print_wire_unavailable(void)
{
    print_mask_bits("wire.present", 0);
    printf("wire.minKeyCode=-1\n");
    printf("wire.maxKeyCode=-1\n");
    printf("wire.keytypes.first=-1\n");
    printf("wire.keytypes.n=-1\n");
    printf("wire.keytypes.total=-1\n");
    printf("wire.keysyms.first=-1\n");
    printf("wire.keysyms.n=-1\n");
    printf("wire.keysyms.total=-1\n");
    printf("wire.keyactions.first=-1\n");
    printf("wire.keyactions.n=-1\n");
    printf("wire.keyactions.total=-1\n");
    printf("wire.keybehaviors.first=-1\n");
    printf("wire.keybehaviors.n=-1\n");
    printf("wire.keybehaviors.total=-1\n");
    printf("wire.explicit.first=-1\n");
    printf("wire.explicit.n=-1\n");
    printf("wire.explicit.total=-1\n");
    printf("wire.modmap.first=-1\n");
    printf("wire.modmap.n=-1\n");
    printf("wire.modmap.total=-1\n");
    printf("wire.vmodmap.first=-1\n");
    printf("wire.vmodmap.n=-1\n");
    printf("wire.vmodmap.total=-1\n");
    printf("wire.virtualmods=-1\n");
    print_mask_bits("wire.libx11.allocservermap.which", 0);
    printf("wire.libx11.allocservermap.allocates_behaviors=-1\n");
    printf("wire.libx11.readkeybehaviors.allocates=-1\n");
}

/* ---- PART 1: the raw GetMap reply header, straight off the wire -------- */
static int probe_wire(const char *display)
{
    int screen = 0;
    xcb_connection_t *c = xcb_connect(display, &screen);
    if (!c || xcb_connection_has_error(c)) {
        printf("wire.connect=fail\n");
        printf("wire.useextension.supported=-1\n");
        printf("wire.server.xkb_major=-1\n");
        printf("wire.server.xkb_minor=-1\n");
        printf("wire.getmap=fail\n");
        printf("wire.getmap.error_code=0\n");
        print_wire_unavailable();
        if (c)
            xcb_disconnect(c);
        return 1;
    }
    printf("wire.connect=ok\n");

    /* The server refuses XKB requests from a client that has not run
     * UseExtension (_XkbClientInitialized in Xorg's ProcXkbGetMap). */
    xcb_xkb_use_extension_reply_t *ue =
        xcb_xkb_use_extension_reply(c, xcb_xkb_use_extension(c, 1, 0), NULL);
    if (!ue) {
        printf("wire.useextension.supported=-1\n");
        printf("wire.server.xkb_major=-1\n");
        printf("wire.server.xkb_minor=-1\n");
        printf("wire.getmap=fail\n");
        printf("wire.getmap.error_code=0\n");
        print_wire_unavailable();
        xcb_disconnect(c);
        return 1;
    }
    printf("wire.useextension.supported=%u\n", ue->supported);
    printf("wire.server.xkb_major=%u\n", ue->serverMajor);
    printf("wire.server.xkb_minor=%u\n", ue->serverMinor);
    free(ue);

    xcb_generic_error_t *err = NULL;
    xcb_xkb_get_map_reply_t *r = xcb_xkb_get_map_reply(
        c,
        xcb_xkb_get_map(c, XCB_XKB_ID_USE_CORE_KBD,
                        BARRIER_MASK, /* full    */
                        0,            /* partial */
                        0, 0,         /* firstType,        nTypes           */
                        0, 0,         /* firstKeySym,      nKeySyms         */
                        0, 0,         /* firstKeyAction,   nKeyActions      */
                        0, 0,         /* firstKeyBehavior, nKeyBehaviors    */
                        0,            /* virtualMods                        */
                        0, 0,         /* firstKeyExplicit, nKeyExplicit     */
                        0, 0,         /* firstModMapKey,   nModMapKeys      */
                        0, 0),        /* firstVModMapKey,  nVModMapKeys     */
        &err);

    if (!r) {
        printf("wire.getmap=fail\n");
        printf("wire.getmap.error_code=%u\n", err ? err->error_code : 0);
        free(err);
        print_wire_unavailable();
        xcb_disconnect(c);
        return 1;
    }
    printf("wire.getmap=ok\n");
    printf("wire.getmap.error_code=0\n");

    print_mask_bits("wire.present", r->present);
    printf("wire.minKeyCode=%u\n", r->minKeyCode);
    printf("wire.maxKeyCode=%u\n", r->maxKeyCode);
    printf("wire.keytypes.first=%u\n", r->firstType);
    printf("wire.keytypes.n=%u\n", r->nTypes);
    printf("wire.keytypes.total=%u\n", r->totalTypes);
    printf("wire.keysyms.first=%u\n", r->firstKeySym);
    printf("wire.keysyms.n=%u\n", r->nKeySyms);
    printf("wire.keysyms.total=%u\n", r->totalSyms);
    printf("wire.keyactions.first=%u\n", r->firstKeyAction);
    printf("wire.keyactions.n=%u\n", r->nKeyActions);
    printf("wire.keyactions.total=%u\n", r->totalActions);
    printf("wire.keybehaviors.first=%u\n", r->firstKeyBehavior);
    printf("wire.keybehaviors.n=%u\n", r->nKeyBehaviors);
    printf("wire.keybehaviors.total=%u\n", r->totalKeyBehaviors);
    printf("wire.explicit.first=%u\n", r->firstKeyExplicit);
    printf("wire.explicit.n=%u\n", r->nKeyExplicit);
    printf("wire.explicit.total=%u\n", r->totalKeyExplicit);
    printf("wire.modmap.first=%u\n", r->firstModMapKey);
    printf("wire.modmap.n=%u\n", r->nModMapKeys);
    printf("wire.modmap.total=%u\n", r->totalModMapKeys);
    printf("wire.vmodmap.first=%u\n", r->firstVModMapKey);
    printf("wire.vmodmap.n=%u\n", r->nVModMapKeys);
    printf("wire.vmodmap.total=%u\n", r->totalVModMapKeys);
    printf("wire.virtualmods=0x%04x\n", r->virtualMods);

    /* libX11's two allocation predicates, evaluated on this real reply. */
    print_mask_bits("wire.libx11.allocservermap.which",
                    r->present & XkbAllServerInfoMask);
    printf("wire.libx11.allocservermap.allocates_behaviors=%d\n",
           (r->present & XkbKeyBehaviorsMask) ? 1 : 0);
    printf("wire.libx11.readkeybehaviors.allocates=%d\n",
           r->totalKeyBehaviors > 0 ? 1 : 0);

    free(r);
    xcb_disconnect(c);
    return 0;
}

/* Placeholder block for PART 2, so the line count never varies. */
static void print_xlib_unavailable(void)
{
    printf("xlib.getmap.result=NULL\n");
    printf("xlib.getmap.ptr=%p\n", (void *) NULL);
    printf("xlib.getmap.server=NULL\n");
    printf("xlib.getmap.server.behaviors=NULL\n");
    printf("xlib.getupdatedmap.status=-1\n");
    printf("xlib.getupdatedmap.success=0\n");
    printf("xlib.min_key_code=-1\n");
    printf("xlib.max_key_code=-1\n");
    printf("xlib.map=NULL\n");
    printf("xlib.map.ptr=%p\n", (void *) NULL);
    printf("xlib.map.num_types=-1\n");
    printf("xlib.map.num_syms=-1\n");
    printf("xlib.map.modmap=NULL\n");
    printf("xlib.server=NULL\n");
    printf("xlib.server.ptr=%p\n", (void *) NULL);
    printf("xlib.server.behaviors=NULL\n");
    printf("xlib.server.behaviors.ptr=%p\n", (void *) NULL);
    printf("xlib.server.key_acts=NULL\n");
    printf("xlib.server.acts=NULL\n");
    printf("xlib.server.num_acts=-1\n");
    printf("xlib.server.explicit=NULL\n");
    printf("xlib.server.vmodmap=NULL\n");
    printf("xlib.keycode%d.in_range=0\n", PROBE_KEYCODE);
    printf("xlib.keycode%d.deref_safe=0\n", PROBE_KEYCODE);
    printf("xlib.keycode%d.behavior.type=unavailable\n", PROBE_KEYCODE);
    printf("xlib.keycode%d.behavior.data=unavailable\n", PROBE_KEYCODE);
    printf("xlib.keycode%d.behavior.is_lock=unavailable\n", PROBE_KEYCODE);
    printf("xlib.nondefault_behaviors=unavailable\n");
}

/* ---- PART 2: Barrier's own Xlib call sequence, faithfully -------------- */
static int probe_xlib(const char *display)
{
    Display *dpy = XOpenDisplay(display);
    if (!dpy) {
        printf("xlib.connect=fail\n");
        printf("xlib.queryextension=0\n");
        print_xlib_unavailable();
        return 1;
    }
    printf("xlib.connect=ok\n");

    int op = 0, ev = 0, err = 0;
    int major = XkbMajorVersion, minor = XkbMinorVersion;
    if (!XkbQueryExtension(dpy, &op, &ev, &err, &major, &minor)) {
        printf("xlib.queryextension=0\n");
        print_xlib_unavailable();
        XCloseDisplay(dpy);
        return 1;
    }
    printf("xlib.queryextension=1\n");

    /* XWindowsKeyState::init(): the FULL Barrier mask, not which=0. */
    XkbDescPtr xkb = XkbGetMap(dpy, BARRIER_MASK, XkbUseCoreKbd);
    printf("xlib.getmap.result=%s\n", NULLNESS(xkb));
    printf("xlib.getmap.ptr=%p\n", (void *) xkb);
    if (!xkb) {
        printf("xlib.getmap.server=NULL\n");
        printf("xlib.getmap.server.behaviors=NULL\n");
        printf("xlib.getupdatedmap.status=-1\n");
        printf("xlib.getupdatedmap.success=0\n");
        printf("xlib.min_key_code=-1\n");
        printf("xlib.max_key_code=-1\n");
        printf("xlib.map=NULL\n");
        printf("xlib.map.ptr=%p\n", (void *) NULL);
        printf("xlib.map.num_types=-1\n");
        printf("xlib.map.num_syms=-1\n");
        printf("xlib.map.modmap=NULL\n");
        printf("xlib.server=NULL\n");
        printf("xlib.server.ptr=%p\n", (void *) NULL);
        printf("xlib.server.behaviors=NULL\n");
        printf("xlib.server.behaviors.ptr=%p\n", (void *) NULL);
        printf("xlib.server.key_acts=NULL\n");
        printf("xlib.server.acts=NULL\n");
        printf("xlib.server.num_acts=-1\n");
        printf("xlib.server.explicit=NULL\n");
        printf("xlib.server.vmodmap=NULL\n");
        printf("xlib.keycode%d.in_range=0\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.deref_safe=0\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.behavior.type=unavailable\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.behavior.data=unavailable\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.behavior.is_lock=unavailable\n", PROBE_KEYCODE);
        printf("xlib.nondefault_behaviors=unavailable\n");
        XCloseDisplay(dpy);
        return 2;
    }
    /* State after init(), before updateKeyMap() runs. */
    printf("xlib.getmap.server=%s\n", NULLNESS(xkb->server));
    printf("xlib.getmap.server.behaviors=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->behaviors : NULL));

    /* XWindowsKeyState::updateKeyMap(): the same mask again. */
    Status st = XkbGetUpdatedMap(dpy, BARRIER_MASK, xkb);
    printf("xlib.getupdatedmap.status=%d\n", (int) st);
    printf("xlib.getupdatedmap.success=%d\n", st == Success ? 1 : 0);

    printf("xlib.min_key_code=%d\n", (int) xkb->min_key_code);
    printf("xlib.max_key_code=%d\n", (int) xkb->max_key_code);

    printf("xlib.map=%s\n", NULLNESS(xkb->map));
    printf("xlib.map.ptr=%p\n", (void *) xkb->map);
    printf("xlib.map.num_types=%d\n", xkb->map ? (int) xkb->map->num_types : -1);
    printf("xlib.map.num_syms=%d\n", xkb->map ? (int) xkb->map->num_syms : -1);
    printf("xlib.map.modmap=%s\n",
           NULLNESS(xkb->map ? (void *) xkb->map->modmap : NULL));

    printf("xlib.server=%s\n", NULLNESS(xkb->server));
    printf("xlib.server.ptr=%p\n", (void *) xkb->server);
    printf("xlib.server.behaviors=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->behaviors : NULL));
    printf("xlib.server.behaviors.ptr=%p\n",
           xkb->server ? (void *) xkb->server->behaviors : (void *) 0);
    printf("xlib.server.key_acts=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->key_acts : NULL));
    printf("xlib.server.acts=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->acts : NULL));
    printf("xlib.server.num_acts=%d\n",
           xkb->server ? (int) xkb->server->num_acts : -1);
    printf("xlib.server.explicit=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->explicit : NULL));
    printf("xlib.server.vmodmap=%s\n",
           NULLNESS(xkb->server ? (void *) xkb->server->vmodmap : NULL));

    /* The exact dereference at XWindowsKeyState.cpp:608, fully guarded. */
    int in_range = (PROBE_KEYCODE >= (int) xkb->min_key_code &&
                    PROBE_KEYCODE <= (int) xkb->max_key_code);
    int safe = in_range && xkb->server != NULL &&
               xkb->server->behaviors != NULL && st == Success;
    printf("xlib.keycode%d.in_range=%d\n", PROBE_KEYCODE, in_range);
    printf("xlib.keycode%d.deref_safe=%d\n", PROBE_KEYCODE, safe);
    if (safe) {
        const XkbBehavior *b = &xkb->server->behaviors[PROBE_KEYCODE];
        printf("xlib.keycode%d.behavior.type=0x%02x\n", PROBE_KEYCODE, b->type);
        printf("xlib.keycode%d.behavior.data=0x%02x\n", PROBE_KEYCODE, b->data);
        printf("xlib.keycode%d.behavior.is_lock=%d\n", PROBE_KEYCODE,
               (b->type & XkbKB_OpMask) == XkbKB_Lock ? 1 : 0);
        /* Xorg only puts non-default behaviors on the wire
         * (XkbWriteKeyBehaviors skips XkbKB_Default), so this separates
         * "present bit set, empty section" from "real behavior data". */
        int n = 0;
        for (int k = xkb->min_key_code; k <= (int) xkb->max_key_code; k++)
            if (xkb->server->behaviors[k].type != XkbKB_Default)
                n++;
        printf("xlib.nondefault_behaviors=%d\n", n);
    } else {
        printf("xlib.keycode%d.behavior.type=unavailable\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.behavior.data=unavailable\n", PROBE_KEYCODE);
        printf("xlib.keycode%d.behavior.is_lock=unavailable\n", PROBE_KEYCODE);
        printf("xlib.nondefault_behaviors=unavailable\n");
    }

    XkbFreeKeyboard(xkb, 0, True);
    XCloseDisplay(dpy);
    return safe ? 0 : 2;
}

int main(int argc, char **argv)
{
    const char *display = argc > 1 ? argv[1] : NULL;

    printf("probe=xkb-behaviors-probe\n");
    printf("probe.format=1\n");
    printf("probe.requested_mask_name="
           "XkbKeyActionsMask|XkbKeyBehaviorsMask|XkbAllClientInfoMask\n");
    print_mask_bits("probe.requested_mask", BARRIER_MASK);
    printf("probe.sizeof_XkbBehavior=%zu\n", sizeof(XkbBehavior));
    printf("probe.keycode=%d\n", PROBE_KEYCODE);
    printf("probe.keycode_byte_offset=0x%02zx\n",
           (size_t) PROBE_KEYCODE * sizeof(XkbBehavior));

    int a = probe_wire(display);
    int b = probe_xlib(display);
    return a ? 1 : b;
}
