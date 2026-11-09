/*
 * wl-demo: a small xdg-shell Wayland client for PeetOS.
 *
 * Opens one xdg_toplevel, draws a solid colour with a border (plus an accent
 * bar while the window is activated), and reacts to input:
 *   - left click toggles between the primary and alternate colour
 *   - every key press is logged with its keysym and UTF-8 text (translated
 *     with libxkbcommon from the compositor's XKB_V1 keymap); 'q' exits
 *   - xdg_toplevel.close (the compositor's close box) exits
 * With --animate it redraws continuously, paced by frame callbacks.
 * With --copy TEXT it takes the clipboard selection (as text) the first time
 * it gets keyboard focus and serves it to other clients; with --paste it reads
 * every text selection it is offered and logs it.
 *
 * Usage: wl-demo <title> <0xRRGGBB> [--size WxH] [--animate]
 *                [--copy TEXT] [--paste]
 *
 * Log markers are prefixed "[wl-demo:<title>]".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <poll.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>
#include "xdg-shell-client-protocol.h"

#define KEY_Q 16
#define BTN_LEFT 0x110
#define NUM_BUFFERS 2

struct buffer {
    struct wl_buffer *wl;
    uint32_t *pixels;
    int busy;
};

struct app {
    const char *title;
    uint32_t color;
    uint32_t alt_color;
    int use_alt;
    int width, height;
    int animate;
    int activated;
    int running;
    int configured;
    int frame_pending;
    int frames;
    int first_logged;
    uint32_t phase;

    struct wl_display *display;
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    struct wl_seat *seat;
    struct xdg_wm_base *wm_base;
    struct wl_pointer *pointer;
    struct wl_keyboard *keyboard;
    struct wl_data_device_manager *ddm;
    struct wl_data_device *data_device;
    /* clipboard */
    const char *copy_text;
    int copied;
    struct wl_data_source *source;
    int paste;
    struct wl_data_offer *pending_offer;
    int offer_mime; /* index into TEXT_MIMES, -1 if no text type */
    int paste_fd;
    char paste_buf[4096];
    size_t paste_len;
    struct xkb_context *xkb_ctx;
    struct xkb_keymap *keymap;
    struct xkb_state *xkb_state;

    struct wl_surface *surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *toplevel;

    int pool_fd;
    size_t pool_size;
    void *pool_data;
    struct wl_shm_pool *pool;
    struct buffer buffers[NUM_BUFFERS];
};

static void redraw(struct app *app);

/* ------------------------------------------------------------------ */
/* Buffers                                                             */
/* ------------------------------------------------------------------ */

static void buffer_release(void *data, struct wl_buffer *wl) {
    (void)wl;
    struct buffer *b = data;
    b->busy = 0;
}

static const struct wl_buffer_listener buffer_listener = {.release = buffer_release};

static int create_buffers(struct app *app) {
    size_t frame = (size_t)app->width * (size_t)app->height * 4;
    app->pool_size = frame * NUM_BUFFERS;
    app->pool_fd = memfd_create("wl-demo", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    if (app->pool_fd < 0 || ftruncate(app->pool_fd, (off_t)app->pool_size) != 0) {
        printf("[wl-demo:%s] FAIL shm pool errno=%d\n", app->title, errno);
        return -1;
    }
    app->pool_data =
        mmap(NULL, app->pool_size, PROT_READ | PROT_WRITE, MAP_SHARED, app->pool_fd, 0);
    if (app->pool_data == MAP_FAILED) {
        printf("[wl-demo:%s] FAIL mmap errno=%d\n", app->title, errno);
        return -1;
    }
    app->pool = wl_shm_create_pool(app->shm, app->pool_fd, (int32_t)app->pool_size);
    for (int i = 0; i < NUM_BUFFERS; i++) {
        struct buffer *b = &app->buffers[i];
        b->pixels = (uint32_t *)((uint8_t *)app->pool_data + frame * (size_t)i);
        b->wl = wl_shm_pool_create_buffer(app->pool, (int32_t)(frame * (size_t)i), app->width,
                                          app->height, app->width * 4, WL_SHM_FORMAT_XRGB8888);
        wl_buffer_add_listener(b->wl, &buffer_listener, b);
    }
    return 0;
}

static struct buffer *free_buffer(struct app *app) {
    for (int i = 0; i < NUM_BUFFERS; i++) {
        if (!app->buffers[i].busy) {
            return &app->buffers[i];
        }
    }
    return NULL;
}

static void paint(struct app *app, uint32_t *px) {
    uint32_t base = app->use_alt ? app->alt_color : app->color;
    uint32_t edge = ((base >> 1) & 0x7f7f7f) | 0x00000000;
    for (int y = 0; y < app->height; y++) {
        for (int x = 0; x < app->width; x++) {
            uint32_t c = base;
            if (x < 4 || y < 4 || x >= app->width - 4 || y >= app->height - 4) {
                c = edge;
            } else if (app->activated && y >= 8 && y < 14) {
                c = 0x00ffffff;
            } else if (app->animate && (uint32_t)x == app->phase % (uint32_t)app->width) {
                c = 0x00ffff00;
            }
            px[(size_t)y * app->width + x] = c | 0xff000000u;
        }
    }
}

/* ------------------------------------------------------------------ */
/* Frame pacing                                                        */
/* ------------------------------------------------------------------ */

static void frame_done(void *data, struct wl_callback *cb, uint32_t time) {
    (void)time;
    struct app *app = data;
    wl_callback_destroy(cb);
    app->frame_pending = 0;
    if (!app->first_logged) {
        app->first_logged = 1;
        printf("[wl-demo:%s] first frame presented color=0x%06x\n", app->title,
               (app->use_alt ? app->alt_color : app->color) & 0xffffff);
    }
    if (app->animate) {
        app->phase += 4;
        redraw(app);
    }
}

static const struct wl_callback_listener frame_listener = {.done = frame_done};

static void redraw(struct app *app) {
    if (!app->configured || app->frame_pending) {
        return;
    }
    struct buffer *b = free_buffer(app);
    if (b == NULL) {
        return;
    }
    paint(app, b->pixels);
    wl_surface_attach(app->surface, b->wl, 0, 0);
    wl_surface_damage_buffer(app->surface, 0, 0, app->width, app->height);
    struct wl_callback *cb = wl_surface_frame(app->surface);
    wl_callback_add_listener(cb, &frame_listener, app);
    app->frame_pending = 1;
    b->busy = 1;
    wl_surface_commit(app->surface);
    app->frames++;
}

/* ------------------------------------------------------------------ */
/* xdg-shell                                                           */
/* ------------------------------------------------------------------ */

static void wm_base_ping(void *data, struct xdg_wm_base *wm, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(wm, serial);
}

static const struct xdg_wm_base_listener wm_base_listener = {.ping = wm_base_ping};

static void xdg_surface_configure(void *data, struct xdg_surface *xs, uint32_t serial) {
    struct app *app = data;
    xdg_surface_ack_configure(xs, serial);
    if (!app->configured) {
        app->configured = 1;
        printf("[wl-demo:%s] configured %dx%d\n", app->title, app->width, app->height);
    }
    /* A reconfigure (e.g. activation change) needs a fresh frame even if one
     * is in flight: drop the pacing guard so the new state shows up. */
    app->frame_pending = 0;
    redraw(app);
}

static const struct xdg_surface_listener xdg_surface_listener = {
    .configure = xdg_surface_configure,
};

static void toplevel_configure(void *data, struct xdg_toplevel *t, int32_t w, int32_t h,
                               struct wl_array *states) {
    (void)t;
    (void)w;
    (void)h;
    struct app *app = data;
    int activated = 0;
    uint32_t *st;
    wl_array_for_each(st, states) {
        if (*st == XDG_TOPLEVEL_STATE_ACTIVATED) {
            activated = 1;
        }
    }
    if (activated != app->activated) {
        app->activated = activated;
        printf("[wl-demo:%s] %s\n", app->title, activated ? "activated" : "deactivated");
    }
    /* Fixed-size client: the requested size is ignored. */
}

static void toplevel_close(void *data, struct xdg_toplevel *t) {
    (void)t;
    struct app *app = data;
    printf("[wl-demo:%s] close requested\n", app->title);
    app->running = 0;
}

static void toplevel_configure_bounds(void *data, struct xdg_toplevel *t, int32_t w, int32_t h) {
    (void)data;
    (void)t;
    (void)w;
    (void)h;
}

static void toplevel_wm_capabilities(void *data, struct xdg_toplevel *t,
                                     struct wl_array *caps) {
    (void)data;
    (void)t;
    (void)caps;
}

static const struct xdg_toplevel_listener toplevel_listener = {
    .configure = toplevel_configure,
    .close = toplevel_close,
    .configure_bounds = toplevel_configure_bounds,
    .wm_capabilities = toplevel_wm_capabilities,
};

/* ------------------------------------------------------------------ */
/* Input                                                               */
/* ------------------------------------------------------------------ */

static void pointer_enter(void *data, struct wl_pointer *p, uint32_t serial,
                          struct wl_surface *s, wl_fixed_t x, wl_fixed_t y) {
    (void)p;
    (void)serial;
    (void)s;
    struct app *app = data;
    printf("[wl-demo:%s] pointer enter %d,%d\n", app->title, wl_fixed_to_int(x),
           wl_fixed_to_int(y));
}

static void pointer_leave(void *data, struct wl_pointer *p, uint32_t serial,
                          struct wl_surface *s) {
    (void)p;
    (void)serial;
    (void)s;
    struct app *app = data;
    printf("[wl-demo:%s] pointer leave\n", app->title);
}

static void pointer_motion(void *data, struct wl_pointer *p, uint32_t time, wl_fixed_t x,
                           wl_fixed_t y) {
    (void)data;
    (void)p;
    (void)time;
    (void)x;
    (void)y;
}

static void pointer_button(void *data, struct wl_pointer *p, uint32_t serial, uint32_t time,
                           uint32_t button, uint32_t state) {
    (void)p;
    (void)serial;
    (void)time;
    struct app *app = data;
    if (state != WL_POINTER_BUTTON_STATE_PRESSED) {
        return;
    }
    printf("[wl-demo:%s] button %u pressed\n", app->title, button);
    if (button == BTN_LEFT) {
        app->use_alt = !app->use_alt;
        printf("[wl-demo:%s] color now 0x%06x\n", app->title,
               (app->use_alt ? app->alt_color : app->color) & 0xffffff);
        app->frame_pending = 0;
        redraw(app);
    }
}

static void pointer_axis(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis,
                         wl_fixed_t value) {
    (void)data;
    (void)p;
    (void)time;
    (void)axis;
    (void)value;
}

static void pointer_frame(void *data, struct wl_pointer *p) {
    (void)data;
    (void)p;
}

static void pointer_axis_source(void *data, struct wl_pointer *p, uint32_t source) {
    (void)data;
    (void)p;
    (void)source;
}

static void pointer_axis_stop(void *data, struct wl_pointer *p, uint32_t time, uint32_t axis) {
    (void)data;
    (void)p;
    (void)time;
    (void)axis;
}

static void pointer_axis_discrete(void *data, struct wl_pointer *p, uint32_t axis,
                                  int32_t discrete) {
    (void)data;
    (void)p;
    (void)axis;
    (void)discrete;
}

static const struct wl_pointer_listener pointer_listener = {
    .enter = pointer_enter,
    .leave = pointer_leave,
    .motion = pointer_motion,
    .button = pointer_button,
    .axis = pointer_axis,
    .frame = pointer_frame,
    .axis_source = pointer_axis_source,
    .axis_stop = pointer_axis_stop,
    .axis_discrete = pointer_axis_discrete,
};

static void keyboard_keymap(void *data, struct wl_keyboard *k, uint32_t format, int32_t fd,
                            uint32_t size) {
    (void)k;
    struct app *app = data;
    if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 || size == 0) {
        printf("[wl-demo:%s] no xkb keymap (format %u); using raw keycodes\n", app->title,
               format);
        close(fd);
        return;
    }
    char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);
    if (map == MAP_FAILED) {
        printf("[wl-demo:%s] FAIL keymap mmap errno=%d\n", app->title, errno);
        return;
    }
    if (app->xkb_ctx == NULL) {
        app->xkb_ctx = xkb_context_new(XKB_CONTEXT_NO_DEFAULT_INCLUDES);
    }
    struct xkb_keymap *keymap = xkb_keymap_new_from_buffer(
        app->xkb_ctx, map, strnlen(map, size), XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
    munmap(map, size);
    if (keymap == NULL) {
        printf("[wl-demo:%s] FAIL keymap compile\n", app->title);
        return;
    }
    xkb_state_unref(app->xkb_state);
    xkb_keymap_unref(app->keymap);
    app->keymap = keymap;
    app->xkb_state = xkb_state_new(keymap);
    printf("[wl-demo:%s] xkb keymap loaded (%u bytes, layout %s)\n", app->title, size,
           xkb_keymap_layout_get_name(keymap, 0));
}

/* ------------------------------------------------------------------ */
/* Clipboard (--copy / --paste)                                        */
/* ------------------------------------------------------------------ */

static const char *const TEXT_MIMES[] = {"text/plain;charset=utf-8", "text/plain", "UTF8_STRING"};

static void source_target(void *data, struct wl_data_source *s, const char *mime) {
    (void)data;
    (void)s;
    (void)mime;
}

static void source_send(void *data, struct wl_data_source *s, const char *mime, int32_t fd) {
    (void)s;
    struct app *app = data;
    const char *p = app->copy_text;
    size_t left = strlen(p);
    while (left > 0) {
        ssize_t n = write(fd, p, left);
        if (n <= 0) {
            break;
        }
        p += n;
        left -= (size_t)n;
    }
    close(fd);
    printf("[wl-demo:%s] clipboard sent '%s' as %s\n", app->title, app->copy_text, mime);
}

static void source_cancelled(void *data, struct wl_data_source *s) {
    struct app *app = data;
    printf("[wl-demo:%s] clipboard ownership lost\n", app->title);
    wl_data_source_destroy(s);
    if (app->source == s) {
        app->source = NULL;
    }
}

static void source_dnd_drop_performed(void *data, struct wl_data_source *s) {
    (void)data;
    (void)s;
}

static void source_dnd_finished(void *data, struct wl_data_source *s) {
    (void)data;
    (void)s;
}

static void source_action(void *data, struct wl_data_source *s, uint32_t action) {
    (void)data;
    (void)s;
    (void)action;
}

static const struct wl_data_source_listener source_listener = {
    .target = source_target,
    .send = source_send,
    .cancelled = source_cancelled,
    .dnd_drop_performed = source_dnd_drop_performed,
    .dnd_finished = source_dnd_finished,
    .action = source_action,
};

/* Take the selection once, with the serial of our first keyboard enter. */
static void maybe_copy(struct app *app, uint32_t serial) {
    if (app->copy_text == NULL || app->copied || app->data_device == NULL) {
        return;
    }
    app->source = wl_data_device_manager_create_data_source(app->ddm);
    wl_data_source_add_listener(app->source, &source_listener, app);
    for (size_t i = 0; i < sizeof(TEXT_MIMES) / sizeof(TEXT_MIMES[0]); i++) {
        wl_data_source_offer(app->source, TEXT_MIMES[i]);
    }
    wl_data_device_set_selection(app->data_device, app->source, serial);
    app->copied = 1;
    printf("[wl-demo:%s] clipboard set '%s'\n", app->title, app->copy_text);
}

static void offer_offer(void *data, struct wl_data_offer *o, const char *mime) {
    struct app *app = data;
    if (o != app->pending_offer) {
        return;
    }
    for (size_t i = 0; i < sizeof(TEXT_MIMES) / sizeof(TEXT_MIMES[0]); i++) {
        if (strcmp(mime, TEXT_MIMES[i]) == 0 && (app->offer_mime < 0 || (int)i < app->offer_mime)) {
            app->offer_mime = (int)i;
        }
    }
}

static void offer_source_actions(void *data, struct wl_data_offer *o, uint32_t actions) {
    (void)data;
    (void)o;
    (void)actions;
}

static void offer_action(void *data, struct wl_data_offer *o, uint32_t action) {
    (void)data;
    (void)o;
    (void)action;
}

static const struct wl_data_offer_listener offer_listener = {
    .offer = offer_offer,
    .source_actions = offer_source_actions,
    .action = offer_action,
};

static void device_data_offer(void *data, struct wl_data_device *d, struct wl_data_offer *o) {
    (void)d;
    struct app *app = data;
    if (app->pending_offer != NULL) {
        wl_data_offer_destroy(app->pending_offer);
    }
    app->pending_offer = o;
    app->offer_mime = -1;
    wl_data_offer_add_listener(o, &offer_listener, app);
}

static void paste_finish(struct app *app) {
    app->paste_buf[app->paste_len] = '\0';
    printf("[wl-demo:%s] paste '%s'\n", app->title, app->paste_buf);
    close(app->paste_fd);
    app->paste_fd = -1;
}

static void device_selection(void *data, struct wl_data_device *d, struct wl_data_offer *o) {
    (void)d;
    struct app *app = data;
    if (o == NULL) {
        printf("[wl-demo:%s] selection empty\n", app->title);
        return;
    }
    if (!app->paste || o != app->pending_offer || app->offer_mime < 0) {
        return;
    }
    int fds[2];
    if (pipe2(fds, O_CLOEXEC) != 0) {
        printf("[wl-demo:%s] FAIL pipe errno=%d\n", app->title, errno);
        return;
    }
    if (app->paste_fd >= 0) {
        close(app->paste_fd);
    }
    wl_data_offer_receive(o, TEXT_MIMES[app->offer_mime], fds[1]);
    close(fds[1]);
    /* Read in the poll loop: we may be the source, and our own `send` must
     * still be dispatched while the pipe is open. */
    app->paste_fd = fds[0];
    app->paste_len = 0;
    fcntl(app->paste_fd, F_SETFL, O_NONBLOCK);
}

static void paste_readable(struct app *app) {
    for (;;) {
        size_t room = sizeof(app->paste_buf) - 1 - app->paste_len;
        if (room == 0) {
            paste_finish(app);
            return;
        }
        ssize_t n = read(app->paste_fd, app->paste_buf + app->paste_len, room);
        if (n > 0) {
            app->paste_len += (size_t)n;
            continue;
        }
        if (n < 0 && (errno == EAGAIN || errno == EINTR)) {
            return;
        }
        paste_finish(app); /* EOF or error */
        return;
    }
}

static void device_enter(void *data, struct wl_data_device *d, uint32_t serial,
                         struct wl_surface *s, wl_fixed_t x, wl_fixed_t y, struct wl_data_offer *o) {
    (void)data;
    (void)d;
    (void)serial;
    (void)s;
    (void)x;
    (void)y;
    (void)o;
}

static void device_leave(void *data, struct wl_data_device *d) {
    (void)data;
    (void)d;
}

static void device_motion(void *data, struct wl_data_device *d, uint32_t t, wl_fixed_t x,
                          wl_fixed_t y) {
    (void)data;
    (void)d;
    (void)t;
    (void)x;
    (void)y;
}

static void device_drop(void *data, struct wl_data_device *d) {
    (void)data;
    (void)d;
}

static const struct wl_data_device_listener device_listener = {
    .data_offer = device_data_offer,
    .enter = device_enter,
    .leave = device_leave,
    .motion = device_motion,
    .drop = device_drop,
    .selection = device_selection,
};

static void keyboard_enter(void *data, struct wl_keyboard *k, uint32_t serial,
                           struct wl_surface *s, struct wl_array *keys) {
    (void)k;
    (void)s;
    (void)keys;
    struct app *app = data;
    printf("[wl-demo:%s] keyboard enter\n", app->title);
    maybe_copy(app, serial);
}

static void keyboard_leave(void *data, struct wl_keyboard *k, uint32_t serial,
                           struct wl_surface *s) {
    (void)k;
    (void)serial;
    (void)s;
    struct app *app = data;
    printf("[wl-demo:%s] keyboard leave\n", app->title);
}

static void keyboard_key(void *data, struct wl_keyboard *k, uint32_t serial, uint32_t time,
                         uint32_t key, uint32_t state) {
    (void)k;
    (void)serial;
    (void)time;
    struct app *app = data;
    if (state != WL_KEYBOARD_KEY_STATE_PRESSED) {
        return;
    }
    if (app->xkb_state == NULL) {
        printf("[wl-demo:%s] key %u pressed\n", app->title, key);
        if (key == KEY_Q) {
            app->running = 0;
        }
        return;
    }
    /* evdev keycode + 8 = XKB keycode */
    xkb_keysym_t sym = xkb_state_key_get_one_sym(app->xkb_state, key + 8);
    char name[64];
    char text[16];
    xkb_keysym_get_name(sym, name, sizeof(name));
    xkb_state_key_get_utf8(app->xkb_state, key + 8, text, sizeof(text));
    printf("[wl-demo:%s] key %u pressed sym=%s text='%s'\n", app->title, key, name, text);
    if (sym == XKB_KEY_q || sym == XKB_KEY_Q) {
        app->running = 0;
    }
}

static void keyboard_modifiers(void *data, struct wl_keyboard *k, uint32_t serial,
                               uint32_t depressed, uint32_t latched, uint32_t locked,
                               uint32_t group) {
    (void)k;
    (void)serial;
    struct app *app = data;
    if (app->xkb_state != NULL) {
        xkb_state_update_mask(app->xkb_state, depressed, latched, locked, 0, 0, group);
    }
}

static void keyboard_repeat_info(void *data, struct wl_keyboard *k, int32_t rate, int32_t delay) {
    (void)data;
    (void)k;
    (void)rate;
    (void)delay;
}

static const struct wl_keyboard_listener keyboard_listener = {
    .keymap = keyboard_keymap,
    .enter = keyboard_enter,
    .leave = keyboard_leave,
    .key = keyboard_key,
    .modifiers = keyboard_modifiers,
    .repeat_info = keyboard_repeat_info,
};

static void seat_capabilities(void *data, struct wl_seat *seat, uint32_t caps) {
    struct app *app = data;
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && app->pointer == NULL) {
        app->pointer = wl_seat_get_pointer(seat);
        wl_pointer_add_listener(app->pointer, &pointer_listener, app);
    }
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && app->keyboard == NULL) {
        app->keyboard = wl_seat_get_keyboard(seat);
        wl_keyboard_add_listener(app->keyboard, &keyboard_listener, app);
    }
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {
    (void)data;
    (void)seat;
    (void)name;
}

static const struct wl_seat_listener seat_listener = {
    .capabilities = seat_capabilities,
    .name = seat_name,
};

/* ------------------------------------------------------------------ */
/* Registry and main                                                   */
/* ------------------------------------------------------------------ */

static uint32_t min_u32(uint32_t a, uint32_t b) {
    return a < b ? a : b;
}

static void registry_global(void *data, struct wl_registry *reg, uint32_t name,
                            const char *iface, uint32_t version) {
    struct app *app = data;
    if (strcmp(iface, wl_compositor_interface.name) == 0) {
        app->compositor = wl_registry_bind(reg, name, &wl_compositor_interface, min_u32(version, 4));
    } else if (strcmp(iface, wl_shm_interface.name) == 0) {
        app->shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (strcmp(iface, wl_seat_interface.name) == 0) {
        app->seat = wl_registry_bind(reg, name, &wl_seat_interface, min_u32(version, 5));
        wl_seat_add_listener(app->seat, &seat_listener, app);
    } else if (strcmp(iface, wl_data_device_manager_interface.name) == 0) {
        app->ddm = wl_registry_bind(reg, name, &wl_data_device_manager_interface, min_u32(version, 3));
    } else if (strcmp(iface, xdg_wm_base_interface.name) == 0) {
        app->wm_base = wl_registry_bind(reg, name, &xdg_wm_base_interface, min_u32(version, 6));
        xdg_wm_base_add_listener(app->wm_base, &wm_base_listener, app);
    }
}

static void registry_global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data;
    (void)reg;
    (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    static struct app app;
    if (argc < 3) {
        printf("usage: wl-demo <title> <0xRRGGBB> [--size WxH] [--animate] [--copy TEXT] [--paste]\n");
        return 2;
    }
    app.title = argv[1];
    app.color = (uint32_t)strtoul(argv[2], NULL, 0) & 0xffffff;
    app.alt_color = (~app.color) & 0xffffff;
    app.width = 280;
    app.height = 180;
    app.running = 1;
    app.paste_fd = -1;
    app.offer_mime = -1;
    for (int i = 3; i < argc; i++) {
        if (strcmp(argv[i], "--animate") == 0) {
            app.animate = 1;
        } else if (strcmp(argv[i], "--size") == 0 && i + 1 < argc) {
            sscanf(argv[++i], "%dx%d", &app.width, &app.height);
        } else if (strcmp(argv[i], "--copy") == 0 && i + 1 < argc) {
            app.copy_text = argv[++i];
        } else if (strcmp(argv[i], "--paste") == 0) {
            app.paste = 1;
        }
    }

    app.display = wl_display_connect(NULL);
    if (app.display == NULL) {
        printf("[wl-demo:%s] FAIL connect errno=%d\n", app.title, errno);
        return 1;
    }
    struct wl_registry *reg = wl_display_get_registry(app.display);
    wl_registry_add_listener(reg, &registry_listener, &app);
    wl_display_roundtrip(app.display);
    if (app.compositor == NULL || app.shm == NULL || app.wm_base == NULL) {
        printf("[wl-demo:%s] FAIL missing globals\n", app.title);
        return 1;
    }
    if ((app.copy_text != NULL || app.paste) && (app.ddm == NULL || app.seat == NULL)) {
        printf("[wl-demo:%s] FAIL no wl_data_device_manager\n", app.title);
        return 1;
    }
    if (app.ddm != NULL && app.seat != NULL && (app.copy_text != NULL || app.paste)) {
        app.data_device = wl_data_device_manager_get_data_device(app.ddm, app.seat);
        wl_data_device_add_listener(app.data_device, &device_listener, &app);
    }
    if (create_buffers(&app) != 0) {
        return 1;
    }

    app.surface = wl_compositor_create_surface(app.compositor);
    app.xdg_surface = xdg_wm_base_get_xdg_surface(app.wm_base, app.surface);
    xdg_surface_add_listener(app.xdg_surface, &xdg_surface_listener, &app);
    app.toplevel = xdg_surface_get_toplevel(app.xdg_surface);
    xdg_toplevel_add_listener(app.toplevel, &toplevel_listener, &app);
    xdg_toplevel_set_title(app.toplevel, app.title);
    xdg_toplevel_set_app_id(app.toplevel, "peetos.wl-demo");
    wl_surface_commit(app.surface);

    /* Poll loop rather than wl_display_dispatch: a paste pipe is read
     * alongside the display connection. */
    while (app.running) {
        while (wl_display_prepare_read(app.display) != 0) {
            wl_display_dispatch_pending(app.display);
        }
        if (wl_display_flush(app.display) < 0 && errno != EAGAIN) {
            wl_display_cancel_read(app.display);
            break;
        }
        struct pollfd pfd[2] = {
            {.fd = wl_display_get_fd(app.display), .events = POLLIN},
            {.fd = app.paste_fd, .events = POLLIN},
        };
        int nfds = app.paste_fd >= 0 ? 2 : 1;
        if (poll(pfd, (nfds_t)nfds, -1) < 0) {
            wl_display_cancel_read(app.display);
            if (errno == EINTR) {
                continue;
            }
            break;
        }
        if (pfd[0].revents & (POLLIN | POLLERR | POLLHUP)) {
            if (wl_display_read_events(app.display) < 0) {
                break;
            }
        } else {
            wl_display_cancel_read(app.display);
        }
        if (wl_display_dispatch_pending(app.display) < 0) {
            break;
        }
        if (nfds == 2 && app.paste_fd >= 0 && (pfd[1].revents & (POLLIN | POLLERR | POLLHUP))) {
            paste_readable(&app);
        }
    }

    printf("[wl-demo:%s] exiting after %d frames\n", app.title, app.frames);
    xdg_toplevel_destroy(app.toplevel);
    xdg_surface_destroy(app.xdg_surface);
    wl_surface_destroy(app.surface);
    for (int i = 0; i < NUM_BUFFERS; i++) {
        wl_buffer_destroy(app.buffers[i].wl);
    }
    wl_shm_pool_destroy(app.pool);
    wl_display_roundtrip(app.display);
    wl_display_disconnect(app.display);
    return 0;
}
