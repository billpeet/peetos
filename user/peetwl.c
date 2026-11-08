/*
 * peetwl: the PeetOS Wayland compositor (libwayland-server, software
 * rendering to /dev/fb0).
 *
 * Globals: wl_compositor (v4), wl_shm, wl_seat (v5, pointer + keyboard),
 * wl_output (v3), xdg_wm_base (v6; toplevels, popups are dismissed).
 *
 * Window management is deliberately small: toplevels are cascaded, get
 * server-side decorations (title bar with a close box), are raised and focused
 * on click, and can be dragged by the title bar. Committed wl_shm buffers are
 * copied into compositor-owned storage and released immediately. Rendering is
 * damage-tracked into a back buffer and copied to the framebuffer at most once
 * per 16 ms; frame callbacks fire after each repaint.
 *
 * Input comes from /dev/input/event0 (keyboard) and /dev/input/event1
 * (pointer; relative PS/2 or absolute tablet). The keyboard keymap is
 * compiled with libxkbcommon (from the xkeyboard-config data under
 * /usr/share/X11/xkb), sent to clients as an XKB_V1 keymap in a sealed memfd,
 * and modifier/lock/group state is tracked with xkb_state. If the keymap
 * cannot be compiled, clients get raw keycodes with NO_KEYMAP instead.
 *
 * Usage: peetwl [--smoke] [--xkb-layout L] [--xkb-variant V]
 *               [--xkb-options O] [--client "<cmd> [args...]"]...
 *   --client       spawn a client once the socket is up (repeatable)
 *   --xkb-*        keymap selection (default: XKB_DEFAULT_* environment,
 *                  then evdev/pc105/us)
 *   --smoke        log cursor/focus details for tests, exit when every
 *                  spawned client has exited, and enforce a watchdog
 *
 * Log markers are prefixed "[peetwl]".
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <wayland-server.h>
#include <xkbcommon/xkbcommon.h>
#include "xdg-shell-server-protocol.h"

#include "font8x8.h"

#define FBIOGETINFO 0x5001UL
#define EV_SYN 0x00
#define EV_KEY 0x01
#define EV_REL 0x02
#define EV_ABS 0x03
#define SYN_REPORT 0
#define REL_X 0x00
#define REL_Y 0x01
#define ABS_X 0x00
#define ABS_Y 0x01
#define BTN_LEFT 0x110
#define BTN_RIGHT 0x111
#define BTN_MIDDLE 0x112
#define KEY_LEFTCTRL 29
#define KEY_LEFTSHIFT 42
#define KEY_RIGHTSHIFT 54
#define KEY_LEFTALT 56
#define KEY_RIGHTCTRL 97
#define KEY_RIGHTALT 100

#define MOD_SHIFT 0x1
#define MOD_CTRL 0x4
#define MOD_ALT 0x8

#define TITLE_H 22
#define BORDER 2
#define CLOSE_SIZE 14
#define CURSOR_W 11
#define CURSOR_H 16
#define DEFAULT_W 320
#define DEFAULT_H 220
#define REPAINT_MS 16
#define SMOKE_WATCHDOG_MS 90000
#define MAX_CLIENTS 8
#define RUNTIME_DIR "/tmp"

#define COLOR_BG 0x00203040u
#define COLOR_TITLE_FOCUSED 0x003a6ea5u
#define COLOR_TITLE 0x00505860u
#define COLOR_BORDER_FOCUSED 0x005a8ec5u
#define COLOR_BORDER 0x00303840u
#define COLOR_TEXT 0x00f0f0f0u
#define COLOR_CLOSE 0x00c04040u

struct fb_info {
    uint32_t width;
    uint32_t height;
    uint32_t pitch;
    uint32_t bpp;
    uint32_t format;
    uint64_t map_size;
};

struct input_event_raw {
    int64_t tv_sec;
    int64_t tv_usec;
    uint16_t type;
    uint16_t code;
    int32_t value;
};

struct box {
    int x, y, w, h;
};

struct server;

struct surface {
    struct server *srv;
    struct wl_resource *resource;
    struct wl_list link; /* server.surfaces */

    struct wl_resource *pending_buffer;
    int pending_buffer_set;
    struct wl_list pending_frames; /* wl_callback resources */

    uint32_t *pixels; /* committed content, owned */
    int width;
    int height;

    struct xdg_surface_state *xdg;
};

struct xdg_surface_state {
    struct wl_resource *resource;
    struct surface *surface;
    struct toplevel *toplevel;
    uint32_t last_serial;
    int configure_sent;
    int acked;
};

struct toplevel {
    struct wl_resource *resource;
    struct xdg_surface_state *xdg;
    struct wl_list link; /* server.stack, top first */
    char title[64];
    int x, y; /* content origin on screen */
    int mapped;
    int activated_sent;
};

enum hit { HIT_NONE, HIT_CONTENT, HIT_TITLE, HIT_CLOSE };

struct server {
    struct wl_display *display;
    struct wl_event_loop *loop;
    int smoke;

    /* output */
    int fb_fd;
    uint32_t *fb;
    uint32_t *back;
    int width, height, stride_px;
    struct box dirty;
    int dirty_set;
    struct wl_event_source *repaint_timer;
    int repaint_scheduled;
    uint32_t frame_count;

    /* scene */
    struct wl_list surfaces;
    struct wl_list stack;
    struct wl_list frame_callbacks;
    int placed;

    /* seat */
    struct wl_list pointers;
    struct wl_list keyboards;
    int cursor_x, cursor_y;
    int pending_dx, pending_dy;
    int abs_x, abs_y, abs_pending;
    uint32_t buttons;
    uint32_t mods;
    struct surface *pointer_focus;
    /* keyboard keymap (xkbcommon) */
    struct xkb_context *xkb_ctx;
    struct xkb_keymap *keymap;
    struct xkb_state *xkb_state;
    int keymap_fd;
    uint32_t keymap_size;
    uint32_t mods_depressed, mods_latched, mods_locked, group;
    struct surface *keyboard_focus;
    struct toplevel *grab;
    int grab_dx, grab_dy;

    /* clients we spawned */
    pid_t children[MAX_CLIENTS];
    int child_count;
    int children_exited;
    int children_failed;
    int failed;
};

static uint32_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

/* ------------------------------------------------------------------ */
/* Damage and rendering                                                */
/* ------------------------------------------------------------------ */

static void schedule_repaint(struct server *srv) {
    if (!srv->repaint_scheduled) {
        srv->repaint_scheduled = 1;
        wl_event_source_timer_update(srv->repaint_timer, REPAINT_MS);
    }
}

static void damage_box(struct server *srv, struct box b) {
    int x0 = b.x < 0 ? 0 : b.x;
    int y0 = b.y < 0 ? 0 : b.y;
    int x1 = b.x + b.w > srv->width ? srv->width : b.x + b.w;
    int y1 = b.y + b.h > srv->height ? srv->height : b.y + b.h;
    if (x1 <= x0 || y1 <= y0) {
        return;
    }
    if (!srv->dirty_set) {
        srv->dirty = (struct box){x0, y0, x1 - x0, y1 - y0};
        srv->dirty_set = 1;
    } else {
        int dx1 = srv->dirty.x + srv->dirty.w;
        int dy1 = srv->dirty.y + srv->dirty.h;
        if (x0 < srv->dirty.x) srv->dirty.x = x0;
        if (y0 < srv->dirty.y) srv->dirty.y = y0;
        if (x1 > dx1) dx1 = x1;
        if (y1 > dy1) dy1 = y1;
        srv->dirty.w = dx1 - srv->dirty.x;
        srv->dirty.h = dy1 - srv->dirty.y;
    }
    schedule_repaint(srv);
}

static struct box frame_box(struct toplevel *top) {
    struct surface *s = top->xdg->surface;
    return (struct box){top->x - BORDER, top->y - TITLE_H - BORDER, s->width + 2 * BORDER,
                        s->height + TITLE_H + 2 * BORDER};
}

static struct box close_box(struct toplevel *top) {
    struct surface *s = top->xdg->surface;
    return (struct box){top->x + s->width - CLOSE_SIZE - 4, top->y - TITLE_H + 4, CLOSE_SIZE,
                        CLOSE_SIZE};
}

static void damage_toplevel(struct server *srv, struct toplevel *top) {
    if (top->mapped) {
        damage_box(srv, frame_box(top));
    }
}

static struct box cursor_box(struct server *srv) {
    return (struct box){srv->cursor_x, srv->cursor_y, CURSOR_W, CURSOR_H};
}

/* Clip `b` to `clip`; returns 0 when empty. */
static int clip_box(struct box *b, struct box clip) {
    int x0 = b->x > clip.x ? b->x : clip.x;
    int y0 = b->y > clip.y ? b->y : clip.y;
    int x1 = b->x + b->w < clip.x + clip.w ? b->x + b->w : clip.x + clip.w;
    int y1 = b->y + b->h < clip.y + clip.h ? b->y + b->h : clip.y + clip.h;
    if (x1 <= x0 || y1 <= y0) {
        return 0;
    }
    *b = (struct box){x0, y0, x1 - x0, y1 - y0};
    return 1;
}

static void fill(struct server *srv, struct box b, struct box clip, uint32_t color) {
    if (!clip_box(&b, clip)) {
        return;
    }
    for (int y = b.y; y < b.y + b.h; y++) {
        uint32_t *row = srv->back + (size_t)y * srv->width;
        for (int x = b.x; x < b.x + b.w; x++) {
            row[x] = color;
        }
    }
}

static void draw_text(struct server *srv, int x, int y, const char *text, int max_w,
                      struct box clip, uint32_t color) {
    for (int i = 0; text[i] != '\0' && (i + 1) * 8 <= max_w; i++) {
        unsigned char ch = (unsigned char)text[i];
        const unsigned char *glyph = font8x8_basic[ch < 128 ? ch : '?'];
        for (int gy = 0; gy < 8; gy++) {
            int py = y + gy;
            if (py < clip.y || py >= clip.y + clip.h) {
                continue;
            }
            for (int gx = 0; gx < 8; gx++) {
                int px = x + i * 8 + gx;
                if (px < clip.x || px >= clip.x + clip.w) {
                    continue;
                }
                if (glyph[gy] & (1u << gx)) {
                    srv->back[(size_t)py * srv->width + px] = color;
                }
            }
        }
    }
}

static void draw_toplevel(struct server *srv, struct toplevel *top, struct box clip) {
    struct surface *s = top->xdg->surface;
    int focused = srv->keyboard_focus == s;
    struct box frame = frame_box(top);

    fill(srv, frame, clip, focused ? COLOR_BORDER_FOCUSED : COLOR_BORDER);
    fill(srv, (struct box){top->x, top->y - TITLE_H, s->width, TITLE_H}, clip,
         focused ? COLOR_TITLE_FOCUSED : COLOR_TITLE);
    draw_text(srv, top->x + 6, top->y - TITLE_H + 7, top->title, s->width - CLOSE_SIZE - 16,
              clip, COLOR_TEXT);
    struct box cb = close_box(top);
    fill(srv, cb, clip, COLOR_CLOSE);
    for (int i = 3; i < CLOSE_SIZE - 3; i++) {
        struct box d1 = {cb.x + i, cb.y + i, 1, 1};
        struct box d2 = {cb.x + CLOSE_SIZE - 1 - i, cb.y + i, 1, 1};
        fill(srv, d1, clip, COLOR_TEXT);
        fill(srv, d2, clip, COLOR_TEXT);
    }

    struct box content = {top->x, top->y, s->width, s->height};
    if (!clip_box(&content, clip)) {
        return;
    }
    for (int y = content.y; y < content.y + content.h; y++) {
        const uint32_t *src = s->pixels + (size_t)(y - top->y) * s->width + (content.x - top->x);
        uint32_t *dst = srv->back + (size_t)y * srv->width + content.x;
        memcpy(dst, src, (size_t)content.w * 4);
    }
}

/* Arrow cursor: 1 = black outline, 2 = white fill. */
static const char *cursor_rows[CURSOR_H] = {
    "1          ", "11         ", "121        ", "1221       ", "12221      ", "122221     ",
    "1222221    ", "12222221   ", "122222221  ", "1222222221 ", "12222211111", "1221221    ",
    "121 1221   ", "11  1221   ", "1    1221  ", "     111   ",
};

static void draw_cursor(struct server *srv, struct box clip) {
    for (int y = 0; y < CURSOR_H; y++) {
        int py = srv->cursor_y + y;
        if (py < clip.y || py >= clip.y + clip.h) {
            continue;
        }
        for (int x = 0; x < CURSOR_W; x++) {
            int px = srv->cursor_x + x;
            if (px < clip.x || px >= clip.x + clip.w) {
                continue;
            }
            char c = cursor_rows[y][x];
            if (c == '1') {
                srv->back[(size_t)py * srv->width + px] = 0x00000000u;
            } else if (c == '2') {
                srv->back[(size_t)py * srv->width + px] = 0x00ffffffu;
            }
        }
    }
}

static void repaint(struct server *srv) {
    if (srv->dirty_set) {
        struct box clip = srv->dirty;
        fill(srv, clip, clip, COLOR_BG);
        struct toplevel *top;
        wl_list_for_each_reverse(top, &srv->stack, link) {
            if (top->mapped) {
                draw_toplevel(srv, top, clip);
            }
        }
        draw_cursor(srv, clip);
        for (int y = clip.y; y < clip.y + clip.h; y++) {
            memcpy(srv->fb + (size_t)y * srv->stride_px + clip.x,
                   srv->back + (size_t)y * srv->width + clip.x, (size_t)clip.w * 4);
        }
        srv->dirty_set = 0;
        srv->frame_count++;
    }

    uint32_t t = now_ms();
    struct wl_resource *cb, *tmp;
    wl_resource_for_each_safe(cb, tmp, &srv->frame_callbacks) {
        wl_callback_send_done(cb, t);
        wl_resource_destroy(cb);
    }
}

static int on_repaint_timer(void *data) {
    struct server *srv = data;
    srv->repaint_scheduled = 0;
    repaint(srv);
    return 0;
}

/* ------------------------------------------------------------------ */
/* Seat helpers                                                        */
/* ------------------------------------------------------------------ */

static const char *surface_title(struct surface *s) {
    if (s != NULL && s->xdg != NULL && s->xdg->toplevel != NULL) {
        return s->xdg->toplevel->title;
    }
    return "?";
}

static enum hit hit_test(struct server *srv, int x, int y, struct toplevel **out) {
    struct toplevel *top;
    wl_list_for_each(top, &srv->stack, link) {
        if (!top->mapped) {
            continue;
        }
        struct surface *s = top->xdg->surface;
        struct box f = frame_box(top);
        if (x < f.x || y < f.y || x >= f.x + f.w || y >= f.y + f.h) {
            continue;
        }
        *out = top;
        struct box cb = close_box(top);
        if (x >= cb.x && y >= cb.y && x < cb.x + cb.w && y < cb.y + cb.h) {
            return HIT_CLOSE;
        }
        if (x >= top->x && y >= top->y && x < top->x + s->width && y < top->y + s->height) {
            return HIT_CONTENT;
        }
        return HIT_TITLE;
    }
    *out = NULL;
    return HIT_NONE;
}

static void send_pointer_frame(struct wl_resource *res) {
    if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION) {
        wl_pointer_send_frame(res);
    }
}

static void set_pointer_focus(struct server *srv, struct surface *s, int sx, int sy) {
    if (srv->pointer_focus == s) {
        return;
    }
    uint32_t serial = wl_display_next_serial(srv->display);
    struct wl_resource *res;
    if (srv->pointer_focus != NULL) {
        struct wl_client *old = wl_resource_get_client(srv->pointer_focus->resource);
        wl_resource_for_each(res, &srv->pointers) {
            if (wl_resource_get_client(res) == old) {
                wl_pointer_send_leave(res, serial, srv->pointer_focus->resource);
                send_pointer_frame(res);
            }
        }
    }
    srv->pointer_focus = s;
    if (s != NULL) {
        struct wl_client *client = wl_resource_get_client(s->resource);
        wl_resource_for_each(res, &srv->pointers) {
            if (wl_resource_get_client(res) == client) {
                wl_pointer_send_enter(res, serial, s->resource, wl_fixed_from_int(sx),
                                      wl_fixed_from_int(sy));
                send_pointer_frame(res);
            }
        }
        if (srv->smoke) {
            printf("[peetwl] pointer enter '%s'\n", surface_title(s));
        }
    }
}

static void send_modifiers(struct server *srv, struct wl_resource *kb, uint32_t serial) {
    if (srv->xkb_state != NULL) {
        wl_keyboard_send_modifiers(kb, serial, srv->mods_depressed, srv->mods_latched,
                                   srv->mods_locked, srv->group);
    } else {
        wl_keyboard_send_modifiers(kb, serial, srv->mods, 0, 0, 0);
    }
}

static void send_keyboard_enter(struct server *srv, struct wl_resource *kb, struct surface *s) {
    struct wl_array keys;
    wl_array_init(&keys);
    uint32_t serial = wl_display_next_serial(srv->display);
    wl_keyboard_send_enter(kb, serial, s->resource, &keys);
    send_modifiers(srv, kb, serial);
    wl_array_release(&keys);
}

static void send_toplevel_configure(struct server *srv, struct toplevel *top);

static void set_keyboard_focus(struct server *srv, struct surface *s) {
    if (srv->keyboard_focus == s) {
        return;
    }
    struct surface *old = srv->keyboard_focus;
    uint32_t serial = wl_display_next_serial(srv->display);
    struct wl_resource *res;
    if (old != NULL) {
        struct wl_client *client = wl_resource_get_client(old->resource);
        wl_resource_for_each(res, &srv->keyboards) {
            if (wl_resource_get_client(res) == client) {
                wl_keyboard_send_leave(res, serial, old->resource);
            }
        }
    }
    srv->keyboard_focus = s;
    if (s != NULL) {
        struct wl_client *client = wl_resource_get_client(s->resource);
        wl_resource_for_each(res, &srv->keyboards) {
            if (wl_resource_get_client(res) == client) {
                send_keyboard_enter(srv, res, s);
            }
        }
        printf("[peetwl] focus '%s'\n", surface_title(s));
    }
    /* Activated state changes are reported to both windows. */
    if (old != NULL && old->xdg != NULL && old->xdg->toplevel != NULL) {
        send_toplevel_configure(srv, old->xdg->toplevel);
        damage_toplevel(srv, old->xdg->toplevel);
    }
    if (s != NULL && s->xdg != NULL && s->xdg->toplevel != NULL) {
        send_toplevel_configure(srv, s->xdg->toplevel);
        damage_toplevel(srv, s->xdg->toplevel);
    }
}

static void update_pointer_focus(struct server *srv) {
    if (srv->grab != NULL) {
        return;
    }
    struct toplevel *top = NULL;
    enum hit h = hit_test(srv, srv->cursor_x, srv->cursor_y, &top);
    if (h == HIT_CONTENT) {
        set_pointer_focus(srv, top->xdg->surface, srv->cursor_x - top->x, srv->cursor_y - top->y);
    } else {
        set_pointer_focus(srv, NULL, 0, 0);
    }
}

static void raise_toplevel(struct server *srv, struct toplevel *top) {
    wl_list_remove(&top->link);
    wl_list_insert(&srv->stack, &top->link);
    damage_toplevel(srv, top);
}

static struct toplevel *topmost_mapped(struct server *srv) {
    struct toplevel *top;
    wl_list_for_each(top, &srv->stack, link) {
        if (top->mapped) {
            return top;
        }
    }
    return NULL;
}

/* ------------------------------------------------------------------ */
/* Input devices                                                       */
/* ------------------------------------------------------------------ */

static void pointer_motion(struct server *srv, int nx, int ny) {
    if (nx < 0) nx = 0;
    if (ny < 0) ny = 0;
    if (nx > srv->width - 1) nx = srv->width - 1;
    if (ny > srv->height - 1) ny = srv->height - 1;
    if (nx == srv->cursor_x && ny == srv->cursor_y) {
        return;
    }
    damage_box(srv, cursor_box(srv));
    srv->cursor_x = nx;
    srv->cursor_y = ny;
    damage_box(srv, cursor_box(srv));
    if (srv->smoke) {
        printf("[peetwl] cursor %d,%d\n", nx, ny);
    }

    if (srv->grab != NULL) {
        damage_toplevel(srv, srv->grab);
        srv->grab->x = nx - srv->grab_dx;
        srv->grab->y = ny - srv->grab_dy;
        damage_toplevel(srv, srv->grab);
        return;
    }

    update_pointer_focus(srv);
    if (srv->pointer_focus != NULL) {
        struct toplevel *top = srv->pointer_focus->xdg->toplevel;
        uint32_t t = now_ms();
        struct wl_client *client = wl_resource_get_client(srv->pointer_focus->resource);
        struct wl_resource *res;
        wl_resource_for_each(res, &srv->pointers) {
            if (wl_resource_get_client(res) == client) {
                wl_pointer_send_motion(res, t, wl_fixed_from_int(nx - top->x),
                                       wl_fixed_from_int(ny - top->y));
                send_pointer_frame(res);
            }
        }
    }
}

static void pointer_button(struct server *srv, uint32_t button, int pressed) {
    uint32_t bit = 1u << (button - BTN_LEFT);
    srv->buttons = pressed ? (srv->buttons | bit) : (srv->buttons & ~bit);
    if (srv->grab != NULL) {
        if (!pressed && button == BTN_LEFT) {
            printf("[peetwl] move end '%s' content=%d,%d\n", srv->grab->title, srv->grab->x,
                   srv->grab->y);
            srv->grab = NULL;
            update_pointer_focus(srv);
        }
        return;
    }

    struct toplevel *top = NULL;
    enum hit h = hit_test(srv, srv->cursor_x, srv->cursor_y, &top);
    if (pressed && top != NULL) {
        raise_toplevel(srv, top);
        set_keyboard_focus(srv, top->xdg->surface);
    }
    if (pressed && h == HIT_CLOSE) {
        printf("[peetwl] close requested '%s'\n", top->title);
        xdg_toplevel_send_close(top->resource);
        return;
    }
    if (pressed && h == HIT_TITLE && button == BTN_LEFT) {
        srv->grab = top;
        srv->grab_dx = srv->cursor_x - top->x;
        srv->grab_dy = srv->cursor_y - top->y;
        set_pointer_focus(srv, NULL, 0, 0);
        printf("[peetwl] move start '%s'\n", top->title);
        return;
    }

    if (srv->pointer_focus != NULL) {
        uint32_t serial = wl_display_next_serial(srv->display);
        uint32_t t = now_ms();
        struct wl_client *client = wl_resource_get_client(srv->pointer_focus->resource);
        struct wl_resource *res;
        wl_resource_for_each(res, &srv->pointers) {
            if (wl_resource_get_client(res) == client) {
                wl_pointer_send_button(res, serial, t, button,
                                       pressed ? WL_POINTER_BUTTON_STATE_PRESSED
                                               : WL_POINTER_BUTTON_STATE_RELEASED);
                send_pointer_frame(res);
            }
        }
    }
}

static int on_pointer_input(int fd, uint32_t mask, void *data) {
    (void)mask;
    struct server *srv = data;
    struct input_event_raw ev[32];
    for (;;) {
        ssize_t n = read(fd, ev, sizeof(ev));
        if (n <= 0) {
            break;
        }
        for (size_t i = 0; i < (size_t)n / sizeof(ev[0]); i++) {
            if (ev[i].type == EV_REL && ev[i].code == REL_X) {
                srv->pending_dx += ev[i].value;
            } else if (ev[i].type == EV_REL && ev[i].code == REL_Y) {
                srv->pending_dy += ev[i].value;
            } else if (ev[i].type == EV_ABS && ev[i].code == ABS_X) {
                srv->abs_x = ev[i].value;
                srv->abs_pending = 1;
            } else if (ev[i].type == EV_ABS && ev[i].code == ABS_Y) {
                srv->abs_y = ev[i].value;
                srv->abs_pending = 1;
            } else if (ev[i].type == EV_KEY && ev[i].code >= BTN_LEFT && ev[i].code <= BTN_MIDDLE) {
                pointer_button(srv, ev[i].code, ev[i].value != 0);
            } else if (ev[i].type == EV_SYN && ev[i].code == SYN_REPORT) {
                if (srv->abs_pending) {
                    pointer_motion(srv, srv->abs_x, srv->abs_y);
                    srv->abs_pending = 0;
                }
                if (srv->pending_dx != 0 || srv->pending_dy != 0) {
                    pointer_motion(srv, srv->cursor_x + srv->pending_dx,
                                   srv->cursor_y + srv->pending_dy);
                    srv->pending_dx = 0;
                    srv->pending_dy = 0;
                }
            }
        }
    }
    return 0;
}

static uint32_t modifier_for_key(uint32_t code) {
    switch (code) {
    case KEY_LEFTSHIFT:
    case KEY_RIGHTSHIFT:
        return MOD_SHIFT;
    case KEY_LEFTCTRL:
    case KEY_RIGHTCTRL:
        return MOD_CTRL;
    case KEY_LEFTALT:
    case KEY_RIGHTALT:
        return MOD_ALT;
    default:
        return 0;
    }
}

/* Feed a key to xkb_state; returns true if the serialized modifier/group
 * state changed. Without a keymap, a small Shift/Ctrl/Alt tracker is used. */
static int update_modifiers(struct server *srv, uint32_t code, int pressed) {
    if (srv->xkb_state != NULL) {
        /* evdev keycode + 8 = XKB keycode */
        xkb_state_update_key(srv->xkb_state, code + 8, pressed ? XKB_KEY_DOWN : XKB_KEY_UP);
        uint32_t dep = xkb_state_serialize_mods(srv->xkb_state, XKB_STATE_MODS_DEPRESSED);
        uint32_t lat = xkb_state_serialize_mods(srv->xkb_state, XKB_STATE_MODS_LATCHED);
        uint32_t lck = xkb_state_serialize_mods(srv->xkb_state, XKB_STATE_MODS_LOCKED);
        uint32_t grp = xkb_state_serialize_layout(srv->xkb_state, XKB_STATE_LAYOUT_EFFECTIVE);
        int changed = dep != srv->mods_depressed || lat != srv->mods_latched ||
                      lck != srv->mods_locked || grp != srv->group;
        srv->mods_depressed = dep;
        srv->mods_latched = lat;
        srv->mods_locked = lck;
        srv->group = grp;
        return changed;
    }
    uint32_t mod = modifier_for_key(code);
    uint32_t old_mods = srv->mods;
    if (mod != 0) {
        srv->mods = pressed ? (srv->mods | mod) : (srv->mods & ~mod);
    }
    return srv->mods != old_mods;
}

static void keyboard_key(struct server *srv, uint32_t code, int pressed) {
    int mods_changed = update_modifiers(srv, code, pressed);
    if (srv->keyboard_focus == NULL) {
        return;
    }
    uint32_t serial = wl_display_next_serial(srv->display);
    uint32_t t = now_ms();
    struct wl_client *client = wl_resource_get_client(srv->keyboard_focus->resource);
    struct wl_resource *res;
    wl_resource_for_each(res, &srv->keyboards) {
        if (wl_resource_get_client(res) != client) {
            continue;
        }
        wl_keyboard_send_key(res, serial, t, code,
                             pressed ? WL_KEYBOARD_KEY_STATE_PRESSED
                                     : WL_KEYBOARD_KEY_STATE_RELEASED);
        if (mods_changed) {
            send_modifiers(srv, res, serial);
        }
    }
}

static int on_keyboard_input(int fd, uint32_t mask, void *data) {
    (void)mask;
    struct server *srv = data;
    struct input_event_raw ev[32];
    for (;;) {
        ssize_t n = read(fd, ev, sizeof(ev));
        if (n <= 0) {
            break;
        }
        for (size_t i = 0; i < (size_t)n / sizeof(ev[0]); i++) {
            /* value 2 is autorepeat; clients repeat keys themselves. */
            if (ev[i].type == EV_KEY && ev[i].value != 2) {
                keyboard_key(srv, ev[i].code, ev[i].value != 0);
            }
        }
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/* wl_surface / wl_region                                              */
/* ------------------------------------------------------------------ */

static void map_toplevel(struct server *srv, struct toplevel *top);
static void unmap_toplevel(struct server *srv, struct toplevel *top);

static void surface_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void surface_attach(struct wl_client *c, struct wl_resource *r, struct wl_resource *buffer,
                           int32_t x, int32_t y) {
    (void)c;
    (void)x;
    (void)y;
    struct surface *s = wl_resource_get_user_data(r);
    s->pending_buffer = buffer;
    s->pending_buffer_set = 1;
}

static void surface_damage(struct wl_client *c, struct wl_resource *r, int32_t x, int32_t y,
                           int32_t w, int32_t h) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
    /* Damage is tracked per window: every commit redraws the whole window. */
}

static void frame_callback_destroyed(struct wl_resource *r) {
    wl_list_remove(wl_resource_get_link(r));
}

static void surface_frame(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct surface *s = wl_resource_get_user_data(r);
    struct wl_resource *cb = wl_resource_create(c, &wl_callback_interface, 1, id);
    if (cb == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(cb, NULL, NULL, frame_callback_destroyed);
    wl_list_insert(s->pending_frames.prev, wl_resource_get_link(cb));
}

static void surface_set_region(struct wl_client *c, struct wl_resource *r,
                               struct wl_resource *region) {
    (void)c;
    (void)r;
    (void)region;
}

static int copy_buffer(struct surface *s, struct wl_resource *buffer) {
    struct wl_shm_buffer *shm = wl_shm_buffer_get(buffer);
    if (shm == NULL) {
        return -1;
    }
    uint32_t format = wl_shm_buffer_get_format(shm);
    if (format != WL_SHM_FORMAT_XRGB8888 && format != WL_SHM_FORMAT_ARGB8888) {
        return -1;
    }
    int w = wl_shm_buffer_get_width(shm);
    int h = wl_shm_buffer_get_height(shm);
    int stride = wl_shm_buffer_get_stride(shm);
    if (w != s->width || h != s->height || s->pixels == NULL) {
        uint32_t *px = realloc(s->pixels, (size_t)w * (size_t)h * 4);
        if (px == NULL) {
            return -1;
        }
        s->pixels = px;
        s->width = w;
        s->height = h;
    }
    wl_shm_buffer_begin_access(shm);
    const uint8_t *src = wl_shm_buffer_get_data(shm);
    for (int y = 0; y < h; y++) {
        memcpy(s->pixels + (size_t)y * w, src + (size_t)y * stride, (size_t)w * 4);
    }
    wl_shm_buffer_end_access(shm);
    return 0;
}

static void surface_commit(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    struct surface *s = wl_resource_get_user_data(r);
    struct server *srv = s->srv;
    struct toplevel *top = (s->xdg != NULL) ? s->xdg->toplevel : NULL;

    wl_list_insert_list(srv->frame_callbacks.prev, &s->pending_frames);
    wl_list_init(&s->pending_frames);

    if (s->pending_buffer_set) {
        struct wl_resource *buffer = s->pending_buffer;
        s->pending_buffer = NULL;
        s->pending_buffer_set = 0;
        if (buffer == NULL) {
            if (top != NULL && top->mapped) {
                unmap_toplevel(srv, top);
            }
        } else {
            if (s->xdg != NULL && !s->xdg->acked) {
                wl_resource_post_error(s->xdg->resource, XDG_SURFACE_ERROR_UNCONFIGURED_BUFFER,
                                       "buffer attached before ack_configure");
                return;
            }
            if (top != NULL && top->mapped) {
                damage_toplevel(srv, top); /* old size */
            }
            if (copy_buffer(s, buffer) != 0) {
                wl_resource_post_error(r, WL_SHM_ERROR_INVALID_FORMAT,
                                       "unsupported buffer (need XRGB8888/ARGB8888 wl_shm)");
                return;
            }
            wl_buffer_send_release(buffer);
            if (top != NULL) {
                if (!top->mapped) {
                    map_toplevel(srv, top);
                } else {
                    damage_toplevel(srv, top);
                }
            }
        }
    } else if (s->xdg != NULL && top != NULL && !s->xdg->configure_sent) {
        /* Initial commit of a new toplevel: reply with its first configure. */
        send_toplevel_configure(srv, top);
    }
    schedule_repaint(srv);
}

static void surface_set_int(struct wl_client *c, struct wl_resource *r, int32_t v) {
    (void)c;
    (void)r;
    (void)v;
}

static void surface_offset(struct wl_client *c, struct wl_resource *r, int32_t x, int32_t y) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
}

static const struct wl_surface_interface surface_impl = {
    .destroy = surface_destroy,
    .attach = surface_attach,
    .damage = surface_damage,
    .frame = surface_frame,
    .set_opaque_region = surface_set_region,
    .set_input_region = surface_set_region,
    .commit = surface_commit,
    .set_buffer_transform = surface_set_int,
    .set_buffer_scale = surface_set_int,
    .damage_buffer = surface_damage,
    .offset = surface_offset,
};

static void surface_resource_destroyed(struct wl_resource *r) {
    struct surface *s = wl_resource_get_user_data(r);
    struct server *srv = s->srv;
    if (s->xdg != NULL && s->xdg->toplevel != NULL) {
        unmap_toplevel(srv, s->xdg->toplevel);
    }
    if (srv->pointer_focus == s) {
        srv->pointer_focus = NULL;
    }
    if (srv->keyboard_focus == s) {
        srv->keyboard_focus = NULL;
    }
    if (s->xdg != NULL) {
        s->xdg->surface = NULL;
    }
    struct wl_resource *cb, *tmp;
    wl_resource_for_each_safe(cb, tmp, &s->pending_frames) {
        wl_resource_destroy(cb);
    }
    wl_list_remove(&s->link);
    free(s->pixels);
    free(s);
}

static void region_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void region_rect(struct wl_client *c, struct wl_resource *r, int32_t x, int32_t y, int32_t w,
                        int32_t h) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
}

static const struct wl_region_interface region_impl = {
    .destroy = region_destroy,
    .add = region_rect,
    .subtract = region_rect,
};

static void compositor_create_surface(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct server *srv = wl_resource_get_user_data(r);
    struct surface *s = calloc(1, sizeof(*s));
    struct wl_resource *res =
        wl_resource_create(c, &wl_surface_interface, wl_resource_get_version(r), id);
    if (s == NULL || res == NULL) {
        free(s);
        wl_client_post_no_memory(c);
        return;
    }
    s->srv = srv;
    s->resource = res;
    wl_list_init(&s->pending_frames);
    wl_list_insert(&srv->surfaces, &s->link);
    wl_resource_set_implementation(res, &surface_impl, s, surface_resource_destroyed);
}

static void compositor_create_region(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    (void)r;
    struct wl_resource *res = wl_resource_create(c, &wl_region_interface, 1, id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &region_impl, NULL, NULL);
}

static const struct wl_compositor_interface compositor_impl = {
    .create_surface = compositor_create_surface,
    .create_region = compositor_create_region,
};

static void bind_compositor(struct wl_client *c, void *data, uint32_t version, uint32_t id) {
    struct wl_resource *res = wl_resource_create(c, &wl_compositor_interface, version, id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &compositor_impl, data, NULL);
}

/* ------------------------------------------------------------------ */
/* xdg-shell                                                           */
/* ------------------------------------------------------------------ */

static void send_toplevel_configure(struct server *srv, struct toplevel *top) {
    if (top->xdg == NULL || top->xdg->surface == NULL) {
        return;
    }
    struct wl_array states;
    wl_array_init(&states);
    int activated = srv->keyboard_focus == top->xdg->surface;
    if (activated) {
        uint32_t *st = wl_array_add(&states, sizeof(uint32_t));
        if (st != NULL) {
            *st = XDG_TOPLEVEL_STATE_ACTIVATED;
        }
    }
    /* 0x0 lets the client pick its size. */
    xdg_toplevel_send_configure(top->resource, 0, 0, &states);
    wl_array_release(&states);
    uint32_t serial = wl_display_next_serial(srv->display);
    top->xdg->last_serial = serial;
    top->xdg->configure_sent = 1;
    top->activated_sent = activated;
    xdg_surface_send_configure(top->xdg->resource, serial);
}

static void map_toplevel(struct server *srv, struct toplevel *top) {
    struct surface *s = top->xdg->surface;
    int slot = srv->placed++;
    top->x = 60 + (slot % 3) * 320;
    top->y = 60 + TITLE_H + ((slot / 3) % 2) * 300 + (slot % 3) * 24;
    if (top->x + s->width > srv->width) {
        top->x = srv->width - s->width - BORDER > 0 ? srv->width - s->width - BORDER : 0;
    }
    if (top->y + s->height > srv->height) {
        top->y = srv->height - s->height - BORDER > TITLE_H ? srv->height - s->height - BORDER
                                                            : TITLE_H + BORDER;
    }
    top->mapped = 1;
    wl_list_remove(&top->link);
    wl_list_insert(&srv->stack, &top->link);
    struct box cb = close_box(top);
    printf("[peetwl] mapped '%s' content=%d,%d,%dx%d title=%d,%d close=%d,%d\n", top->title,
           top->x, top->y, s->width, s->height, top->x + 40, top->y - TITLE_H / 2,
           cb.x + CLOSE_SIZE / 2, cb.y + CLOSE_SIZE / 2);
    damage_toplevel(srv, top);
    set_keyboard_focus(srv, s);
    update_pointer_focus(srv);
}

static void unmap_toplevel(struct server *srv, struct toplevel *top) {
    if (!top->mapped) {
        return;
    }
    damage_toplevel(srv, top);
    top->mapped = 0;
    if (srv->grab == top) {
        srv->grab = NULL;
    }
    printf("[peetwl] unmapped '%s'\n", top->title);
    struct surface *s = top->xdg != NULL ? top->xdg->surface : NULL;
    if (s != NULL && srv->pointer_focus == s) {
        set_pointer_focus(srv, NULL, 0, 0);
    }
    if (s != NULL && srv->keyboard_focus == s) {
        struct toplevel *next = topmost_mapped(srv);
        set_keyboard_focus(srv, next != NULL ? next->xdg->surface : NULL);
    }
    update_pointer_focus(srv);
}

static void toplevel_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void toplevel_set_parent(struct wl_client *c, struct wl_resource *r,
                                struct wl_resource *parent) {
    (void)c;
    (void)r;
    (void)parent;
}

static void toplevel_set_title(struct wl_client *c, struct wl_resource *r, const char *title) {
    (void)c;
    struct toplevel *top = wl_resource_get_user_data(r);
    snprintf(top->title, sizeof(top->title), "%s", title);
    if (top->xdg != NULL && top->xdg->surface != NULL) {
        damage_toplevel(top->xdg->surface->srv, top);
    }
}

static void toplevel_set_app_id(struct wl_client *c, struct wl_resource *r, const char *id) {
    (void)c;
    (void)r;
    (void)id;
}

static void toplevel_show_window_menu(struct wl_client *c, struct wl_resource *r,
                                      struct wl_resource *seat, uint32_t serial, int32_t x,
                                      int32_t y) {
    (void)c;
    (void)r;
    (void)seat;
    (void)serial;
    (void)x;
    (void)y;
}

static void toplevel_move(struct wl_client *c, struct wl_resource *r, struct wl_resource *seat,
                          uint32_t serial) {
    (void)c;
    (void)seat;
    (void)serial;
    struct toplevel *top = wl_resource_get_user_data(r);
    if (top->xdg == NULL || top->xdg->surface == NULL || !top->mapped) {
        return;
    }
    struct server *srv = top->xdg->surface->srv;
    /* Clients request a move while the button that started it is held. */
    if (srv->buttons != 0 && srv->grab == NULL) {
        srv->grab = top;
        srv->grab_dx = srv->cursor_x - top->x;
        srv->grab_dy = srv->cursor_y - top->y;
        set_pointer_focus(srv, NULL, 0, 0);
    }
}

static void toplevel_resize(struct wl_client *c, struct wl_resource *r, struct wl_resource *seat,
                            uint32_t serial, uint32_t edges) {
    (void)c;
    (void)r;
    (void)seat;
    (void)serial;
    (void)edges;
}

static void toplevel_set_size(struct wl_client *c, struct wl_resource *r, int32_t w, int32_t h) {
    (void)c;
    (void)r;
    (void)w;
    (void)h;
}

static void toplevel_noop(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    (void)r;
}

static void toplevel_set_fullscreen(struct wl_client *c, struct wl_resource *r,
                                    struct wl_resource *output) {
    (void)c;
    (void)r;
    (void)output;
}

static const struct xdg_toplevel_interface toplevel_impl = {
    .destroy = toplevel_destroy,
    .set_parent = toplevel_set_parent,
    .set_title = toplevel_set_title,
    .set_app_id = toplevel_set_app_id,
    .show_window_menu = toplevel_show_window_menu,
    .move = toplevel_move,
    .resize = toplevel_resize,
    .set_max_size = toplevel_set_size,
    .set_min_size = toplevel_set_size,
    .set_maximized = toplevel_noop,
    .unset_maximized = toplevel_noop,
    .set_fullscreen = toplevel_set_fullscreen,
    .unset_fullscreen = toplevel_noop,
    .set_minimized = toplevel_noop,
};

static void toplevel_resource_destroyed(struct wl_resource *r) {
    struct toplevel *top = wl_resource_get_user_data(r);
    if (top->xdg != NULL && top->xdg->surface != NULL) {
        unmap_toplevel(top->xdg->surface->srv, top);
    }
    if (top->xdg != NULL) {
        top->xdg->toplevel = NULL;
    }
    wl_list_remove(&top->link);
    free(top);
}

static void xdg_surface_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void xdg_surface_get_toplevel(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct xdg_surface_state *xs = wl_resource_get_user_data(r);
    if (xs->toplevel != NULL || xs->surface == NULL) {
        wl_resource_post_error(r, XDG_SURFACE_ERROR_ALREADY_CONSTRUCTED,
                               "xdg_surface already has a role");
        return;
    }
    struct toplevel *top = calloc(1, sizeof(*top));
    struct wl_resource *res =
        wl_resource_create(c, &xdg_toplevel_interface, wl_resource_get_version(r), id);
    if (top == NULL || res == NULL) {
        free(top);
        wl_client_post_no_memory(c);
        return;
    }
    top->resource = res;
    top->xdg = xs;
    snprintf(top->title, sizeof(top->title), "untitled");
    wl_list_insert(xs->surface->srv->stack.prev, &top->link);
    xs->toplevel = top;
    wl_resource_set_implementation(res, &toplevel_impl, top, toplevel_resource_destroyed);
}

static void popup_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void popup_grab(struct wl_client *c, struct wl_resource *r, struct wl_resource *seat,
                       uint32_t serial) {
    (void)c;
    (void)r;
    (void)seat;
    (void)serial;
}

static void popup_reposition(struct wl_client *c, struct wl_resource *r,
                             struct wl_resource *positioner, uint32_t token) {
    (void)c;
    (void)r;
    (void)positioner;
    (void)token;
}

static const struct xdg_popup_interface popup_impl = {
    .destroy = popup_destroy,
    .grab = popup_grab,
    .reposition = popup_reposition,
};

static void xdg_surface_get_popup(struct wl_client *c, struct wl_resource *r, uint32_t id,
                                  struct wl_resource *parent, struct wl_resource *positioner) {
    (void)parent;
    (void)positioner;
    struct wl_resource *res =
        wl_resource_create(c, &xdg_popup_interface, wl_resource_get_version(r), id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &popup_impl, NULL, NULL);
    /* Popups are not supported yet: dismiss immediately. */
    xdg_popup_send_popup_done(res);
}

static void xdg_surface_set_window_geometry(struct wl_client *c, struct wl_resource *r, int32_t x,
                                            int32_t y, int32_t w, int32_t h) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
}

static void xdg_surface_ack_configure(struct wl_client *c, struct wl_resource *r,
                                      uint32_t serial) {
    (void)c;
    struct xdg_surface_state *xs = wl_resource_get_user_data(r);
    if (!xs->configure_sent) {
        wl_resource_post_error(r, XDG_SURFACE_ERROR_INVALID_SERIAL, "no configure sent");
        return;
    }
    (void)serial;
    xs->acked = 1;
}

static const struct xdg_surface_interface xdg_surface_impl = {
    .destroy = xdg_surface_destroy,
    .get_toplevel = xdg_surface_get_toplevel,
    .get_popup = xdg_surface_get_popup,
    .set_window_geometry = xdg_surface_set_window_geometry,
    .ack_configure = xdg_surface_ack_configure,
};

static void xdg_surface_resource_destroyed(struct wl_resource *r) {
    struct xdg_surface_state *xs = wl_resource_get_user_data(r);
    if (xs->toplevel != NULL && xs->surface != NULL) {
        unmap_toplevel(xs->surface->srv, xs->toplevel);
    }
    if (xs->toplevel != NULL) {
        xs->toplevel->xdg = NULL;
    }
    if (xs->surface != NULL) {
        xs->surface->xdg = NULL;
    }
    free(xs);
}

static void positioner_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void positioner_set_size(struct wl_client *c, struct wl_resource *r, int32_t w, int32_t h) {
    (void)c;
    (void)r;
    (void)w;
    (void)h;
}

static void positioner_set_rect(struct wl_client *c, struct wl_resource *r, int32_t x, int32_t y,
                                int32_t w, int32_t h) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
}

static void positioner_set_u32(struct wl_client *c, struct wl_resource *r, uint32_t v) {
    (void)c;
    (void)r;
    (void)v;
}

static void positioner_set_offset(struct wl_client *c, struct wl_resource *r, int32_t x,
                                  int32_t y) {
    (void)c;
    (void)r;
    (void)x;
    (void)y;
}

static void positioner_noop(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    (void)r;
}

static const struct xdg_positioner_interface positioner_impl = {
    .destroy = positioner_destroy,
    .set_size = positioner_set_size,
    .set_anchor_rect = positioner_set_rect,
    .set_anchor = positioner_set_u32,
    .set_gravity = positioner_set_u32,
    .set_constraint_adjustment = positioner_set_u32,
    .set_offset = positioner_set_offset,
    .set_reactive = positioner_noop,
    .set_parent_size = positioner_set_offset,
    .set_parent_configure = positioner_set_u32,
};

static void wm_base_destroy(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static void wm_base_create_positioner(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct wl_resource *res =
        wl_resource_create(c, &xdg_positioner_interface, wl_resource_get_version(r), id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &positioner_impl, NULL, NULL);
}

static void wm_base_get_xdg_surface(struct wl_client *c, struct wl_resource *r, uint32_t id,
                                    struct wl_resource *surface_res) {
    struct surface *s = wl_resource_get_user_data(surface_res);
    if (s->xdg != NULL) {
        wl_resource_post_error(r, XDG_WM_BASE_ERROR_ROLE, "surface already has an xdg_surface");
        return;
    }
    if (s->pixels != NULL) {
        wl_resource_post_error(r, XDG_WM_BASE_ERROR_INVALID_SURFACE_STATE,
                               "surface already has a buffer");
        return;
    }
    struct xdg_surface_state *xs = calloc(1, sizeof(*xs));
    struct wl_resource *res =
        wl_resource_create(c, &xdg_surface_interface, wl_resource_get_version(r), id);
    if (xs == NULL || res == NULL) {
        free(xs);
        wl_client_post_no_memory(c);
        return;
    }
    xs->resource = res;
    xs->surface = s;
    s->xdg = xs;
    wl_resource_set_implementation(res, &xdg_surface_impl, xs, xdg_surface_resource_destroyed);
}

static void wm_base_pong(struct wl_client *c, struct wl_resource *r, uint32_t serial) {
    (void)c;
    (void)r;
    (void)serial;
}

static const struct xdg_wm_base_interface wm_base_impl = {
    .destroy = wm_base_destroy,
    .create_positioner = wm_base_create_positioner,
    .get_xdg_surface = wm_base_get_xdg_surface,
    .pong = wm_base_pong,
};

static void bind_wm_base(struct wl_client *c, void *data, uint32_t version, uint32_t id) {
    struct wl_resource *res = wl_resource_create(c, &xdg_wm_base_interface, version, id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &wm_base_impl, data, NULL);
}

/* ------------------------------------------------------------------ */
/* wl_seat / wl_pointer / wl_keyboard                                  */
/* ------------------------------------------------------------------ */

static void resource_unlink(struct wl_resource *r) {
    wl_list_remove(wl_resource_get_link(r));
}

static void pointer_set_cursor(struct wl_client *c, struct wl_resource *r, uint32_t serial,
                               struct wl_resource *surface, int32_t hx, int32_t hy) {
    (void)c;
    (void)r;
    (void)serial;
    (void)surface;
    (void)hx;
    (void)hy;
    /* The compositor always draws its own cursor. */
}

static void resource_release(struct wl_client *c, struct wl_resource *r) {
    (void)c;
    wl_resource_destroy(r);
}

static const struct wl_pointer_interface pointer_impl = {
    .set_cursor = pointer_set_cursor,
    .release = resource_release,
};

static const struct wl_keyboard_interface keyboard_impl = {
    .release = resource_release,
};

static void seat_get_pointer(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct server *srv = wl_resource_get_user_data(r);
    struct wl_resource *res =
        wl_resource_create(c, &wl_pointer_interface, wl_resource_get_version(r), id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &pointer_impl, srv, resource_unlink);
    wl_list_insert(&srv->pointers, wl_resource_get_link(res));
    if (srv->pointer_focus != NULL && wl_resource_get_client(srv->pointer_focus->resource) == c) {
        struct toplevel *top = srv->pointer_focus->xdg->toplevel;
        wl_pointer_send_enter(res, wl_display_next_serial(srv->display),
                              srv->pointer_focus->resource,
                              wl_fixed_from_int(srv->cursor_x - top->x),
                              wl_fixed_from_int(srv->cursor_y - top->y));
        send_pointer_frame(res);
    }
}

static void seat_get_keyboard(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    struct server *srv = wl_resource_get_user_data(r);
    struct wl_resource *res =
        wl_resource_create(c, &wl_keyboard_interface, wl_resource_get_version(r), id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &keyboard_impl, srv, resource_unlink);
    wl_list_insert(&srv->keyboards, wl_resource_get_link(res));

    if (srv->keymap_fd >= 0) {
        /* libwayland dups the fd when marshalling, so the compositor keeps
         * its sealed copy for later clients. */
        wl_keyboard_send_keymap(res, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, srv->keymap_fd,
                                srv->keymap_size);
    } else {
        /* No keymap could be compiled: raw evdev keycodes. */
        int fd = memfd_create("peetwl-keymap", MFD_CLOEXEC);
        if (fd >= 0) {
            wl_keyboard_send_keymap(res, WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP, fd, 0);
            close(fd);
        }
    }
    if (wl_resource_get_version(res) >= WL_KEYBOARD_REPEAT_INFO_SINCE_VERSION) {
        wl_keyboard_send_repeat_info(res, 25, 600);
    }
    if (srv->keyboard_focus != NULL && wl_resource_get_client(srv->keyboard_focus->resource) == c) {
        send_keyboard_enter(srv, res, srv->keyboard_focus);
    }
}

static void seat_get_touch(struct wl_client *c, struct wl_resource *r, uint32_t id) {
    (void)c;
    (void)id;
    wl_resource_post_error(r, WL_SEAT_ERROR_MISSING_CAPABILITY, "no touch device");
}

static const struct wl_seat_interface seat_impl = {
    .get_pointer = seat_get_pointer,
    .get_keyboard = seat_get_keyboard,
    .get_touch = seat_get_touch,
    .release = resource_release,
};

static void bind_seat(struct wl_client *c, void *data, uint32_t version, uint32_t id) {
    struct wl_resource *res = wl_resource_create(c, &wl_seat_interface, version, id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &seat_impl, data, NULL);
    wl_seat_send_capabilities(res, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);
    if (version >= WL_SEAT_NAME_SINCE_VERSION) {
        wl_seat_send_name(res, "seat0");
    }
}

/* ------------------------------------------------------------------ */
/* wl_output                                                           */
/* ------------------------------------------------------------------ */

static const struct wl_output_interface output_impl = {
    .release = resource_release,
};

static void bind_output(struct wl_client *c, void *data, uint32_t version, uint32_t id) {
    struct server *srv = data;
    struct wl_resource *res = wl_resource_create(c, &wl_output_interface, version, id);
    if (res == NULL) {
        wl_client_post_no_memory(c);
        return;
    }
    wl_resource_set_implementation(res, &output_impl, srv, NULL);
    /* Physical size assumes ~96 DPI. */
    wl_output_send_geometry(res, 0, 0, srv->width * 254 / 960, srv->height * 254 / 960,
                            WL_OUTPUT_SUBPIXEL_UNKNOWN, "PeetOS", "fb0",
                            WL_OUTPUT_TRANSFORM_NORMAL);
    wl_output_send_mode(res, WL_OUTPUT_MODE_CURRENT | WL_OUTPUT_MODE_PREFERRED, srv->width,
                        srv->height, 60000);
    if (version >= WL_OUTPUT_SCALE_SINCE_VERSION) {
        wl_output_send_scale(res, 1);
    }
    if (version >= WL_OUTPUT_DONE_SINCE_VERSION) {
        wl_output_send_done(res);
    }
}

/* ------------------------------------------------------------------ */
/* Clients, signals, setup                                             */
/* ------------------------------------------------------------------ */

static void maybe_finish_smoke(struct server *srv) {
    if (srv->smoke && srv->child_count > 0 && srv->children_exited >= srv->child_count) {
        printf("[peetwl] all clients exited (failed=%d)\n", srv->children_failed);
        wl_display_terminate(srv->display);
    }
}

static int on_sigchld(int signum, void *data) {
    (void)signum;
    struct server *srv = data;
    int status = 0;
    pid_t pid;
    while ((pid = waitpid(-1, &status, WNOHANG)) > 0) {
        int ok = WIFEXITED(status) && WEXITSTATUS(status) == 0;
        printf("[peetwl] client exited pid=%d status=%#x\n", (int)pid, status);
        for (int i = 0; i < srv->child_count; i++) {
            if (srv->children[i] == pid) {
                srv->children_exited++;
                if (!ok) {
                    srv->children_failed++;
                }
            }
        }
    }
    maybe_finish_smoke(srv);
    return 0;
}

static int on_terminate(int signum, void *data) {
    struct server *srv = data;
    printf("[peetwl] signal %d, shutting down\n", signum);
    wl_display_terminate(srv->display);
    return 0;
}

static int on_watchdog(void *data) {
    struct server *srv = data;
    printf("[peetwl] FAIL watchdog expired (clients exited %d/%d)\n", srv->children_exited,
           srv->child_count);
    srv->failed = 1;
    wl_display_terminate(srv->display);
    return 0;
}

static void spawn_client(struct server *srv, const char *cmdline) {
    if (srv->child_count >= MAX_CLIENTS) {
        printf("[peetwl] too many clients, ignoring '%s'\n", cmdline);
        return;
    }
    char buf[256];
    snprintf(buf, sizeof(buf), "%s", cmdline);
    char *argv[16];
    int argc = 0;
    for (char *tok = strtok(buf, " "); tok != NULL && argc < 15; tok = strtok(NULL, " ")) {
        argv[argc++] = tok;
    }
    argv[argc] = NULL;
    if (argc == 0) {
        return;
    }
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        printf("[peetwl] fork failed errno=%d\n", errno);
        return;
    }
    if (pid == 0) {
        sigset_t none;
        sigemptyset(&none);
        sigprocmask(SIG_SETMASK, &none, NULL);
        execv(argv[0], argv);
        printf("[peetwl] exec '%s' failed errno=%d\n", argv[0], errno);
        _exit(127);
    }
    srv->children[srv->child_count++] = pid;
    printf("[peetwl] spawned pid=%d '%s'\n", (int)pid, cmdline);
}

/* Compile the keyboard keymap and publish it in a sealed, read-only memfd. */
static void setup_keymap(struct server *srv, const char *layout, const char *variant,
                         const char *options) {
    srv->keymap_fd = -1;
    srv->xkb_ctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    if (srv->xkb_ctx == NULL) {
        printf("[peetwl] xkb context failed; using raw keycodes\n");
        return;
    }
    struct xkb_rule_names names = {
        .rules = NULL, /* NULL fields fall back to XKB_DEFAULT_* / built-ins */
        .model = NULL,
        .layout = layout,
        .variant = variant,
        .options = options,
    };
    srv->keymap = xkb_keymap_new_from_names(srv->xkb_ctx, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (srv->keymap == NULL) {
        printf("[peetwl] keymap compile failed (layout=%s); using raw keycodes\n",
               layout ? layout : "default");
        return;
    }
    char *text = xkb_keymap_get_as_string(srv->keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    if (text == NULL) {
        return;
    }
    size_t size = strlen(text) + 1;
    int fd = memfd_create("peetwl-keymap", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    if (fd < 0 || write(fd, text, size) != (ssize_t)size) {
        printf("[peetwl] keymap memfd failed errno=%d; using raw keycodes\n", errno);
        free(text);
        if (fd >= 0) {
            close(fd);
        }
        return;
    }
    free(text);
    /* Clients may map it MAP_SHARED: make sure nobody can change it. */
    fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_SEAL);
    srv->keymap_fd = fd;
    srv->keymap_size = (uint32_t)size;
    srv->xkb_state = xkb_state_new(srv->keymap);
    xkb_layout_index_t n_layouts = xkb_keymap_num_layouts(srv->keymap);
    printf("[peetwl] keymap layout=%s (%u layout%s, %u bytes)\n",
           n_layouts > 0 ? xkb_keymap_layout_get_name(srv->keymap, 0) : "?", n_layouts,
           n_layouts == 1 ? "" : "s", srv->keymap_size);
}

static int open_framebuffer(struct server *srv) {
    srv->fb_fd = open("/dev/fb0", O_RDWR | O_CLOEXEC);
    if (srv->fb_fd < 0) {
        printf("[peetwl] FAIL open /dev/fb0 errno=%d\n", errno);
        return -1;
    }
    struct fb_info info;
    if (ioctl(srv->fb_fd, FBIOGETINFO, &info) != 0 || info.bpp != 32) {
        printf("[peetwl] FAIL FBIOGETINFO errno=%d bpp=%u\n", errno, info.bpp);
        return -1;
    }
    void *map = mmap(NULL, info.map_size, PROT_READ | PROT_WRITE, MAP_SHARED, srv->fb_fd, 0);
    if (map == MAP_FAILED) {
        printf("[peetwl] FAIL mmap /dev/fb0 errno=%d\n", errno);
        return -1;
    }
    srv->fb = map;
    srv->width = (int)info.width;
    srv->height = (int)info.height;
    srv->stride_px = (int)(info.pitch / 4);
    srv->back = calloc((size_t)srv->width * (size_t)srv->height, 4);
    if (srv->back == NULL) {
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    static struct server srv;
    const char *clients[MAX_CLIENTS];
    int nclients = 0;
    const char *xkb_layout = NULL, *xkb_variant = NULL, *xkb_options = NULL;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--smoke") == 0) {
            srv.smoke = 1;
        } else if (strcmp(argv[i], "--client") == 0 && i + 1 < argc && nclients < MAX_CLIENTS) {
            clients[nclients++] = argv[++i];
        } else if (strcmp(argv[i], "--xkb-layout") == 0 && i + 1 < argc) {
            xkb_layout = argv[++i];
        } else if (strcmp(argv[i], "--xkb-variant") == 0 && i + 1 < argc) {
            xkb_variant = argv[++i];
        } else if (strcmp(argv[i], "--xkb-options") == 0 && i + 1 < argc) {
            xkb_options = argv[++i];
        } else {
            printf("usage: peetwl [--smoke] [--xkb-layout L] [--xkb-variant V] "
                   "[--xkb-options O] [--client \"cmd args\"]...\n");
            return 2;
        }
    }

    wl_list_init(&srv.surfaces);
    wl_list_init(&srv.stack);
    wl_list_init(&srv.frame_callbacks);
    wl_list_init(&srv.pointers);
    wl_list_init(&srv.keyboards);

    if (open_framebuffer(&srv) != 0) {
        return 1;
    }
    srv.cursor_x = srv.width / 2;
    srv.cursor_y = srv.height / 2;
    setup_keymap(&srv, xkb_layout, xkb_variant, xkb_options);

    setenv("XDG_RUNTIME_DIR", RUNTIME_DIR, 0);
    srv.display = wl_display_create();
    if (srv.display == NULL) {
        printf("[peetwl] FAIL wl_display_create\n");
        return 1;
    }
    const char *socket_name = wl_display_add_socket_auto(srv.display);
    if (socket_name == NULL) {
        printf("[peetwl] FAIL add_socket errno=%d\n", errno);
        return 1;
    }
    setenv("WAYLAND_DISPLAY", socket_name, 1);
    srv.loop = wl_display_get_event_loop(srv.display);

    wl_display_init_shm(srv.display);
    wl_global_create(srv.display, &wl_compositor_interface, 4, &srv, bind_compositor);
    wl_global_create(srv.display, &wl_seat_interface, 5, &srv, bind_seat);
    wl_global_create(srv.display, &wl_output_interface, 3, &srv, bind_output);
    wl_global_create(srv.display, &xdg_wm_base_interface, 6, &srv, bind_wm_base);

    srv.repaint_timer = wl_event_loop_add_timer(srv.loop, on_repaint_timer, &srv);
    wl_event_loop_add_signal(srv.loop, SIGCHLD, on_sigchld, &srv);
    wl_event_loop_add_signal(srv.loop, SIGTERM, on_terminate, &srv);
    wl_event_loop_add_signal(srv.loop, SIGINT, on_terminate, &srv);
    if (srv.smoke) {
        struct wl_event_source *wd = wl_event_loop_add_timer(srv.loop, on_watchdog, &srv);
        wl_event_source_timer_update(wd, SMOKE_WATCHDOG_MS);
    }

    int kbd = open("/dev/input/event0", O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    int ptr = open("/dev/input/event1", O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (kbd >= 0) {
        wl_event_loop_add_fd(srv.loop, kbd, WL_EVENT_READABLE, on_keyboard_input, &srv);
    } else {
        printf("[peetwl] no keyboard device (errno=%d)\n", errno);
    }
    if (ptr >= 0) {
        wl_event_loop_add_fd(srv.loop, ptr, WL_EVENT_READABLE, on_pointer_input, &srv);
    } else {
        printf("[peetwl] no pointer device (errno=%d)\n", errno);
    }

    damage_box(&srv, (struct box){0, 0, srv.width, srv.height});
    repaint(&srv);
    printf("[peetwl] ready fb=%dx%d socket=%s/%s\n", srv.width, srv.height, RUNTIME_DIR,
           socket_name);
    if (srv.smoke) {
        printf("[peetwl] cursor %d,%d\n", srv.cursor_x, srv.cursor_y);
    }

    for (int i = 0; i < nclients; i++) {
        spawn_client(&srv, clients[i]);
    }

    wl_display_run(srv.display);

    wl_display_destroy_clients(srv.display);
    wl_display_destroy(srv.display);
    printf("[peetwl] exit frames=%u\n", srv.frame_count);
    if (srv.smoke) {
        if (srv.failed || srv.children_failed != 0) {
            printf("[peetwl] FAIL\n");
            return 1;
        }
        printf("[peetwl] pass\n");
    }
    return 0;
}
