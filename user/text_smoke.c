/*
 * Software rendering stack smoke test for PeetOS: fontconfig finds a font,
 * freetype rasterizes it, pixman composites the glyphs (and a scaled copy),
 * and the frame is shown through DRM/KMS. Dynamically linked against
 * libfontconfig, libfreetype, libpixman-1 and libdrm.
 *
 * The kernel copies the DRM scanout buffer to the VESA framebuffer, so what
 * is on screen is checked by reading /dev/fb0.
 *
 * Output: "[text] <check> ok", "[text] FAIL ...", and "[text] pass".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include <drm_fourcc.h>
#include <fontconfig/fontconfig.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include <pixman.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

#define FBIOGETINFO 0x5001
struct peet_fb_info {
    uint32_t width, height, pitch, bpp, format;
    uint64_t map_size;
};

#define BG 0xff1d2733u
#define FG 0xffe8eef4u
#define ACCENT 0xff5fb3f0u

static int failures;

static void check(int cond, const char *what) {
    if (cond) {
        printf("[text] %s ok\n", what);
    } else {
        printf("[text] FAIL %s (errno %d)\n", what, errno);
        failures++;
    }
}

/* Draw `text` at baseline (x, y) into dst with pixman: one a8 mask per
 * glyph, composited OVER from a solid fill. Returns the pen x position. */
static int draw_text(pixman_image_t *dst, FT_Face face, const char *text, int x, int y, uint32_t argb) {
    pixman_color_t c = {
        .red = (uint16_t)(((argb >> 16) & 0xff) * 257),
        .green = (uint16_t)(((argb >> 8) & 0xff) * 257),
        .blue = (uint16_t)((argb & 0xff) * 257),
        .alpha = 0xffff,
    };
    pixman_image_t *solid = pixman_image_create_solid_fill(&c);
    for (const char *p = text; *p; p++) {
        if (FT_Load_Char(face, (unsigned char)*p, FT_LOAD_RENDER) != 0) {
            continue;
        }
        FT_GlyphSlot g = face->glyph;
        FT_Bitmap *bm = &g->bitmap;
        if (bm->width > 0 && bm->rows > 0 && bm->pixel_mode == FT_PIXEL_MODE_GRAY) {
            /* pixman a8 rows must be 4-byte aligned: copy into a padded buffer. */
            int stride = (int)((bm->width + 3) & ~3u);
            uint8_t *buf = calloc((size_t)stride * bm->rows, 1);
            for (unsigned r = 0; r < bm->rows; r++) {
                memcpy(buf + (size_t)r * stride, bm->buffer + (size_t)r * bm->pitch, bm->width);
            }
            pixman_image_t *mask = pixman_image_create_bits(PIXMAN_a8, (int)bm->width, (int)bm->rows,
                                                            (uint32_t *)buf, stride);
            pixman_image_composite32(PIXMAN_OP_OVER, solid, mask, dst, 0, 0, 0, 0,
                                     x + g->bitmap_left, y - g->bitmap_top, (int)bm->width, (int)bm->rows);
            pixman_image_unref(mask);
            free(buf);
        }
        x += (int)(g->advance.x >> 6);
    }
    pixman_image_unref(solid);
    return x;
}

static int count_not(pixman_image_t *img, int x0, int y0, int w, int h, uint32_t bg) {
    uint32_t *bits = pixman_image_get_data(img);
    int stride = pixman_image_get_stride(img) / 4;
    int n = 0;
    for (int y = y0; y < y0 + h; y++) {
        for (int x = x0; x < x0 + w; x++) {
            n += (bits[(size_t)y * stride + x] | 0xff000000u) != bg;
        }
    }
    return n;
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);

    /* --- fontconfig --------------------------------------------------- */
    check(FcInit(), "FcInit");
    FcPattern *pat = FcNameParse((const FcChar8 *)"monospace");
    FcConfigSubstitute(NULL, pat, FcMatchPattern);
    FcDefaultSubstitute(pat);
    FcResult result;
    FcPattern *match = FcFontMatch(NULL, pat, &result);
    FcChar8 *file = NULL, *family = NULL;
    if (match) {
        FcPatternGetString(match, FC_FILE, 0, &file);
        FcPatternGetString(match, FC_FAMILY, 0, &family);
    }
    printf("[text] monospace -> %s (%s)\n", family ? (char *)family : "?", file ? (char *)file : "?");
    check(file && strstr((char *)file, "DejaVuSansMono.ttf") && family && strcmp((char *)family, "DejaVu Sans Mono") == 0,
          "fontconfig matches monospace to DejaVu Sans Mono");
    FcPattern *sans = FcNameParse((const FcChar8 *)"sans-serif");
    FcConfigSubstitute(NULL, sans, FcMatchPattern);
    FcDefaultSubstitute(sans);
    FcPattern *sans_match = FcFontMatch(NULL, sans, &result);
    FcChar8 *sans_file = NULL;
    if (sans_match) {
        FcPatternGetString(sans_match, FC_FILE, 0, &sans_file);
    }
    check(sans_file && strstr((char *)sans_file, "DejaVuSans.ttf"), "fontconfig matches sans-serif to DejaVu Sans");
    FcObjectSet *os = FcObjectSetBuild(FC_FAMILY, NULL);
    FcPattern *all = FcPatternCreate();
    FcFontSet *set = FcFontList(NULL, all, os);
    check(set && set->nfont >= 2, "FcFontList sees the staged fonts");

    /* --- freetype ------------------------------------------------------ */
    FT_Library ft;
    FT_Face mono = NULL, sans_face = NULL;
    check(FT_Init_FreeType(&ft) == 0, "FT_Init_FreeType");
    check(file && FT_New_Face(ft, (char *)file, 0, &mono) == 0 && mono->num_glyphs > 1000 &&
              strcmp(mono->family_name, "DejaVu Sans Mono") == 0,
          "freetype opens the face");
    if (!mono) {
        return 1;
    }
    if (sans_file) {
        FT_New_Face(ft, (char *)sans_file, 0, &sans_face);
    }
    FT_Set_Pixel_Sizes(mono, 0, 48);
    int coverage = 0;
    if (FT_Load_Char(mono, 'A', FT_LOAD_RENDER) == 0) {
        FT_Bitmap *bm = &mono->glyph->bitmap;
        for (unsigned r = 0; r < bm->rows; r++) {
            for (unsigned c = 0; c < bm->width; c++) {
                coverage += bm->buffer[r * bm->pitch + c] > 128;
            }
        }
        printf("[text] 'A' at 48px: %ux%u, %d covered pixels\n", bm->width, bm->rows, coverage);
    }
    check(coverage > 200, "freetype rasterizes an antialiased glyph");

    /* --- pixman: compose a frame in memory ------------------------------ */
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    drmModeResPtr res = fd >= 0 ? drmModeGetResources(fd) : NULL;
    drmModeConnectorPtr conn = res ? drmModeGetConnector(fd, res->connectors[0]) : NULL;
    check(conn && conn->count_modes > 0, "DRM connector for presentation");
    if (!conn) {
        return 1;
    }
    drmModeModeInfo mode = conn->modes[0];
    int w = mode.hdisplay, h = mode.vdisplay;
    uint32_t handle, pitch, fb_id;
    uint64_t size, offset;
    check(drmModeCreateDumbBuffer(fd, w, h, 32, 0, &handle, &pitch, &size) == 0 &&
              drmModeMapDumbBuffer(fd, handle, &offset) == 0,
          "DRM dumb buffer");
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)offset);
    pixman_image_t *screen = pixman_image_create_bits(PIXMAN_x8r8g8b8, w, h, pixels, (int)pitch);
    check(screen != NULL, "pixman wraps the scanout buffer");

    pixman_color_t bgc = {0x1d1d, 0x2727, 0x3333, 0xffff};
    pixman_box32_t full = {0, 0, w, h};
    pixman_image_fill_boxes(PIXMAN_OP_SRC, screen, &bgc, 1, &full);
    pixman_color_t barc = {0x5f5f, 0xb3b3, 0xf0f0, 0xffff};
    pixman_box32_t bar = {0, 0, w, 6};
    pixman_image_fill_boxes(PIXMAN_OP_SRC, screen, &barc, 1, &bar);

    FT_Set_Pixel_Sizes(mono, 0, 40);
    int end = draw_text(screen, mono, "PeetOS renders text", 40, 90, FG);
    check(count_not(screen, 40, 50, end - 40, 50, BG) > 2000, "pixman composites glyph masks");
    FT_Set_Pixel_Sizes(mono, 0, 20);
    draw_text(screen, mono, "fontconfig -> freetype -> pixman -> DRM/KMS", 40, 140, ACCENT);
    if (sans_face) {
        FT_Set_Pixel_Sizes(sans_face, 0, 20);
        draw_text(screen, sans_face, "DejaVu Sans: The quick brown fox jumps over the lazy dog.", 40, 180, FG);
    }
    FT_Set_Pixel_Sizes(mono, 0, 16);
    draw_text(screen, mono, "0123456789 !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~", 40, 216, FG);

    /* A 3x bilinear upscale of a small text image: exercises pixman's
     * transforms and filters. */
    pixman_image_t *small = pixman_image_create_bits(PIXMAN_a8r8g8b8, 160, 24, NULL, 0);
    pixman_color_t clear = {0, 0, 0, 0};
    pixman_box32_t sbox = {0, 0, 160, 24};
    pixman_image_fill_boxes(PIXMAN_OP_SRC, small, &clear, 1, &sbox);
    FT_Set_Pixel_Sizes(mono, 0, 16);
    draw_text(small, mono, "scaled 3x", 4, 18, ACCENT);
    pixman_transform_t xf;
    pixman_transform_init_scale(&xf, pixman_double_to_fixed(1.0 / 3), pixman_double_to_fixed(1.0 / 3));
    pixman_image_set_transform(small, &xf);
    pixman_image_set_filter(small, PIXMAN_FILTER_BILINEAR, NULL, 0);
    pixman_image_composite32(PIXMAN_OP_OVER, small, NULL, screen, 0, 0, 0, 0, 40, 240, 480, 72);
    check(count_not(screen, 40, 240, 480, 72, BG) > 2000, "pixman scaled bilinear composite");

    /* --- present through DRM and verify via /dev/fb0 ------------------- */
    uint32_t handles[4] = {handle}, pitches[4] = {pitch}, offsets[4] = {0};
    check(drmModeAddFB2(fd, w, h, DRM_FORMAT_XRGB8888, handles, pitches, offsets, &fb_id, 0) == 0 &&
              drmModeSetCrtc(fd, res->crtcs[0], fb_id, 0, 0, &conn->connector_id, 1, &mode) == 0,
          "present through DRM");
    int fbfd = open("/dev/fb0", O_RDONLY);
    struct peet_fb_info fbi;
    int on_screen = 0;
    if (fbfd >= 0 && ioctl(fbfd, FBIOGETINFO, &fbi) == 0) {
        uint32_t *fbm = mmap(NULL, fbi.map_size, PROT_READ, MAP_SHARED, fbfd, 0);
        if (fbm != MAP_FAILED) {
            int stride = (int)(fbi.pitch / 4);
            for (int y = 50; y < 100; y++) {
                for (int x = 40; x < end; x++) {
                    on_screen += (fbm[(size_t)y * stride + x] | 0xff000000u) != BG;
                }
            }
        }
    }
    check(on_screen > 2000, "rendered text is on screen");
    printf("[text] frame on screen\n");
    sleep(3); /* time for the runner's screendump */

    if (failures) {
        printf("[text] failures=%d\n", failures);
        return 1;
    }
    printf("[text] pass\n");
    return 0;
}
