#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <fcntl.h>

#define FBIOGETINFO 0x5001UL
#define FB_FORMAT_XRGB8888 1u
#define EV_SYN 0x00u
#define EV_KEY 0x01u
#define EV_REL 0x02u
#define SYN_REPORT 0u
#define KEY_A 30u
#define REL_X 0u
#define REL_Y 1u
#define BTN_LEFT 0x110u

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
    int n = snprintf(buf, sizeof(buf), "[gui-smoke] %s errno=%d\n", tag, errno);
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
            "[gui-smoke] env mismatch %s expected=%s actual=%s\n",
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

static uint32_t stripe_color(uint32_t width, uint32_t x) {
    if (x < width / 3) {
        return 0x00ff0000u;
    }
    if (x < (width / 3) * 2) {
        return 0x0000ff00u;
    }
    return 0x000000ffu;
}

static void draw_pattern(uint8_t *fb, const struct FramebufferDeviceInfo *info) {
    uint32_t center_x0 = info->width / 4;
    uint32_t center_x1 = info->width - center_x0;
    uint32_t center_y0 = info->height / 4;
    uint32_t center_y1 = info->height - center_y0;

    for (uint32_t y = 0; y < info->height; y++) {
        uint8_t *row = fb + (size_t)y * info->pitch;
        for (uint32_t x = 0; x < info->width; x++) {
            uint32_t color = stripe_color(info->width, x);
            if (x >= center_x0 && x < center_x1 && y >= center_y0 && y < center_y1) {
                color ^= 0x00202020u;
            }
            ((uint32_t *)row)[x] = color;
        }
    }
}

static int validate_pattern(uint8_t *fb, const struct FramebufferDeviceInfo *info) {
    uint32_t *top = (uint32_t *)fb;
    uint32_t *mid = (uint32_t *)(fb + (size_t)(info->height / 2) * info->pitch);
    uint32_t *bot = (uint32_t *)(fb + (size_t)(info->height - 1) * info->pitch);

    if (top[0] != stripe_color(info->width, 0)) {
        return -1;
    }
    if (mid[info->width - 1] != stripe_color(info->width, info->width - 1)) {
        return -1;
    }
    if (bot[info->width / 2] == 0) {
        return -1;
    }
    return 0;
}

static int drain_input_once(int fd, const char *tag) {
    struct LinuxInputEvent events[4];
    ssize_t n = read(fd, events, sizeof(events));
    if (n < 0) {
        if (errno == EAGAIN) {
            return 0;
        }
        log_errno(tag);
        return -1;
    }
    if ((n % (ssize_t)sizeof(struct LinuxInputEvent)) != 0) {
        log_line("[gui-smoke] input read returned partial event payload\n");
        return -1;
    }
    if (n > 0) {
        char buf[192];
        int count = (int)(n / (ssize_t)sizeof(struct LinuxInputEvent));
        int out = snprintf(buf, sizeof(buf), "[gui-smoke] drained %s events=%d\n", tag, count);
        if (out > 0) {
            write_all(1, buf, (size_t)out);
        }
    }
    return 0;
}

static int consume_keyboard_events(int fd, int *saw_key_a_press) {
    struct LinuxInputEvent events[8];
    ssize_t n = read(fd, events, sizeof(events));
    if (n < 0) {
        if (errno == EAGAIN) {
            return 0;
        }
        log_errno("keyboard read failed");
        return -1;
    }
    if ((n % (ssize_t)sizeof(struct LinuxInputEvent)) != 0) {
        log_line("[gui-smoke] keyboard read returned partial event payload\n");
        return -1;
    }
    for (ssize_t i = 0; i < n / (ssize_t)sizeof(struct LinuxInputEvent); i++) {
        if (events[i].type == EV_KEY && events[i].code == KEY_A && events[i].value == 1) {
            *saw_key_a_press = 1;
        }
    }
    return 0;
}

static int consume_pointer_events(int fd, int *saw_pointer_motion) {
    struct LinuxInputEvent events[8];
    ssize_t n = read(fd, events, sizeof(events));
    if (n < 0) {
        if (errno == EAGAIN) {
            return 0;
        }
        log_errno("pointer read failed");
        return -1;
    }
    if ((n % (ssize_t)sizeof(struct LinuxInputEvent)) != 0) {
        log_line("[gui-smoke] pointer read returned partial event payload\n");
        return -1;
    }
    for (ssize_t i = 0; i < n / (ssize_t)sizeof(struct LinuxInputEvent); i++) {
        if (events[i].type == EV_REL &&
            ((events[i].code == REL_X) || (events[i].code == REL_Y)) &&
            events[i].value != 0) {
            *saw_pointer_motion = 1;
        }
        if (events[i].type == EV_KEY && events[i].code == BTN_LEFT && events[i].value == 1) {
            *saw_pointer_motion = 1;
        }
    }
    return 0;
}

static int wait_for_input_events(int epfd, int kbd_fd, int ptr_fd) {
    int saw_key_a_press = 0;
    int saw_pointer_motion = 0;
    int rounds = 0;

    log_line("[gui-smoke] waiting-for-input\n");
    while ((!saw_key_a_press || !saw_pointer_motion) && rounds < 12) {
        struct epoll_event ready[4];
        int nready = epoll_wait(epfd, ready, 4, 500);
        if (nready < 0) {
            log_errno("epoll_wait input phase failed");
            return -1;
        }
        if (nready == 0) {
            rounds++;
            continue;
        }
        for (int i = 0; i < nready; i++) {
            if (ready[i].data.u32 == 1) {
                if (consume_keyboard_events(kbd_fd, &saw_key_a_press) < 0) {
                    return -1;
                }
            } else if (ready[i].data.u32 == 2) {
                if (consume_pointer_events(ptr_fd, &saw_pointer_motion) < 0) {
                    return -1;
                }
            }
        }
        rounds++;
    }

    {
        char buf[160];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[gui-smoke] observed keyboard=%d pointer=%d\n",
            saw_key_a_press,
            saw_pointer_motion
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    return (saw_key_a_press && saw_pointer_motion) ? 0 : -1;
}

int main(void) {
    const char *fb_path = getenv("PEETOS_FB");
    const char *kbd_path = getenv("PEETOS_INPUT_KEYBOARD");
    const char *ptr_path = getenv("PEETOS_INPUT_POINTER");
    struct FramebufferDeviceInfo info;
    uint8_t *fb = MAP_FAILED;
    int fb_fd = -1;
    int kbd_fd = -1;
    int ptr_fd = -1;
    int epfd = -1;

    if (require_env_equals("PEETOS_DISPLAY_MODE", "compositor-first") < 0) {
        return 1;
    }
    if (require_env_equals("PEETOS_FB", "/dev/fb0") < 0) {
        return 2;
    }
    if (require_env_equals("PEETOS_INPUT_KEYBOARD", "/dev/input/event0") < 0) {
        return 3;
    }
    if (require_env_equals("PEETOS_INPUT_POINTER", "/dev/input/event1") < 0) {
        return 4;
    }

    if (fb_path == NULL || kbd_path == NULL || ptr_path == NULL) {
        log_line("[gui-smoke] required device environment missing\n");
        return 5;
    }

    memset(&info, 0, sizeof(info));
    fb_fd = open(fb_path, O_RDWR);
    if (fb_fd < 0) {
        log_errno("open framebuffer failed");
        return 6;
    }
    if (ioctl(fb_fd, FBIOGETINFO, &info) < 0) {
        log_errno("FBIOGETINFO failed");
        return 7;
    }
    if (info.width == 0 || info.height == 0 || info.pitch < info.width * 4u ||
        info.bpp != 32u || info.format != FB_FORMAT_XRGB8888 || info.map_size == 0) {
        log_line("[gui-smoke] framebuffer metadata validation failed\n");
        return 8;
    }

    fb = mmap(NULL, (size_t)info.map_size, PROT_READ | PROT_WRITE, MAP_SHARED, fb_fd, 0);
    if (fb == MAP_FAILED) {
        log_errno("framebuffer mmap failed");
        return 9;
    }
    draw_pattern(fb, &info);
    if (validate_pattern(fb, &info) < 0) {
        log_line("[gui-smoke] framebuffer pattern validation failed\n");
        return 10;
    }

    kbd_fd = open(kbd_path, O_RDONLY | O_NONBLOCK);
    if (kbd_fd < 0) {
        log_errno("open keyboard event device failed");
        return 11;
    }
    ptr_fd = open(ptr_path, O_RDONLY | O_NONBLOCK);
    if (ptr_fd < 0) {
        log_errno("open pointer event device failed");
        return 12;
    }
    if (drain_input_once(kbd_fd, "keyboard") < 0) {
        return 13;
    }
    if (drain_input_once(ptr_fd, "pointer") < 0) {
        return 14;
    }

    epfd = epoll_create1(0);
    if (epfd < 0) {
        log_errno("epoll_create1 failed");
        return 15;
    }

    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = EPOLLIN;
    ev.data.u32 = 1;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, kbd_fd, &ev) < 0) {
        log_errno("epoll_ctl add keyboard failed");
        return 16;
    }
    ev.data.u32 = 2;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, ptr_fd, &ev) < 0) {
        log_errno("epoll_ctl add pointer failed");
        return 17;
    }

    struct epoll_event ready[2];
    int nready = epoll_wait(epfd, ready, 2, 25);
    if (nready < 0) {
        log_errno("epoll_wait failed");
        return 18;
    }
    {
        char buf[224];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[gui-smoke] framebuffer=%ux%u pitch=%u map=%llu epoll_ready=%d\n",
            info.width,
            info.height,
            info.pitch,
            (unsigned long long)info.map_size,
            nready
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }

    if (wait_for_input_events(epfd, kbd_fd, ptr_fd) < 0) {
        log_line("[gui-smoke] input delivery validation failed\n");
        return 19;
    }

    log_line("[gui-smoke] pass\n");

    if (epfd >= 0) {
        close(epfd);
    }
    if (ptr_fd >= 0) {
        close(ptr_fd);
    }
    if (kbd_fd >= 0) {
        close(kbd_fd);
    }
    if (fb != MAP_FAILED) {
        munmap(fb, (size_t)info.map_size);
    }
    if (fb_fd >= 0) {
        close(fb_fd);
    }
    return 0;
}
