/*
 * wl-demo: a small xdg-shell Wayland client for PeetOS.
 *
 * Opens one xdg_toplevel, draws a solid colour with a border (plus an accent
 * bar while the window is activated), and reacts to input:
 *   - left click toggles between the primary and alternate colour
 *   - every key press is logged; 'q' (evdev KEY_Q) exits
 *   - xdg_toplevel.close (the compositor's close box) exits
 * With --animate it redraws continuously, paced by frame callbacks.
 *
 * Usage: wl-demo <title> <0xRRGGBB> [--size WxH] [--animate]
 *
 * Log markers are prefixed "[wl-demo:<title>]".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>
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
    (void)data;
    (void)k;
    (void)format;
    (void)size;
    close(fd);
}

static void keyboard_enter(void *data, struct wl_keyboard *k, uint32_t serial,
                           struct wl_surface *s, struct wl_array *keys) {
    (void)k;
    (void)serial;
    (void)s;
    (void)keys;
    struct app *app = data;
    printf("[wl-demo:%s] keyboard enter\n", app->title);
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
    printf("[wl-demo:%s] key %u pressed\n", app->title, key);
    if (key == KEY_Q) {
        app->running = 0;
    }
}

static void keyboard_modifiers(void *data, struct wl_keyboard *k, uint32_t serial,
                               uint32_t depressed, uint32_t latched, uint32_t locked,
                               uint32_t group) {
    (void)data;
    (void)k;
    (void)serial;
    (void)depressed;
    (void)latched;
    (void)locked;
    (void)group;
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
        printf("usage: wl-demo <title> <0xRRGGBB> [--size WxH] [--animate]\n");
        return 2;
    }
    app.title = argv[1];
    app.color = (uint32_t)strtoul(argv[2], NULL, 0) & 0xffffff;
    app.alt_color = (~app.color) & 0xffffff;
    app.width = 280;
    app.height = 180;
    app.running = 1;
    for (int i = 3; i < argc; i++) {
        if (strcmp(argv[i], "--animate") == 0) {
            app.animate = 1;
        } else if (strcmp(argv[i], "--size") == 0 && i + 1 < argc) {
            sscanf(argv[++i], "%dx%d", &app.width, &app.height);
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

    while (app.running && wl_display_dispatch(app.display) != -1) {
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
