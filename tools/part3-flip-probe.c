// Feasibility probe for part 3 of the stage 2c-i debt stage (spec section 9).
//
// This is NOT part 3, and no result of it satisfies any of P3-1..P3-4: it
// drives dumb buffers straight through libdrm and never touches the
// ResourceService, the ledger or a managed scanout buffer. What it answers is
// the question that comes first -- whether this box can run part 3 at all:
// can a session on an active VT take DRM master on the card driving the
// connected output, modeset it, have a page flip ACCEPTED, receive the
// kernel's completion, and obtain an out-fence from an atomic commit.
//
// Build: gcc -O1 -Wall -o /tmp/flip_probe tools/part3-flip-probe.c $(pkg-config --cflags --libs libdrm)
// Run  : from a VT that is the seat's ACTIVE session (logind grants master
//         only there), as ./flip_probe /dev/dri/cardN.
//
// It restores the previous CRTC configuration and drops master on exit.
// Takes DRM master on the card driving the connected output, does a real
// modeset, a legacy page flip with a completion event, and an atomic commit
// with OUT_FENCE_PTR. Restores the previous CRTC configuration and drops
// master before exiting, whatever happens.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <drm_fourcc.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

static int fd = -1;
static drmModeCrtc *saved = NULL;
static uint32_t saved_conn = 0;

static void restore(void) {
    if (saved && fd >= 0) {
        int r = drmModeSetCrtc(fd, saved->crtc_id, saved->buffer_id, saved->x, saved->y,
                               &saved_conn, 1, saved->mode_valid ? &saved->mode : NULL);
        printf("restore: drmModeSetCrtc -> %d (%s)\n", r, r ? strerror(errno) : "ok");
        drmModeFreeCrtc(saved); saved = NULL;
    }
    if (fd >= 0) { drmDropMaster(fd); printf("restore: master dropped\n"); }
}

struct dumb { uint32_t handle, pitch, fb; uint64_t size; void *map; };

static int make_fb(int w, int h, uint32_t color, struct dumb *d) {
    struct drm_mode_create_dumb creq = { .width = w, .height = h, .bpp = 32 };
    if (drmIoctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &creq)) return -1;
    d->handle = creq.handle; d->pitch = creq.pitch; d->size = creq.size;
    uint32_t handles[4] = { d->handle }, pitches[4] = { d->pitch }, offsets[4] = { 0 };
    if (drmModeAddFB2(fd, w, h, DRM_FORMAT_XRGB8888, handles, pitches, offsets, &d->fb, 0))
        return -2;
    struct drm_mode_map_dumb mreq = { .handle = d->handle };
    if (drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mreq)) return -3;
    d->map = mmap(0, d->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, mreq.offset);
    if (d->map == MAP_FAILED) return -4;
    for (uint64_t i = 0; i < d->size / 4; i++) ((uint32_t *)d->map)[i] = color;
    return 0;
}

static void page_flip_handler(int f, unsigned seq, unsigned sec, unsigned usec, void *data) {
    (void)f; (void)data;
    printf("P3-1/P3-2: page-flip COMPLETION from the kernel: seq=%u at %u.%06u\n", seq, sec, usec);
    *(int *)data = 1;
}

int main(int argc, char **argv) {
    const char *card = argc > 1 ? argv[1] : "/dev/dri/card1";
    fd = open(card, O_RDWR | O_CLOEXEC);
    if (fd < 0) { perror("open"); return 1; }
    printf("card: %s\n", card);

    if (drmSetMaster(fd)) { printf("FATAL: drmSetMaster: %s\n", strerror(errno)); return 1; }
    printf("master: acquired\n");
    atexit(restore);

    drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1);
    int atomic_ok = drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0;
    printf("atomic client cap: %s\n", atomic_ok ? "yes" : "no");

    drmModeRes *res = drmModeGetResources(fd);
    if (!res) { printf("FATAL: drmModeGetResources: %s\n", strerror(errno)); return 1; }

    drmModeConnector *conn = NULL;
    for (int i = 0; i < res->count_connectors && !conn; i++) {
        drmModeConnector *c = drmModeGetConnector(fd, res->connectors[i]);
        if (c && c->connection == DRM_MODE_CONNECTED && c->count_modes > 0) conn = c;
        else if (c) drmModeFreeConnector(c);
    }
    if (!conn) { printf("FATAL: no connected connector with modes\n"); return 1; }
    drmModeModeInfo mode = conn->modes[0];
    printf("connector %u: %s %ux%u@%u\n", conn->connector_id, mode.name,
           mode.hdisplay, mode.vdisplay, mode.vrefresh);

    drmModeEncoder *enc = drmModeGetEncoder(fd, conn->encoder_id ? conn->encoder_id : conn->encoders[0]);
    if (!enc) { printf("FATAL: no encoder\n"); return 1; }
    uint32_t crtc_id = enc->crtc_id;
    if (!crtc_id) for (int i = 0; i < res->count_crtcs && !crtc_id; i++)
        if (enc->possible_crtcs & (1 << i)) crtc_id = res->crtcs[i];
    printf("crtc: %u\n", crtc_id);

    saved = drmModeGetCrtc(fd, crtc_id);
    saved_conn = conn->connector_id;

    struct dumb a = {0}, b = {0};
    int r = make_fb(mode.hdisplay, mode.vdisplay, 0x00202060, &a);
    if (r) { printf("FATAL: framebuffer A: step %d: %s\n", r, strerror(errno)); return 1; }
    r = make_fb(mode.hdisplay, mode.vdisplay, 0x00206020, &b);
    if (r) { printf("FATAL: framebuffer B: step %d: %s\n", r, strerror(errno)); return 1; }
    printf("dumb buffers + ADDFB2: ok (fb %u, fb %u)\n", a.fb, b.fb);

    if (drmModeSetCrtc(fd, crtc_id, a.fb, 0, 0, &saved_conn, 1, &mode)) {
        printf("FATAL: modeset: %s\n", strerror(errno)); return 1;
    }
    printf("modeset: ACCEPTED -- fb %u is on screen\n", a.fb);

    int done = 0;
    if (drmModePageFlip(fd, crtc_id, b.fb, DRM_MODE_PAGE_FLIP_EVENT, &done)) {
        printf("P3-1: page flip REJECTED: %s\n", strerror(errno));
    } else {
        printf("P3-1: page flip ACCEPTED (fb %u), waiting for the kernel event...\n", b.fb);
        drmEventContext ev = { .version = 2, .page_flip_handler = page_flip_handler };
        struct pollfd pfd = { .fd = fd, .events = POLLIN };
        if (poll(&pfd, 1, 2000) > 0) drmHandleEvent(fd, &ev);
        if (!done) printf("P3-2: NO completion event within 2s\n");
    }

    if (atomic_ok) {
        drmModeAtomicReq *req = drmModeAtomicAlloc();
        drmModeObjectProperties *props = drmModeObjectGetProperties(fd, crtc_id, DRM_MODE_OBJECT_CRTC);
        uint32_t out_fence_prop = 0;
        for (uint32_t i = 0; props && i < props->count_props; i++) {
            drmModePropertyRes *p = drmModeGetProperty(fd, props->props[i]);
            if (p && !strcmp(p->name, "OUT_FENCE_PTR")) out_fence_prop = p->prop_id;
            if (p) drmModeFreeProperty(p);
        }
        printf("OUT_FENCE_PTR property: %s\n", out_fence_prop ? "present" : "ABSENT");
        if (out_fence_prop) {
            int64_t out_fence = -1;
            drmModeAtomicAddProperty(req, crtc_id, out_fence_prop, (uint64_t)(uintptr_t)&out_fence);
            drmModeObjectProperties *pp = drmModeObjectGetProperties(fd, res->crtcs[0], DRM_MODE_OBJECT_CRTC);
            (void)pp;
            uint32_t plane_id = 0;
            drmModePlaneRes *pres = drmModeGetPlaneResources(fd);
            for (uint32_t i = 0; pres && i < pres->count_planes && !plane_id; i++) {
                drmModePlane *pl = drmModeGetPlane(fd, pres->planes[i]);
                if (pl && (pl->possible_crtcs & (1 << 0)) && pl->fb_id) plane_id = pl->plane_id;
                if (pl) drmModeFreePlane(pl);
            }
            if (plane_id) {
                drmModeObjectProperties *plp = drmModeObjectGetProperties(fd, plane_id, DRM_MODE_OBJECT_PLANE);
                for (uint32_t i = 0; plp && i < plp->count_props; i++) {
                    drmModePropertyRes *p = drmModeGetProperty(fd, plp->props[i]);
                    if (p && !strcmp(p->name, "FB_ID"))
                        drmModeAtomicAddProperty(req, plane_id, p->prop_id, a.fb);
                    if (p) drmModeFreeProperty(p);
                }
                int ar = drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_PAGE_FLIP_EVENT, &done);
                printf("P3-4: atomic commit with OUT_FENCE_PTR -> %d (%s), out_fence fd=%ld\n",
                       ar, ar ? strerror(errno) : "ok", (long)out_fence);
                if (!ar) {
                    struct pollfd pfd2 = { .fd = fd, .events = POLLIN };
                    drmEventContext ev = { .version = 2, .page_flip_handler = page_flip_handler };
                    if (poll(&pfd2, 1, 2000) > 0) drmHandleEvent(fd, &ev);
                    if (out_fence >= 0) close((int)out_fence);
                }
            } else printf("P3-4: no primary plane with an fb found; atomic step skipped\n");
        }
        drmModeAtomicFree(req);
    }

    printf("probe: done\n");
    return 0;
}
