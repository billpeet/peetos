/*
 * 2D toolkit smoke test for PeetOS: GLib (main loop, threads, GRegex/pcre2,
 * GObject, GIO), cairo (vector drawing, PNG round trip through libpng/zlib,
 * PDF and SVG output) and pango (markup, wrapping, bidi, font resolution
 * through fontconfig/harfbuzz). The last step draws a frame with cairo and
 * pango into a DRM dumb buffer and checks it on screen through /dev/fb0.
 *
 * Output: "[pango] <check> ok", "[pango] FAIL ...", and "[pango] pass".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <cairo-pdf.h>
#include <cairo-svg.h>
#include <cairo.h>
#include <drm_fourcc.h>
#include <gio/gio.h>
#include <glib-object.h>
#include <glib.h>
#include <pango/pangocairo.h>
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
        printf("[pango] %s ok\n", what);
    } else {
        printf("[pango] FAIL %s (errno %d)\n", what, errno);
        failures++;
    }
}

/* --- GLib ------------------------------------------------------------------ */

static gboolean on_timeout(gpointer data) {
    int *fired = data;
    (*fired)++;
    return *fired < 3 ? G_SOURCE_CONTINUE : G_SOURCE_REMOVE;
}

static gboolean quit_when_done(gpointer data) {
    GMainLoop *loop = data;
    g_main_loop_quit(loop);
    return G_SOURCE_REMOVE;
}

typedef struct {
    GMutex lock;
    int total;
} Counter;

static gpointer thread_body(gpointer data) {
    Counter *c = data;
    for (int i = 0; i < 10000; i++) {
        g_mutex_lock(&c->lock);
        c->total++;
        g_mutex_unlock(&c->lock);
    }
    return NULL;
}

static void notify_cb(GObject *obj, GParamSpec *pspec, gpointer data) {
    (void)obj;
    (void)pspec;
    (*(int *)data)++;
}

static void test_glib(void) {
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);
    int fired = 0;
    g_timeout_add(10, on_timeout, &fired);
    g_timeout_add(200, quit_when_done, loop);
    g_main_loop_run(loop);
    g_main_loop_unref(loop);
    check(fired == 3, "GMainLoop runs timeout sources");

    Counter c = {.total = 0};
    g_mutex_init(&c.lock);
    GThread *threads[4];
    for (int i = 0; i < 4; i++) threads[i] = g_thread_new("worker", thread_body, &c);
    for (int i = 0; i < 4; i++) g_thread_join(threads[i]);
    check(c.total == 40000, "GThread workers with GMutex");

    GError *err = NULL;
    GRegex *re = g_regex_new("(\\w+)@(\\w+)\\.org", 0, 0, &err);
    GMatchInfo *mi = NULL;
    gchar *user = NULL;
    if (re && g_regex_match(re, "mail peet@peetos.org today", 0, &mi)) user = g_match_info_fetch(mi, 1);
    check(user && strcmp(user, "peet") == 0, "GRegex (pcre2) captures a group");
    g_free(user);
    g_match_info_free(mi);
    if (re) g_regex_unref(re);

    GHashTable *h = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
    for (int i = 0; i < 500; i++) g_hash_table_insert(h, g_strdup_printf("k%d", i), GINT_TO_POINTER(i));
    check(g_hash_table_size(h) == 500 && GPOINTER_TO_INT(g_hash_table_lookup(h, "k123")) == 123,
          "GHashTable");
    g_hash_table_destroy(h);

    const char *arabic = "\xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7"; /* "marhaba" */
    check(g_utf8_validate(arabic, -1, NULL) && g_utf8_strlen(arabic, -1) == 5, "g_utf8 validation");

    /* GObject: a property change emits notify. GApplication is a convenient
     * GObject with a writable property. */
    GApplication *app = g_application_new("org.peetos.PangoSmoke", G_APPLICATION_NON_UNIQUE);
    int notified = 0;
    g_signal_connect(app, "notify::inactivity-timeout", G_CALLBACK(notify_cb), &notified);
    g_application_set_inactivity_timeout(app, 1234);
    check(G_IS_OBJECT(app) && notified == 1 && g_application_get_inactivity_timeout(app) == 1234,
          "GObject property notify");
    g_object_unref(app);

    GFile *f = g_file_new_for_path("/etc/fonts/fonts.conf");
    gchar *contents = NULL;
    gsize len = 0;
    gboolean loaded = g_file_load_contents(f, NULL, &contents, &len, NULL, &err);
    check(loaded && len > 100 && strstr(contents, "<fontconfig>") != NULL, "GIO GFile reads a file");
    g_free(contents);

    /* statx(2): raw syscall against stat(2), then GIO's metadata path
     * (g_file_query_info uses statx when the kernel has it). */
    struct stat st;
    unsigned char sx[256];
    memset(sx, 0, sizeof sx);
    long rc = syscall(332 /* SYS_statx */, AT_FDCWD, "/etc/fonts/fonts.conf", 0, 0x7ffu, sx);
    uint32_t sx_mask, sx_nlink;
    uint16_t sx_mode;
    uint64_t sx_ino, sx_size;
    memcpy(&sx_mask, sx + 0x00, 4);
    memcpy(&sx_nlink, sx + 0x10, 4);
    memcpy(&sx_mode, sx + 0x1c, 2);
    memcpy(&sx_ino, sx + 0x20, 8);
    memcpy(&sx_size, sx + 0x28, 8);
    check(rc == 0 && stat("/etc/fonts/fonts.conf", &st) == 0 && (sx_mask & 0x7ff) == 0x7ff &&
              sx_size == (uint64_t)st.st_size && sx_ino == st.st_ino && sx_mode == (uint16_t)st.st_mode &&
              sx_nlink == st.st_nlink,
          "statx matches stat");
    GFileInfo *info = g_file_query_info(f, G_FILE_ATTRIBUTE_STANDARD_SIZE "," G_FILE_ATTRIBUTE_STANDARD_TYPE,
                                        G_FILE_QUERY_INFO_NONE, NULL, NULL);
    check(info && g_file_info_get_size(info) == st.st_size && g_file_info_get_file_type(info) == G_FILE_TYPE_REGULAR,
          "GIO g_file_query_info size and type");
    if (info) g_object_unref(info);
    g_object_unref(f);
    if (err) g_error_free(err);
}

/* --- cairo ---------------------------------------------------------------- */

static uint32_t pixel(cairo_surface_t *s, int x, int y) {
    cairo_surface_flush(s);
    unsigned char *data = cairo_image_surface_get_data(s);
    int stride = cairo_image_surface_get_stride(s);
    return *(uint32_t *)(data + (size_t)y * stride + (size_t)x * 4);
}

static long file_size(const char *path, char *head, size_t head_len) {
    struct stat st;
    if (stat(path, &st) != 0) return -1;
    FILE *fp = fopen(path, "rb");
    if (!fp) return -1;
    size_t n = fread(head, 1, head_len - 1, fp);
    head[n] = 0;
    fclose(fp);
    return (long)st.st_size;
}

static void test_cairo(void) {
    cairo_surface_t *s = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, 200, 100);
    cairo_t *cr = cairo_create(s);
    cairo_set_source_rgb(cr, 1, 1, 1);
    cairo_paint(cr);
    cairo_pattern_t *grad = cairo_pattern_create_linear(0, 0, 200, 0);
    cairo_pattern_add_color_stop_rgb(grad, 0, 1, 0, 0);
    cairo_pattern_add_color_stop_rgb(grad, 1, 0, 0, 1);
    cairo_rectangle(cr, 0, 0, 200, 20);
    cairo_set_source(cr, grad);
    cairo_fill(cr);
    cairo_pattern_destroy(grad);
    cairo_arc(cr, 100, 60, 30, 0, 2 * G_PI);
    cairo_set_source_rgb(cr, 0, 0.5, 0);
    cairo_fill(cr);
    cairo_destroy(cr);

    uint32_t left = pixel(s, 1, 10), right = pixel(s, 198, 10), centre = pixel(s, 100, 60);
    /* Along the 45-degree diagonal the rim crosses pixels partially, so some
     * pixel there must be a blend of green and white. */
    int edge = 0;
    for (int d = 18; d <= 24; d++) {
        uint32_t p = pixel(s, 100 + d, 60 + d) & 0xffffff;
        edge += p != 0x008000 && p != 0xffffff;
    }
    check((left & 0xff0000) > 0xf00000 && (left & 0xff) < 0x10 && (right & 0xff) > 0xf0 &&
              (right & 0xff0000) < 0x100000,
          "cairo linear gradient");
    check((centre & 0xffffff) == 0x008000, "cairo fills a circle");
    check(edge > 0, "cairo antialiases edges");

    check(cairo_surface_write_to_png(s, "/tmp/pango_smoke.png") == CAIRO_STATUS_SUCCESS,
          "cairo writes a PNG (libpng + zlib)");
    cairo_surface_t *back = cairo_image_surface_create_from_png("/tmp/pango_smoke.png");
    int same = cairo_surface_status(back) == CAIRO_STATUS_SUCCESS && cairo_image_surface_get_width(back) == 200;
    for (int y = 0; same && y < 100; y += 7) {
        for (int x = 0; x < 200; x += 5) {
            if (pixel(back, x, y) != pixel(s, x, y)) {
                same = 0;
                break;
            }
        }
    }
    check(same, "PNG round trip preserves pixels");
    cairo_surface_destroy(back);
    cairo_surface_destroy(s);

    cairo_surface_t *pdf = cairo_pdf_surface_create("/tmp/pango_smoke.pdf", 300, 200);
    cairo_t *pc = cairo_create(pdf);
    PangoLayout *pl = pango_cairo_create_layout(pc);
    pango_layout_set_text(pl, "PDF from cairo and pango on PeetOS", -1);
    pango_cairo_show_layout(pc, pl);
    g_object_unref(pl);
    cairo_destroy(pc);
    cairo_surface_destroy(pdf);
    char head[16];
    long pdf_size = file_size("/tmp/pango_smoke.pdf", head, sizeof head);
    printf("[pango] PDF is %ld bytes\n", pdf_size);
    check(pdf_size > 1000 && strncmp(head, "%PDF-", 5) == 0, "cairo PDF surface with embedded font");

    cairo_surface_t *svg = cairo_svg_surface_create("/tmp/pango_smoke.svg", 100, 100);
    cairo_t *sc = cairo_create(svg);
    cairo_rectangle(sc, 10, 10, 80, 80);
    cairo_fill(sc);
    cairo_destroy(sc);
    cairo_surface_destroy(svg);
    long svg_size = file_size("/tmp/pango_smoke.svg", head, sizeof head);
    check(svg_size > 100 && strncmp(head, "<?xml", 5) == 0, "cairo SVG surface");
}

/* --- pango ---------------------------------------------------------------- */

static void test_pango(void) {
    PangoFontMap *fm = pango_cairo_font_map_get_default();
    PangoContext *ctx = pango_font_map_create_context(fm);
    PangoFontDescription *desc = pango_font_description_from_string("Sans 20");
    PangoFont *font = pango_font_map_load_font(fm, ctx, desc);
    PangoFontDescription *got = font ? pango_font_describe(font) : NULL;
    const char *family = got ? pango_font_description_get_family(got) : NULL;
    printf("[pango] \"Sans 20\" -> %s\n", family ? family : "?");
    check(family && strcmp(family, "DejaVu Sans") == 0, "pango resolves Sans through fontconfig");
    if (got) pango_font_description_free(got);
    if (font) g_object_unref(font);

    PangoLayout *layout = pango_layout_new(ctx);
    pango_layout_set_font_description(layout, desc);
    pango_layout_set_markup(layout, "<b>Bold</b> and <i>italic</i> text", -1);
    PangoAttrList *attrs = pango_layout_get_attributes(layout);
    int n_attrs = 0;
    if (attrs) {
        GSList *list = pango_attr_iterator_get_attrs(pango_attr_list_get_iterator(attrs));
        n_attrs = (int)g_slist_length(list);
        g_slist_free_full(list, (GDestroyNotify)pango_attribute_destroy);
    }
    check(attrs != NULL && n_attrs >= 1 && strcmp(pango_layout_get_text(layout), "Bold and italic text") == 0,
          "pango parses markup");
    /* pango shapes with harfbuzz on a face it loads from the font file; if
     * that load fails every glyph becomes a hex box ("unknown glyph"). */
    int unknown_latin = pango_layout_get_unknown_glyphs_count(layout);
    pango_layout_set_text(layout, "\xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7", -1);
    int unknown_arabic = pango_layout_get_unknown_glyphs_count(layout);
    printf("[pango] unknown glyphs: latin %d, arabic %d\n", unknown_latin, unknown_arabic);
    check(unknown_latin == 0 && unknown_arabic == 0, "pango finds real glyphs for Latin and Arabic");

    pango_layout_set_text(layout,
                          "PeetOS lays out paragraphs with pango: this sentence is long enough to wrap "
                          "onto several lines at a narrow width.",
                          -1);
    PangoFontDescription *body = pango_font_description_from_string("Sans 12");
    pango_layout_set_font_description(layout, body);
    pango_font_description_free(body);
    pango_layout_set_width(layout, 300 * PANGO_SCALE);
    pango_layout_set_wrap(layout, PANGO_WRAP_WORD);
    int lines = pango_layout_get_line_count(layout);
    PangoRectangle ink, logical;
    pango_layout_get_pixel_extents(layout, &ink, &logical);
    printf("[pango] wrapped to %d lines, %dx%d px\n", lines, logical.width, logical.height);
    check(lines >= 3 && logical.width <= 300, "pango wraps a paragraph to a width");

    pango_layout_set_width(layout, -1);
    pango_layout_set_text(layout, "\xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7 hello", -1);
    PangoDirection dir = pango_find_base_dir(pango_layout_get_text(layout), -1);
    PangoLayoutLine *line = pango_layout_get_line_readonly(layout, 0);
    check(dir == PANGO_DIRECTION_RTL && line && line->resolved_dir == PANGO_DIRECTION_RTL,
          "pango resolves an Arabic paragraph to right-to-left (fribidi)");
    int runs = line ? (int)g_slist_length(line->runs) : 0;
    check(runs >= 2, "pango itemizes mixed-script text into runs");

    g_object_unref(layout);
    pango_font_description_free(desc);
    g_object_unref(ctx);
}

/* --- present a cairo + pango frame through DRM ------------------------------- */

static void rounded_rect(cairo_t *cr, double x, double y, double w, double h, double r) {
    cairo_new_sub_path(cr);
    cairo_arc(cr, x + w - r, y + r, r, -G_PI / 2, 0);
    cairo_arc(cr, x + w - r, y + h - r, r, 0, G_PI / 2);
    cairo_arc(cr, x + r, y + h - r, r, G_PI / 2, G_PI);
    cairo_arc(cr, x + r, y + r, r, G_PI, 3 * G_PI / 2);
    cairo_close_path(cr);
}

static void show_markup(cairo_t *cr, const char *font, const char *markup, double x, double y, int width,
                        double r, double g, double b) {
    PangoLayout *l = pango_cairo_create_layout(cr);
    PangoFontDescription *d = pango_font_description_from_string(font);
    pango_layout_set_font_description(l, d);
    pango_layout_set_markup(l, markup, -1);
    if (width > 0) {
        pango_layout_set_width(l, width * PANGO_SCALE);
        pango_layout_set_wrap(l, PANGO_WRAP_WORD);
    }
    cairo_move_to(cr, x, y);
    cairo_set_source_rgb(cr, r, g, b);
    pango_cairo_show_layout(cr, l);
    pango_font_description_free(d);
    g_object_unref(l);
}

static void test_present(void) {
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    drmModeResPtr res = fd >= 0 ? drmModeGetResources(fd) : NULL;
    drmModeConnectorPtr conn = res ? drmModeGetConnector(fd, res->connectors[0]) : NULL;
    check(conn && conn->count_modes > 0, "DRM connector for presentation");
    if (!conn) return;
    drmModeModeInfo mode = conn->modes[0];
    int w = mode.hdisplay, h = mode.vdisplay;
    uint32_t handle, pitch, fb_id;
    uint64_t size, offset;
    if (drmModeCreateDumbBuffer(fd, w, h, 32, 0, &handle, &pitch, &size) != 0 ||
        drmModeMapDumbBuffer(fd, handle, &offset) != 0) {
        check(0, "DRM dumb buffer");
        return;
    }
    unsigned char *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, (off_t)offset);
    cairo_surface_t *screen = cairo_image_surface_create_for_data(pixels, CAIRO_FORMAT_RGB24, w, h, (int)pitch);
    cairo_t *cr = cairo_create(screen);

    cairo_pattern_t *bg = cairo_pattern_create_linear(0, 0, 0, h);
    cairo_pattern_add_color_stop_rgb(bg, 0, 0.11, 0.15, 0.20);
    cairo_pattern_add_color_stop_rgb(bg, 1, 0.05, 0.07, 0.10);
    cairo_set_source(cr, bg);
    cairo_paint(cr);
    cairo_pattern_destroy(bg);

    rounded_rect(cr, 40, 40, w - 80, 300, 18);
    cairo_set_source_rgb(cr, 0.93, 0.95, 0.97);
    cairo_fill_preserve(cr);
    cairo_set_source_rgb(cr, 0.37, 0.70, 0.94);
    cairo_set_line_width(cr, 3);
    cairo_stroke(cr);

    show_markup(cr, "Sans Bold 30", "cairo + pango on PeetOS", 70, 60, 0, 0.11, 0.15, 0.20);
    show_markup(cr, "Sans 15",
                "Text laid out by <b>pango</b>, shaped by <i>harfbuzz</i>, rendered by "
                "<span foreground='#2c7fb8'>cairo</span> and presented through DRM/KMS. "
                "Paragraphs wrap at the card's width, and bidirectional text works: "
                "\xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7 \xd8\xa8\xd8\xa7\xd9\x84\xd8\xb9\xd8\xa7\xd9\x84\xd9\x85 "
                "(hello world).",
                70, 120, w - 140, 0.15, 0.18, 0.22);
    show_markup(cr, "Monospace 13", "GLib 2.82 \xc2\xb7 cairo 1.18 \xc2\xb7 pango 1.54 \xc2\xb7 libpng 1.6 \xc2\xb7 zlib 1.3",
                70, 290, 0, 0.35, 0.40, 0.45);

    /* A few antialiased shapes below the card. */
    for (int i = 0; i < 6; i++) {
        cairo_arc(cr, 100 + i * 150, 460, 50, 0, 2 * G_PI);
        cairo_set_source_rgba(cr, 0.37, 0.70, 0.94, 0.25 + i * 0.12);
        cairo_fill(cr);
    }
    cairo_destroy(cr);
    cairo_surface_flush(screen);

    uint32_t handles[4] = {handle}, pitches[4] = {pitch}, offsets[4] = {0};
    check(drmModeAddFB2(fd, w, h, DRM_FORMAT_XRGB8888, handles, pitches, offsets, &fb_id, 0) == 0 &&
              drmModeSetCrtc(fd, res->crtcs[0], fb_id, 0, 0, &conn->connector_id, 1, &mode) == 0,
          "present through DRM");

    /* The card (light) with dark text on it should be on screen. */
    int fbfd = open("/dev/fb0", O_RDONLY);
    struct peet_fb_info fbi;
    int light = 0, dark = 0;
    if (fbfd >= 0 && ioctl(fbfd, FBIOGETINFO, &fbi) == 0) {
        uint32_t *fbm = mmap(NULL, fbi.map_size, PROT_READ, MAP_SHARED, fbfd, 0);
        if (fbm != MAP_FAILED) {
            int stride = (int)(fbi.pitch / 4);
            for (int y = 60; y < 320; y++) {
                for (int x = 60; x < w - 60; x++) {
                    uint32_t p = fbm[(size_t)y * stride + x] & 0xffffff;
                    light += ((p >> 16) & 0xff) > 0xe0 && ((p >> 8) & 0xff) > 0xe0 && (p & 0xff) > 0xe0;
                    dark += ((p >> 16) & 0xff) < 0x60 && ((p >> 8) & 0xff) < 0x60;
                }
            }
        }
    }
    printf("[pango] on screen: %d card pixels, %d text pixels\n", light, dark);
    check(light > 100000 && dark > 3000, "cairo + pango frame is on screen");
    printf("[pango] frame on screen\n");
    sleep(3); /* time for the runner's screendump */
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("[pango] start (GLib %u.%u.%u, cairo %s, pango %s)\n", glib_major_version, glib_minor_version,
           glib_micro_version, cairo_version_string(), pango_version_string());
    test_glib();
    test_cairo();
    test_pango();
    test_present();
    if (failures) {
        printf("[pango] failures=%d\n", failures);
        return 1;
    }
    printf("[pango] pass\n");
    return 0;
}
