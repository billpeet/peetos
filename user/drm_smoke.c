/*
 * DRM/KMS smoke test for PeetOS's /dev/dri/card0, written against libdrm the
 * way a compositor's legacy KMS path uses it. Linked dynamically against
 * libdrm.so.2.
 *
 * The kernel copies the scanout buffer to the VESA framebuffer, so the test
 * checks what is "on screen" by reading /dev/fb0.
 *
 * Output: "[drm] <check> ok", "[drm] FAIL ...", and "[drm] pass".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <drm_fourcc.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

#define FBIOGETINFO 0x5001
struct peet_fb_info {
    uint32_t width, height, pitch, bpp, format;
    uint64_t map_size;
};

static int failures;

static void check(int cond, const char *what) {
    if (cond) {
        printf("[drm] %s ok\n", what);
    } else {
        printf("[drm] FAIL %s (errno %d)\n", what, errno);
        failures++;
    }
}

struct dumb {
    uint32_t handle, pitch, fb_id;
    uint64_t size;
    uint32_t *map;
};

static int make_dumb(int fd, uint32_t w, uint32_t h, struct dumb *d) {
    if (drmModeCreateDumbBuffer(fd, w, h, 32, 0, &d->handle, &d->pitch, &d->size) != 0) {
        return -1;
    }
    uint64_t offset = 0;
    if (drmModeMapDumbBuffer(fd, d->handle, &offset) != 0) {
        return -1;
    }
    d->map = mmap(NULL, d->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)offset);
    if (d->map == MAP_FAILED) {
        return -1;
    }
    uint32_t handles[4] = {d->handle}, pitches[4] = {d->pitch}, offsets[4] = {0};
    return drmModeAddFB2(fd, w, h, DRM_FORMAT_XRGB8888, handles, pitches, offsets, &d->fb_id, 0);
}

static void fill(struct dumb *d, uint32_t w, uint32_t h, uint32_t color) {
    for (uint32_t y = 0; y < h; y++) {
        uint32_t *row = (uint32_t *)((char *)d->map + (size_t)y * d->pitch);
        for (uint32_t x = 0; x < w; x++) {
            row[x] = color;
        }
    }
}

static uint32_t *screen;
static uint32_t screen_pitch;

static uint32_t pixel(uint32_t x, uint32_t y) {
    return screen[(size_t)y * (screen_pitch / 4) + x] & 0x00ffffff;
}

static uint64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000;
}

static uint64_t flip_data;
static unsigned flip_seq;
static void on_flip(int fd, unsigned seq, unsigned sec, unsigned usec, void *data) {
    (void)fd; (void)sec; (void)usec;
    flip_seq = seq;
    flip_data = (uint64_t)(uintptr_t)data;
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    check(fd >= 0, "open /dev/dri/card0");
    if (fd < 0) {
        return 1;
    }
    drmVersionPtr ver = drmGetVersion(fd);
    check(ver && strcmp(ver->name, "simpledrm") == 0, "drmGetVersion reports simpledrm");
    drmFreeVersion(ver);

    uint64_t cap = 0;
    check(drmGetCap(fd, DRM_CAP_DUMB_BUFFER, &cap) == 0 && cap == 1, "DRM_CAP_DUMB_BUFFER");
    check(drmGetCap(fd, DRM_CAP_PRIME, &cap) == 0 && cap == 3, "DRM_CAP_PRIME import+export");
    check(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) != 0, "atomic refused (legacy KMS only)");
    check(drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1) == 0, "universal planes");
    check(drmIsMaster(fd), "first opener is master");

    drmModeResPtr res = drmModeGetResources(fd);
    check(res && res->count_crtcs == 1 && res->count_connectors == 1 && res->count_encoders == 1,
          "drmModeGetResources: one crtc/connector/encoder");
    if (!res) {
        return 1;
    }
    uint32_t crtc_id = res->crtcs[0];
    drmModeConnectorPtr conn = drmModeGetConnector(fd, res->connectors[0]);
    check(conn && conn->connection == DRM_MODE_CONNECTED && conn->count_modes == 1 && conn->encoder_id == res->encoders[0],
          "connector connected with one mode");
    if (!conn || conn->count_modes < 1) {
        return 1;
    }
    drmModeModeInfo mode = conn->modes[0];
    uint32_t w = mode.hdisplay, h = mode.vdisplay;
    printf("[drm] mode %s %ux%u@%u\n", mode.name, w, h, mode.vrefresh);
    drmModeEncoderPtr enc = drmModeGetEncoder(fd, conn->encoder_id);
    check(enc && (enc->possible_crtcs & 1) && enc->crtc_id == crtc_id, "encoder drives the crtc");

    drmModePlaneResPtr planes = drmModeGetPlaneResources(fd);
    check(planes && planes->count_planes == 1, "one plane");
    if (planes && planes->count_planes == 1) {
        drmModePlanePtr plane = drmModeGetPlane(fd, planes->planes[0]);
        int has_xrgb = 0;
        for (uint32_t i = 0; plane && i < plane->count_formats; i++) {
            has_xrgb |= plane->formats[i] == DRM_FORMAT_XRGB8888;
        }
        check(plane && has_xrgb && (plane->possible_crtcs & 1), "plane supports XRGB8888");
        drmModeObjectPropertiesPtr props =
            drmModeObjectGetProperties(fd, planes->planes[0], DRM_MODE_OBJECT_PLANE);
        int primary = 0;
        for (uint32_t i = 0; props && i < props->count_props; i++) {
            drmModePropertyPtr p = drmModeGetProperty(fd, props->props[i]);
            if (p && strcmp(p->name, "type") == 0 && props->prop_values[i] == DRM_PLANE_TYPE_PRIMARY) {
                primary = 1;
            }
            drmModeFreeProperty(p);
        }
        check(primary, "plane type property is primary");
        drmModeFreeObjectProperties(props);
        drmModeFreePlane(plane);
    }

    struct dumb a = {0}, b = {0};
    check(make_dumb(fd, w, h, &a) == 0 && make_dumb(fd, w, h, &b) == 0, "dumb buffers + mmap + addfb2");
    if (!a.map || !b.map || a.map == MAP_FAILED || b.map == MAP_FAILED) {
        return 1;
    }
    fill(&a, w, h, 0x00c03020);
    fill(&b, w, h, 0x002050d0);

    int fbfd = open("/dev/fb0", O_RDWR);
    struct peet_fb_info fbi;
    if (fbfd < 0 || ioctl(fbfd, FBIOGETINFO, &fbi) != 0) {
        printf("[drm] FAIL /dev/fb0 for readback\n");
        return 1;
    }
    screen_pitch = fbi.pitch;
    screen = mmap(NULL, fbi.map_size, PROT_READ, MAP_SHARED, fbfd, 0);
    check(screen != MAP_FAILED, "map /dev/fb0 for readback");

    check(drmModeSetCrtc(fd, crtc_id, a.fb_id, 0, 0, &res->connectors[0], 1, &mode) == 0, "drmModeSetCrtc");
    check(pixel(5, 5) == 0xc03020 && pixel(w - 1, h - 1) == 0xc03020, "buffer A is on screen");

    drmModeCrtcPtr crtc = drmModeGetCrtc(fd, crtc_id);
    check(crtc && crtc->buffer_id == a.fb_id && crtc->mode_valid && crtc->mode.hdisplay == w, "drmModeGetCrtc");
    drmModeFreeCrtc(crtc);

    /* Page flip to B with an event, then wait for it like a compositor. */
    uint64_t t0 = now_ms();
    check(drmModePageFlip(fd, crtc_id, b.fb_id, DRM_MODE_PAGE_FLIP_EVENT, (void *)(uintptr_t)0xb0b0) == 0,
          "drmModePageFlip");
    check(drmModePageFlip(fd, crtc_id, a.fb_id, 0, NULL) != 0 && errno == EBUSY, "second flip while pending is EBUSY");
    struct pollfd pfd = {.fd = fd, .events = POLLIN};
    int pr = poll(&pfd, 1, 1000);
    uint64_t waited = now_ms() - t0;
    drmEventContext ev = {.version = 2, .page_flip_handler = on_flip};
    check(pr == 1 && drmHandleEvent(fd, &ev) == 0 && flip_data == 0xb0b0 && flip_seq >= 1,
          "flip completion event via poll + drmHandleEvent");
    check(waited >= 10, "flip event paced to the 60 Hz frame");
    check(pixel(5, 5) == 0x2050d0, "buffer B is on screen after the flip");

    /* DIRTYFB: draw into the front buffer and flush one rectangle. */
    for (uint32_t y = 0; y < 8; y++) {
        for (uint32_t x = 0; x < 8; x++) {
            b.map[(size_t)y * (b.pitch / 4) + x] = 0x00ffffff;
        }
    }
    drmModeClip clip = {0, 0, 8, 8};
    check(drmModeDirtyFB(fd, b.fb_id, &clip, 1) == 0 && pixel(3, 3) == 0xffffff && pixel(9, 9) == 0x2050d0,
          "drmModeDirtyFB flushes the clip");

    /* PRIME: export B, see the same pixels through the dma-buf fd, and import it back. */
    int prime = -1;
    check(drmPrimeHandleToFD(fd, b.handle, DRM_CLOEXEC, &prime) == 0 && prime >= 0, "drmPrimeHandleToFD");
    uint32_t *pmap = mmap(NULL, b.size, PROT_READ, MAP_SHARED, prime, 0);
    check(pmap != MAP_FAILED && (pmap[0] & 0xffffff) == 0xffffff && (pmap[(size_t)100 * (b.pitch / 4)] & 0xffffff) == 0x2050d0,
          "exported buffer maps with the same contents");
    uint32_t again = 0;
    check(drmPrimeFDToHandle(fd, prime, &again) == 0 && again == b.handle, "re-import returns the same handle");

    int fd2 = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    uint32_t imported = 0, fb2 = 0;
    check(fd2 >= 0 && !drmIsMaster(fd2), "second opener is not master");
    check(drmPrimeFDToHandle(fd2, prime, &imported) == 0 && imported != 0, "import into another card fd");
    uint32_t handles[4] = {imported}, pitches[4] = {b.pitch}, offsets[4] = {0};
    check(drmModeAddFB2(fd2, w, h, DRM_FORMAT_XRGB8888, handles, pitches, offsets, &fb2, 0) == 0,
          "addfb2 on the imported buffer");
    check(drmModeSetCrtc(fd2, crtc_id, fb2, 0, 0, &res->connectors[0], 1, &mode) != 0 && errno == EACCES,
          "non-master modeset is EACCES");
    close(fd2);
    close(prime);

    check(drmModeRmFB(fd, a.fb_id) == 0 && drmModeDestroyDumbBuffer(fd, a.handle) == 0, "rmfb + destroy dumb");

    drmModeFreeEncoder(enc);
    drmModeFreeConnector(conn);
    drmModeFreeResources(res);
    close(fd);

    if (failures) {
        printf("[drm] failures=%d\n", failures);
        return 1;
    }
    printf("[drm] pass\n");
    return 0;
}
