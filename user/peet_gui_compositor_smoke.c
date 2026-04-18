#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#define FBIOGETINFO 0x5001UL
#define FB_FORMAT_XRGB8888 1u
#define EV_KEY 0x01u
#define EV_REL 0x02u
#define KEY_A 30u
#define KEY_C 46u
#define BTN_LEFT 0x110u
#define REL_X 0u
#define REL_Y 1u
#define CLIENT_COUNT 2
#define SURFACE_W 128u
#define SURFACE_H 96u
#define SURFACE_MARGIN 32u
#define MSG_ATTACH 1u
#define MSG_DAMAGE 2u
#define MSG_COMMIT 3u
#define MSG_FOCUS 4u
#define MSG_KEY 5u
#define MSG_DETACH 6u
#define CLIENT_QUEUE_CAP 8

struct FramebufferDeviceInfo {
    uint32_t width;
    uint32_t height;
    uint32_t pitch;
    uint32_t bpp;
    uint32_t format;
    uint64_t map_size;
};

struct LinuxInputEvent {
    int64_t tv_sec;
    int64_t tv_usec;
    uint16_t type;
    uint16_t code;
    int32_t value;
};

struct PeetGuiMsg {
    uint32_t type;
    uint32_t client_id;
    uint32_t arg0;
    uint32_t arg1;
    uint32_t arg2;
    uint32_t arg3;
};

struct SurfaceState {
    uint32_t client_id;
    uint32_t x;
    uint32_t y;
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t color;
    uint32_t alt_color;
    uint32_t expected_key;
    uint8_t *buffer;
    size_t buffer_len;
    uint32_t damage_x;
    uint32_t damage_y;
    uint32_t damage_w;
    uint32_t damage_h;
    int attached;
    int dirty;
    int done;
};

struct LocalClient {
    uint32_t client_id;
    uint32_t expected_key;
    uint32_t color;
    uint32_t alt_color;
    uint8_t *buffer;
    struct PeetGuiMsg to_server[CLIENT_QUEUE_CAP];
    size_t to_server_len;
    size_t to_server_head;
    struct PeetGuiMsg to_client[CLIENT_QUEUE_CAP];
    size_t to_client_len;
    size_t to_client_head;
};

static void write_all(int fd, const char *buf, size_t len) {
    while (len > 0) {
        ssize_t written = write(fd, buf, len);
        if (written <= 0) {
            return;
        }
        buf += (size_t)written;
        len -= (size_t)written;
    }
}

static void log_line(const char *line) {
    write_all(1, line, strlen(line));
}

static void log_errno(const char *tag) {
    char buf[192];
    int n = snprintf(buf, sizeof(buf), "[peet-gui] %s errno=%d\n", tag, errno);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static void log_client_value(uint32_t client_id, const char *tag, uint32_t value) {
    char buf[192];
    int n = snprintf(buf, sizeof(buf), "[peet-gui-client%u] %s=%u\n", client_id, tag, value);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static int require_env_equals(const char *name, const char *expected) {
    const char *value = getenv(name);
    if (value == NULL || strcmp(value, expected) != 0) {
        char buf[256];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[peet-gui] env mismatch %s expected=%s actual=%s\n",
            name,
            expected,
            value == NULL ? "(null)" : value
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
        return -1;
    }
    return 0;
}

static void fill_surface(uint32_t *pixels, uint32_t width, uint32_t height, uint32_t color) {
    for (uint32_t y = 0; y < height; y++) {
        for (uint32_t x = 0; x < width; x++) {
            uint32_t pixel = color;
            if (x < 3 || y < 3 || x + 3 >= width || y + 3 >= height) {
                pixel ^= 0x00181818u;
            }
            pixels[(size_t)y * width + x] = pixel;
        }
    }
}

static void fill_rect(
    uint32_t *pixels,
    uint32_t width,
    uint32_t rect_x,
    uint32_t rect_y,
    uint32_t rect_w,
    uint32_t rect_h,
    uint32_t color
) {
    for (uint32_t y = rect_y; y < rect_y + rect_h; y++) {
        for (uint32_t x = rect_x; x < rect_x + rect_w; x++) {
            pixels[(size_t)y * width + x] = color;
        }
    }
}

static int queue_push(struct PeetGuiMsg *queue, size_t *len, const struct PeetGuiMsg *msg) {
    if (*len >= CLIENT_QUEUE_CAP) {
        return -1;
    }
    queue[*len] = *msg;
    *len += 1;
    return 0;
}

static int queue_pop(struct PeetGuiMsg *queue, size_t *len, size_t *head, struct PeetGuiMsg *msg) {
    if (*head >= *len) {
        return 0;
    }
    *msg = queue[*head];
    *head += 1;
    if (*head == *len) {
        *head = 0;
        *len = 0;
    }
    return 1;
}

static int local_client_send_attach(struct LocalClient *client, uint32_t x, uint32_t y) {
    size_t map_len = (size_t)SURFACE_W * SURFACE_H * sizeof(uint32_t);
    uint32_t *pixels = malloc(map_len);
    if (pixels == NULL) {
        return -1;
    }

    fill_surface(pixels, SURFACE_W, SURFACE_H, client->color);
    client->buffer = (uint8_t *)pixels;

    struct PeetGuiMsg attach;
    memset(&attach, 0, sizeof(attach));
    attach.type = MSG_ATTACH;
    attach.client_id = client->client_id;
    attach.arg0 = x;
    attach.arg1 = y;
    attach.arg2 = SURFACE_W;
    attach.arg3 = SURFACE_H;
    return queue_push(client->to_server, &client->to_server_len, &attach);
}

static int local_client_send_damage_and_commit(
    struct LocalClient *client,
    uint32_t x,
    uint32_t y,
    uint32_t width,
    uint32_t height
) {
    struct PeetGuiMsg damage;
    struct PeetGuiMsg commit;

    memset(&damage, 0, sizeof(damage));
    damage.type = MSG_DAMAGE;
    damage.client_id = client->client_id;
    damage.arg0 = x;
    damage.arg1 = y;
    damage.arg2 = width;
    damage.arg3 = height;
    if (queue_push(client->to_server, &client->to_server_len, &damage) < 0) {
        return -1;
    }

    memset(&commit, 0, sizeof(commit));
    commit.type = MSG_COMMIT;
    commit.client_id = client->client_id;
    return queue_push(client->to_server, &client->to_server_len, &commit);
}

static int local_client_process_server_messages(struct LocalClient *client, int *done) {
    struct PeetGuiMsg msg;
    while (queue_pop(client->to_client, &client->to_client_len, &client->to_client_head, &msg) > 0) {
        if (msg.type == MSG_FOCUS) {
            log_client_value(client->client_id, "focus", msg.arg0);
        } else if (msg.type == MSG_KEY && msg.arg0 == client->expected_key) {
            log_client_value(client->client_id, "received-key", client->expected_key);
            *done = 1;
            if (client->client_id == 2) {
                struct PeetGuiMsg detach;
                memset(&detach, 0, sizeof(detach));
                detach.type = MSG_DETACH;
                detach.client_id = client->client_id;
                if (queue_push(client->to_server, &client->to_server_len, &detach) < 0) {
                    return -1;
                }
                log_client_value(client->client_id, "disconnect-requested", 1);
            }
        }
    }
    return 0;
}

static uint32_t *framebuffer_pixel(uint8_t *fb, const struct FramebufferDeviceInfo *info, uint32_t x, uint32_t y) {
    return (uint32_t *)(fb + (size_t)y * info->pitch + (size_t)x * sizeof(uint32_t));
}

static void clear_framebuffer(uint8_t *fb, const struct FramebufferDeviceInfo *info, uint32_t color) {
    for (uint32_t y = 0; y < info->height; y++) {
        uint32_t *row = (uint32_t *)(fb + (size_t)y * info->pitch);
        for (uint32_t x = 0; x < info->width; x++) {
            row[x] = color;
        }
    }
}

static void draw_border(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    uint32_t x,
    uint32_t y,
    uint32_t width,
    uint32_t height,
    uint32_t color
) {
    for (uint32_t dy = 0; dy < height + 4; dy++) {
        uint32_t py = y + dy - 2;
        if (py >= info->height) {
            continue;
        }
        for (uint32_t dx = 0; dx < width + 4; dx++) {
            uint32_t px = x + dx - 2;
            if (px >= info->width) {
                continue;
            }
            if (dy < 2 || dy + 2 >= height + 4 || dx < 2 || dx + 2 >= width + 4) {
                *framebuffer_pixel(fb, info, px, py) = color;
            }
        }
    }
}

static void blit_surface_damage(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    const struct SurfaceState *surface
) {
    uint32_t start_x = surface->damage_x;
    uint32_t start_y = surface->damage_y;
    uint32_t end_x = start_x + surface->damage_w;
    uint32_t end_y = start_y + surface->damage_h;

    for (uint32_t row = start_y; row < end_y; row++) {
        uint32_t *dst = framebuffer_pixel(fb, info, surface->x + start_x, surface->y + row);
        const uint32_t *src = (const uint32_t *)(surface->buffer + (size_t)row * surface->stride) + start_x;
        memcpy(dst, src, (size_t)(end_x - start_x) * sizeof(uint32_t));
    }
}

static void compose_full_framebuffer(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    struct SurfaceState *surfaces,
    const int *z_order,
    int focused
) {
    clear_framebuffer(fb, info, 0x00101010u);
    for (int i = 0; i < CLIENT_COUNT; i++) {
        int surface_index = z_order[i];
        if (!surfaces[surface_index].attached || surfaces[surface_index].buffer == NULL) {
            continue;
        }
        surfaces[surface_index].damage_x = 0;
        surfaces[surface_index].damage_y = 0;
        surfaces[surface_index].damage_w = surfaces[surface_index].width;
        surfaces[surface_index].damage_h = surfaces[surface_index].height;
        blit_surface_damage(fb, info, &surfaces[surface_index]);
    }
    for (int i = 0; i < CLIENT_COUNT; i++) {
        if (!surfaces[i].attached || surfaces[i].buffer == NULL) {
            continue;
        }
        draw_border(
            fb,
            info,
            surfaces[i].x,
            surfaces[i].y,
            surfaces[i].width,
            surfaces[i].height,
            i == focused ? 0x00ffffffu : 0x00606060u
        );
    }
}

static int point_is_covered(
    const struct SurfaceState *surfaces,
    const int *z_order,
    int surface_index,
    uint32_t x,
    uint32_t y
) {
    int seen_surface = 0;
    for (int i = 0; i < CLIENT_COUNT; i++) {
        int candidate = z_order[i];
        if (candidate == surface_index) {
            seen_surface = 1;
            continue;
        }
        if (!seen_surface || !surfaces[candidate].attached || surfaces[candidate].buffer == NULL) {
            continue;
        }
        if (x >= surfaces[candidate].x &&
            x < surfaces[candidate].x + surfaces[candidate].width &&
            y >= surfaces[candidate].y &&
            y < surfaces[candidate].y + surfaces[candidate].height) {
            return 1;
        }
    }
    return 0;
}

static int surface_visible_probe(
    const struct SurfaceState *surfaces,
    const int *z_order,
    int surface_index,
    uint32_t *out_x,
    uint32_t *out_y
) {
    static const uint32_t offsets[][2] = {
        {8u, 8u},
        {SURFACE_W - 9u, 8u},
        {8u, SURFACE_H - 9u},
        {SURFACE_W - 9u, SURFACE_H - 9u},
        {SURFACE_W / 2u, SURFACE_H / 2u},
    };

    for (size_t i = 0; i < sizeof(offsets) / sizeof(offsets[0]); i++) {
        uint32_t x = surfaces[surface_index].x + offsets[i][0];
        uint32_t y = surfaces[surface_index].y + offsets[i][1];
        if (!point_is_covered(surfaces, z_order, surface_index, x, y)) {
            *out_x = x;
            *out_y = y;
            return 0;
        }
    }

    return -1;
}

static int validate_surface_pixels(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    const struct SurfaceState *surfaces,
    const int *z_order,
    int focused
) {
    for (int i = 0; i < CLIENT_COUNT; i++) {
        uint32_t probe_x;
        uint32_t probe_y;
        uint32_t fill;
        uint32_t border;

        if (!surfaces[i].attached || surfaces[i].buffer == NULL) {
            continue;
        }
        if (surface_visible_probe(surfaces, z_order, i, &probe_x, &probe_y) < 0) {
            return -1;
        }
        fill = *framebuffer_pixel(fb, info, probe_x, probe_y);
        border = *framebuffer_pixel(fb, info, surfaces[i].x - 1, surfaces[i].y - 1);
        if (fill != surfaces[i].color || border != (i == focused ? 0x00ffffffu : 0x00606060u)) {
            return -1;
        }
    }
    return 0;
}

static void raise_surface_to_front(int *z_order, int surface_index) {
    int next[CLIENT_COUNT];
    int out = 0;
    for (int i = 0; i < CLIENT_COUNT; i++) {
        if (z_order[i] != surface_index) {
            next[out++] = z_order[i];
        }
    }
    next[out] = surface_index;
    for (int i = 0; i < CLIENT_COUNT; i++) {
        z_order[i] = next[i];
    }
}

static int validate_overlap_top(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    const struct SurfaceState *surfaces,
    int expected_top_surface
) {
    uint32_t probe_x = surfaces[1].x + 16;
    uint32_t probe_y = surfaces[1].y + 16;
    uint32_t expected = surfaces[expected_top_surface].color;
    return *framebuffer_pixel(fb, info, probe_x, probe_y) == expected ? 0 : -1;
}

static int validate_damage_update(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    const struct SurfaceState *surface,
    uint32_t rect_x,
    uint32_t rect_y,
    uint32_t rect_w,
    uint32_t rect_h
) {
    uint32_t inside = *framebuffer_pixel(fb, info, surface->x + rect_x + 2, surface->y + rect_y + 2);
    uint32_t outside = *framebuffer_pixel(fb, info, surface->x + rect_x - 2, surface->y + rect_y - 2);
    return inside == surface->alt_color && outside == surface->color ? 0 : -1;
}

static int validate_surface_absent(
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    const struct SurfaceState *surfaces,
    int absent_surface
) {
    static const uint32_t offsets[][2] = {
        {8u, 8u},
        {SURFACE_W - 9u, 8u},
        {8u, SURFACE_H - 9u},
        {SURFACE_W - 9u, SURFACE_H - 9u},
        {SURFACE_W / 2u, SURFACE_H / 2u},
    };
    const struct SurfaceState *surface = &surfaces[absent_surface];

    if (surface->width == 0 || surface->height == 0 || surface->buffer != NULL) {
        return -1;
    }
    for (size_t i = 0; i < sizeof(offsets) / sizeof(offsets[0]); i++) {
        uint32_t x = surface->x + offsets[i][0];
        uint32_t y = surface->y + offsets[i][1];
        int covered = 0;
        for (int j = 0; j < CLIENT_COUNT; j++) {
            if (j == absent_surface || !surfaces[j].attached || surfaces[j].buffer == NULL) {
                continue;
            }
            if (x >= surfaces[j].x &&
                x < surfaces[j].x + surfaces[j].width &&
                y >= surfaces[j].y &&
                y < surfaces[j].y + surfaces[j].height) {
                covered = 1;
                break;
            }
        }
        if (!covered) {
            return *framebuffer_pixel(fb, info, x, y) == 0x00101010u ? 0 : -1;
        }
    }
    return -1;
}

static int all_surfaces_done(const struct SurfaceState *surfaces) {
    for (int i = 0; i < CLIENT_COUNT; i++) {
        if (!surfaces[i].done) {
            return 0;
        }
    }
    return 1;
}

static int next_focus(int current) {
    return current == 0 ? 1 : 0;
}

static int first_attached_surface(const struct SurfaceState *surfaces) {
    for (int i = 0; i < CLIENT_COUNT; i++) {
        if (surfaces[i].attached && surfaces[i].buffer != NULL) {
            return i;
        }
    }
    return -1;
}

static int server_send_focus(struct LocalClient *clients, int focused) {
    for (int i = 0; i < CLIENT_COUNT; i++) {
        struct PeetGuiMsg msg;
        memset(&msg, 0, sizeof(msg));
        msg.type = MSG_FOCUS;
        msg.client_id = clients[i].client_id;
        msg.arg0 = focused >= 0 && i == focused ? 1u : 0u;
        if (queue_push(clients[i].to_client, &clients[i].to_client_len, &msg) < 0) {
            return -1;
        }
    }
    return 0;
}

static int server_send_key(struct LocalClient *client, uint32_t key_code) {
    struct PeetGuiMsg msg;
    memset(&msg, 0, sizeof(msg));
    msg.type = MSG_KEY;
    msg.client_id = client->client_id;
    msg.arg0 = key_code;
    return queue_push(client->to_client, &client->to_client_len, &msg);
}

static int server_drain_client_messages(
    struct LocalClient *client,
    struct SurfaceState *surface,
    uint8_t *fb,
    const struct FramebufferDeviceInfo *info,
    int *detached
) {
    struct PeetGuiMsg msg;
    while (queue_pop(client->to_server, &client->to_server_len, &client->to_server_head, &msg) > 0) {
        if (msg.type == MSG_ATTACH) {
            surface->x = msg.arg0;
            surface->y = msg.arg1;
            surface->width = msg.arg2;
            surface->height = msg.arg3;
            surface->stride = surface->width * sizeof(uint32_t);
            surface->damage_x = 0;
            surface->damage_y = 0;
            surface->damage_w = surface->width;
            surface->damage_h = surface->height;
            surface->attached = 1;
            surface->buffer = client->buffer;
            {
                char buf[160];
                int n = snprintf(
                    buf,
                    sizeof(buf),
                    "[peet-gui] client%u attached x=%u y=%u w=%u h=%u\n",
                    surface->client_id,
                    surface->x,
                    surface->y,
                    surface->width,
                    surface->height
                );
                if (n > 0) {
                    write_all(1, buf, (size_t)n);
                }
            }
        } else if (msg.type == MSG_DAMAGE) {
            surface->damage_x = msg.arg0;
            surface->damage_y = msg.arg1;
            surface->damage_w = msg.arg2;
            surface->damage_h = msg.arg3;
            {
                char buf[160];
                int n = snprintf(
                    buf,
                    sizeof(buf),
                    "[peet-gui] client%u damage x=%u y=%u w=%u h=%u\n",
                    surface->client_id,
                    surface->damage_x,
                    surface->damage_y,
                    surface->damage_w,
                    surface->damage_h
                );
                if (n > 0) {
                    write_all(1, buf, (size_t)n);
                }
            }
        } else if (msg.type == MSG_COMMIT) {
            if (surface->attached && surface->damage_w > 0 && surface->damage_h > 0) {
                blit_surface_damage(fb, info, surface);
                surface->dirty = 1;
                {
                    char buf[128];
                    int n = snprintf(buf, sizeof(buf), "[peet-gui] client%u commit\n", surface->client_id);
                    if (n > 0) {
                        write_all(1, buf, (size_t)n);
                    }
                }
            }
        } else if (msg.type == MSG_DETACH) {
            surface->attached = 0;
            surface->dirty = 0;
            surface->buffer = NULL;
            surface->damage_w = 0;
            surface->damage_h = 0;
            *detached = 1;
            {
                char buf[96];
                int n = snprintf(buf, sizeof(buf), "[peet-gui] client%u detached\n", surface->client_id);
                if (n > 0) {
                    write_all(1, buf, (size_t)n);
                }
            }
        }
    }
    return 0;
}

int main(void) {
    const char *fb_path = getenv("PEETOS_FB");
    const char *kbd_path = getenv("PEETOS_INPUT_KEYBOARD");
    const char *ptr_path = getenv("PEETOS_INPUT_POINTER");
    struct FramebufferDeviceInfo info;
    struct SurfaceState surfaces[CLIENT_COUNT];
    struct LocalClient clients[CLIENT_COUNT];
    int fb_fd = -1;
    int kbd_fd = -1;
    int ptr_fd = -1;
    int epfd = -1;
    int focused = 0;
    int z_order[CLIENT_COUNT] = {1, 0};
    int disconnected_client = 0;
    int saw_pointer_activity = 0;
    int rounds = 0;
    uint8_t *fb = MAP_FAILED;
    const uint32_t damage_x = 24;
    const uint32_t damage_y = 20;
    const uint32_t damage_w = 40;
    const uint32_t damage_h = 28;

    memset(surfaces, 0, sizeof(surfaces));
    memset(clients, 0, sizeof(clients));
    surfaces[0].client_id = 1;
    surfaces[0].color = 0x00d04040u;
    surfaces[0].alt_color = 0x00d0d040u;
    surfaces[0].expected_key = KEY_A;
    surfaces[1].client_id = 2;
    surfaces[1].color = 0x004060d0u;
    surfaces[1].alt_color = 0x0040d090u;
    surfaces[1].expected_key = KEY_C;
    clients[0].client_id = 1;
    clients[0].expected_key = KEY_A;
    clients[0].color = surfaces[0].color;
    clients[0].alt_color = surfaces[0].alt_color;
    clients[1].client_id = 2;
    clients[1].expected_key = KEY_C;
    clients[1].color = surfaces[1].color;
    clients[1].alt_color = surfaces[1].alt_color;

    if (require_env_equals("PEETOS_DISPLAY_MODE", "compositor-first") < 0 ||
        require_env_equals("PEETOS_FB", "/dev/fb0") < 0 ||
        require_env_equals("PEETOS_INPUT_KEYBOARD", "/dev/input/event0") < 0 ||
        require_env_equals("PEETOS_INPUT_POINTER", "/dev/input/event1") < 0) {
        return 1;
    }

    fb_fd = open(fb_path, O_RDWR);
    if (fb_fd < 0) {
        log_errno("open framebuffer failed");
        return 2;
    }
    if (ioctl(fb_fd, FBIOGETINFO, &info) < 0) {
        log_errno("FBIOGETINFO failed");
        return 3;
    }
    if (info.format != FB_FORMAT_XRGB8888 || info.bpp != 32u) {
        log_line("[peet-gui] unexpected framebuffer format\n");
        return 4;
    }
    fb = mmap(NULL, (size_t)info.map_size, PROT_READ | PROT_WRITE, MAP_SHARED, fb_fd, 0);
    if (fb == MAP_FAILED) {
        log_errno("framebuffer mmap failed");
        return 5;
    }

    kbd_fd = open(kbd_path, O_RDONLY | O_NONBLOCK);
    ptr_fd = open(ptr_path, O_RDONLY | O_NONBLOCK);
    if (kbd_fd < 0 || ptr_fd < 0) {
        log_errno("open input device failed");
        return 6;
    }

    for (int i = 0; i < CLIENT_COUNT; i++) {
        surfaces[i].buffer_len = (size_t)SURFACE_W * SURFACE_H * sizeof(uint32_t);
    }

    epfd = epoll_create1(0);
    if (epfd < 0) {
        log_errno("epoll_create1 failed");
        return 7;
    }

    {
        struct epoll_event ev;
        memset(&ev, 0, sizeof(ev));
        ev.events = EPOLLIN;
        ev.data.u32 = 1;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, kbd_fd, &ev) < 0) {
            log_errno("epoll_ctl add keyboard failed");
            return 8;
        }
        ev.data.u32 = 2;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, ptr_fd, &ev) < 0) {
            log_errno("epoll_ctl add pointer failed");
            return 9;
        }
    }

    {
        uint32_t y = info.height > (SURFACE_H + SURFACE_MARGIN * 2)
            ? (info.height - SURFACE_H) / 2
            : SURFACE_MARGIN;
        if (local_client_send_attach(&clients[0], SURFACE_MARGIN, y) < 0 ||
            local_client_send_attach(&clients[1], SURFACE_MARGIN + 64, y + 24) < 0 ||
            local_client_send_damage_and_commit(&clients[0], 0, 0, SURFACE_W, SURFACE_H) < 0 ||
            local_client_send_damage_and_commit(&clients[1], 0, 0, SURFACE_W, SURFACE_H) < 0) {
            log_line("[peet-gui] local client bootstrap failed\n");
            return 10;
        }
    }

    clear_framebuffer(fb, &info, 0x00101010u);
    for (int i = 0; i < CLIENT_COUNT; i++) {
        int detached = 0;
        if (server_drain_client_messages(&clients[i], &surfaces[i], fb, &info, &detached) < 0) {
            return 11;
        }
    }

    compose_full_framebuffer(fb, &info, surfaces, z_order, focused);
    if (validate_surface_pixels(fb, &info, surfaces, z_order, focused) < 0 ||
        validate_overlap_top(fb, &info, surfaces, 0) < 0) {
        log_line("[peet-gui] initial composition validation failed\n");
        return 12;
    }

    fill_rect(
        (uint32_t *)clients[0].buffer,
        SURFACE_W,
        damage_x,
        damage_y,
        damage_w,
        damage_h,
        clients[0].alt_color
    );
    if (local_client_send_damage_and_commit(&clients[0], damage_x, damage_y, damage_w, damage_h) < 0) {
        log_line("[peet-gui] local damage commit failed\n");
        return 13;
    }
    {
        int detached = 0;
        if (server_drain_client_messages(&clients[0], &surfaces[0], fb, &info, &detached) < 0) {
            return 14;
        }
    }
    if (validate_damage_update(fb, &info, &surfaces[0], damage_x, damage_y, damage_w, damage_h) < 0) {
        log_line("[peet-gui] damage validation failed\n");
        return 15;
    }
    {
        char buf[64];
        int n = snprintf(buf, sizeof(buf), "[peet-gui] overlap-top=%d\n", z_order[CLIENT_COUNT - 1] + 1);
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    log_line("[peet-gui] damage-validated=1\n");

    if (server_send_focus(clients, focused) < 0) {
        return 16;
    }
    for (int i = 0; i < CLIENT_COUNT; i++) {
        if (local_client_process_server_messages(&clients[i], &surfaces[i].done) < 0) {
            return 17;
        }
        surfaces[i].done = 0;
    }

    {
        char buf[192];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[peet-gui] composed surfaces=2 framebuffer=%ux%u focus=%d\n",
            info.width,
            info.height,
            focused + 1
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    log_line("[peet-gui] waiting-for-focus-input\n");

    while (!all_surfaces_done(surfaces) && rounds < 24) {
        struct epoll_event ready[4];
        int nready = epoll_wait(epfd, ready, 4, 500);
        if (nready < 0) {
            log_errno("epoll_wait runtime phase failed");
            return 18;
        }
        if (nready == 0) {
            rounds++;
            continue;
        }

        for (int i = 0; i < nready; i++) {
            if (ready[i].data.u32 == 1) {
                struct LinuxInputEvent events[8];
                ssize_t n = read(kbd_fd, events, sizeof(events));
                if (n < 0) {
                    if (errno == EAGAIN) {
                        continue;
                    }
                    log_errno("keyboard read failed");
                    return 19;
                }
                for (ssize_t j = 0; j < n / (ssize_t)sizeof(struct LinuxInputEvent); j++) {
                    if (events[j].type == EV_KEY && events[j].value == 1) {
                        if (server_send_key(&clients[focused], events[j].code) < 0) {
                            return 20;
                        }
                        if (local_client_process_server_messages(&clients[focused], &surfaces[focused].done) < 0) {
                            return 21;
                        }
                        {
                            int detached = 0;
                            if (server_drain_client_messages(&clients[focused], &surfaces[focused], fb, &info, &detached) < 0) {
                                return 22;
                            }
                            if (detached) {
                                disconnected_client = focused + 1;
                                focused = first_attached_surface(surfaces);
                                compose_full_framebuffer(fb, &info, surfaces, z_order, focused);
                                if (focused < 0 || validate_surface_pixels(fb, &info, surfaces, z_order, focused) < 0 ||
                                    validate_surface_absent(fb, &info, surfaces, disconnected_client - 1) < 0 ||
                                    validate_overlap_top(fb, &info, surfaces, focused) < 0) {
                                    log_line("[peet-gui] disconnect redraw validation failed\n");
                                    return 23;
                                }
                                if (server_send_focus(clients, focused) < 0) {
                                    return 24;
                                }
                                for (int k = 0; k < CLIENT_COUNT; k++) {
                                    if (local_client_process_server_messages(&clients[k], &surfaces[k].done) < 0) {
                                        return 25;
                                    }
                                }
                            }
                        }
                    }
                }
            } else if (ready[i].data.u32 == 2) {
                struct LinuxInputEvent events[8];
                ssize_t n = read(ptr_fd, events, sizeof(events));
                if (n < 0) {
                    if (errno == EAGAIN) {
                        continue;
                    }
                    log_errno("pointer read failed");
                    return 26;
                }
                for (ssize_t j = 0; j < n / (ssize_t)sizeof(struct LinuxInputEvent); j++) {
                    if (events[j].type == EV_REL &&
                        ((events[j].code == REL_X) || (events[j].code == REL_Y)) &&
                        events[j].value != 0) {
                        saw_pointer_activity = 1;
                    }
                    if (events[j].type == EV_KEY && events[j].code == BTN_LEFT && events[j].value == 1) {
                        if (first_attached_surface(surfaces) < 0) {
                            log_line("[peet-gui] no attached surfaces remain\n");
                            return 27;
                        }
                        focused = next_focus(focused);
                        if (!surfaces[focused].attached) {
                            focused = first_attached_surface(surfaces);
                        }
                        raise_surface_to_front(z_order, focused);
                        compose_full_framebuffer(fb, &info, surfaces, z_order, focused);
                        if (validate_surface_pixels(fb, &info, surfaces, z_order, focused) < 0 ||
                            validate_overlap_top(fb, &info, surfaces, focused) < 0) {
                            log_line("[peet-gui] focus switch validation failed\n");
                            return 28;
                        }
                        if (server_send_focus(clients, focused) < 0) {
                            return 29;
                        }
                        for (int k = 0; k < CLIENT_COUNT; k++) {
                            if (local_client_process_server_messages(&clients[k], &surfaces[k].done) < 0) {
                                return 30;
                            }
                        }
                        {
                            char buf[96];
                            int out = snprintf(
                                buf,
                                sizeof(buf),
                                "[peet-gui] focus-switched=%d overlap-top=%d\n",
                                focused + 1,
                                z_order[CLIENT_COUNT - 1] + 1
                            );
                            if (out > 0) {
                                write_all(1, buf, (size_t)out);
                            }
                        }
                    }
                }
            }
        }
        rounds++;
    }

    if (!saw_pointer_activity) {
        log_line("[peet-gui] pointer activity was not observed\n");
        return 31;
    }
    if (!all_surfaces_done(surfaces)) {
        log_line("[peet-gui] surfaces did not complete focus/input handshake\n");
        return 32;
    }
    if (disconnected_client != 2) {
        log_line("[peet-gui] expected client2 disconnect was not observed\n");
        return 33;
    }

    {
        char buf[160];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[peet-gui] observed done client1=%d client2=%d pointer=%d disconnect=%d\n",
            surfaces[0].done,
            surfaces[1].done,
            saw_pointer_activity,
            disconnected_client
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    log_line("[peet-gui] pass\n");
    return 0;
}
