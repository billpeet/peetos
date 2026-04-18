#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <signal.h>
#include <unistd.h>

#define FBIOGETINFO 0x5001UL
#define FB_FORMAT_XRGB8888 1u
#define EV_KEY 0x01u
#define EV_REL 0x02u
#define KEY_ESC 1u
#define KEY_0 11u
#define KEY_1 2u
#define KEY_2 3u
#define KEY_3 4u
#define KEY_4 5u
#define KEY_5 6u
#define KEY_6 7u
#define KEY_7 8u
#define KEY_8 9u
#define KEY_9 10u
#define KEY_MINUS 12u
#define KEY_BACKSPACE 14u
#define KEY_TAB 15u
#define KEY_ENTER 28u
#define KEY_A 30u
#define KEY_B 48u
#define KEY_C 46u
#define KEY_D 32u
#define KEY_E 18u
#define KEY_F 33u
#define KEY_G 34u
#define KEY_H 35u
#define KEY_I 23u
#define KEY_J 36u
#define KEY_K 37u
#define KEY_L 38u
#define KEY_M 50u
#define KEY_N 49u
#define KEY_O 24u
#define KEY_P 25u
#define KEY_Q 16u
#define KEY_R 19u
#define KEY_S 31u
#define KEY_T 20u
#define KEY_U 22u
#define KEY_V 47u
#define KEY_W 17u
#define KEY_X 45u
#define KEY_Y 21u
#define KEY_Z 44u
#define KEY_DOT 52u
#define KEY_SLASH 53u
#define KEY_SPACE 57u
#define BTN_LEFT 0x110u
#define REL_X 0u
#define REL_Y 1u
#define GLYPH_W 5u
#define GLYPH_H 7u
#define GLYPH_SCALE 2u
#define TEXT_ADVANCE ((GLYPH_W + 1u) * GLYPH_SCALE)
#define TERM_LINES 10
#define TERM_COLS 48
#define FILE_ITEMS 5
#define PANEL_TITLE_H 28
#define PANEL_SHADOW 6
#define FILES_MSG_HELLO 1u
#define FILES_MSG_STATE 2u
#define FILES_MSG_SET_SELECTION 3u
#define FILES_MSG_ADVANCE 4u
#define FILES_MSG_SET_FOCUS 5u
#define FILES_MSG_QUIT 6u

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

struct Rect {
    int x;
    int y;
    int w;
    int h;
};

struct Canvas {
    uint32_t *pixels;
    int width;
    int height;
    int stride_pixels;
};

struct FilesMsg {
    uint32_t type;
    uint32_t arg0;
    uint32_t arg1;
    uint32_t arg2;
};

enum PanelId {
    PANEL_TERMINAL = 0,
    PANEL_FILES = 1,
    PANEL_INSPECTOR = 2,
    PANEL_COUNT = 3,
};

struct Surface {
    enum PanelId id;
    struct Rect frame;
    struct Canvas canvas;
    int attached;
    int dirty;
};

struct ShellState {
    struct Rect dock;
    struct Rect topbar;
    struct Rect theme_button;
    char terminal_lines[TERM_LINES][TERM_COLS];
    int terminal_count;
    char input_buffer[TERM_COLS];
    int input_len;
    int selected_file;
    int focused_panel;
    int pointer_x;
    int pointer_y;
    int key_presses;
    int click_count;
    int theme_index;
    int should_exit;
    int needs_present;
    char last_command[TERM_COLS];
    char status_line[TERM_COLS];
    struct Surface surfaces[PANEL_COUNT];
    int z_order[PANEL_COUNT];
    int files_sock;
    pid_t files_child_pid;
    int files_ready;
    int terminal_master_fd;
    int terminal_stdout_fd;
    pid_t terminal_child_pid;
    int terminal_ready;
    int terminal_observed_pwd;
    int terminal_live_len;
    char terminal_live[TERM_COLS];
};

static const char *const PANEL_NAMES[PANEL_COUNT] = {"TERMINAL", "FILES", "INSPECTOR"};
static const char *const FILE_LIST[FILE_ITEMS] = {
    "SYSTEM.LOG",
    "NETWORK.CFG",
    "TERMINAL.HIST",
    "APPS/",
    "README.TXT",
};
static const char *const THEME_NAMES[] = {"MIDNIGHT", "AMBER", "MINT"};

struct Theme {
    uint32_t bg0;
    uint32_t bg1;
    uint32_t shell_chrome;
    uint32_t shell_text;
    uint32_t panel_bg;
    uint32_t panel_border;
    uint32_t panel_focus;
    uint32_t accent;
    uint32_t accent_2;
    uint32_t muted;
    uint32_t cursor;
};

static const struct Theme THEMES[] = {
    {0x000d1826u, 0x00132a46u, 0x00182d44u, 0x00f1f6fcu, 0x000b121bu, 0x00324e6au, 0x00f4a340u, 0x0052a7ffu, 0x00ffb347u, 0x0092a3b7u, 0x00f4f7fbu},
    {0x001a140du, 0x00302317u, 0x0044361du, 0x00fff0d4u, 0x00100b06u, 0x00584a2eu, 0x00e9b04cu, 0x00f68026u, 0x00ffd166u, 0x00baa27du, 0x00fff4e4u},
    {0x000d1a14u, 0x00153427u, 0x00194a37u, 0x00ebfff7u, 0x0009100cu, 0x002a6649u, 0x0079e2a0u, 0x0039c48eu, 0x0096f2beu, 0x0088bca7u, 0x00f6fffb},
};

static void append_terminal_line(struct ShellState *state, const char *line);
static void set_status(struct ShellState *state, const char *line);

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
    int n = snprintf(buf, sizeof(buf), "[peet-shell] %s errno=%d\n", tag, errno);
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
            "[peet-shell] env mismatch %s expected=%s actual=%s\n",
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

static void log_surface_attach(enum PanelId panel, struct Rect frame) {
    char buf[128];
    int n = snprintf(
        buf,
        sizeof(buf),
        "[peet-shell] surface-attach=%s x=%d y=%d w=%d h=%d\n",
        PANEL_NAMES[panel],
        frame.x,
        frame.y,
        frame.w,
        frame.h
    );
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static void log_surface_raise(enum PanelId panel, const char *source) {
    char buf[96];
    int n = snprintf(buf, sizeof(buf), "[peet-shell] surface-raised=%s source=%s\n", PANEL_NAMES[panel], source);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static void log_terminal_shell_spawn(pid_t pid) {
    char buf[96];
    int n = snprintf(
        buf,
        sizeof(buf),
        "[peet-shell] terminal-shell-spawned pid=%d backend=socketpair\n",
        (int)pid
    );
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static ssize_t read_full(int fd, void *buf, size_t len) {
    size_t offset = 0;
    while (offset < len) {
        ssize_t n = read(fd, (char *)buf + offset, len - offset);
        if (n <= 0) {
            return n;
        }
        offset += (size_t)n;
    }
    return (ssize_t)offset;
}

static ssize_t write_full(int fd, const void *buf, size_t len) {
    size_t offset = 0;
    while (offset < len) {
        ssize_t n = write(fd, (const char *)buf + offset, len - offset);
        if (n <= 0) {
            return n;
        }
        offset += (size_t)n;
    }
    return (ssize_t)offset;
}

static int files_send_msg(int fd, uint32_t type, uint32_t arg0, uint32_t arg1, uint32_t arg2) {
    struct FilesMsg msg;
    msg.type = type;
    msg.arg0 = arg0;
    msg.arg1 = arg1;
    msg.arg2 = arg2;
    return write_full(fd, &msg, sizeof(msg)) == (ssize_t)sizeof(msg) ? 0 : -1;
}

static int files_send_state(int fd, uint32_t selected, uint32_t focused, uint32_t source_kind) {
    return files_send_msg(fd, FILES_MSG_STATE, selected, focused, source_kind);
}

static int files_client_main(int sock) {
    int selected = 0;
    int focused = 0;
    struct FilesMsg msg;

    log_line("[peet-shell-files] ready\n");
    if (files_send_msg(sock, FILES_MSG_HELLO, 0, 0, 0) < 0 ||
        files_send_state(sock, (uint32_t)selected, (uint32_t)focused, 0u) < 0) {
        return 1;
    }

    while (1) {
        ssize_t n = read_full(sock, &msg, sizeof(msg));
        if (n <= 0) {
            return 0;
        }
        if (msg.type == FILES_MSG_QUIT) {
            return 0;
        }
        if (msg.type == FILES_MSG_SET_SELECTION) {
            if ((int)msg.arg0 >= 0 && (int)msg.arg0 < FILE_ITEMS) {
                selected = (int)msg.arg0;
            }
        } else if (msg.type == FILES_MSG_ADVANCE) {
            selected = (selected + 1) % FILE_ITEMS;
        } else if (msg.type == FILES_MSG_SET_FOCUS) {
            focused = msg.arg0 ? 1 : 0;
        }
        {
            char buf[128];
            int out = snprintf(
                buf,
                sizeof(buf),
                "[peet-shell-files] selected=%s focused=%d source=%u\n",
                FILE_LIST[selected],
                focused,
                msg.type
            );
            if (out > 0) {
                write_all(1, buf, (size_t)out);
            }
        }
        if (files_send_state(sock, (uint32_t)selected, (uint32_t)focused, msg.type) < 0) {
            return 1;
        }
    }
}

static int files_handle_parent_msg(struct ShellState *state, const struct FilesMsg *msg) {
    if (msg->type == FILES_MSG_HELLO) {
        state->files_ready = 1;
        return 0;
    }
    if (msg->type != FILES_MSG_STATE) {
        return -1;
    }
    if ((int)msg->arg0 < 0 || (int)msg->arg0 >= FILE_ITEMS) {
        return -1;
    }
    state->selected_file = (int)msg->arg0;
    set_status(state, FILE_LIST[state->selected_file]);
    state->surfaces[PANEL_FILES].dirty = 1;
    state->surfaces[PANEL_INSPECTOR].dirty = 1;
    state->needs_present = 1;
    return 0;
}

static int files_recv_parent_msg(int fd, struct FilesMsg *msg) {
    ssize_t n = read_full(fd, msg, sizeof(*msg));
    return n == (ssize_t)sizeof(*msg) ? 0 : -1;
}

static int files_sync_focus(struct ShellState *state) {
    if (state->files_sock < 0) {
        return -1;
    }
    return files_send_msg(
        state->files_sock,
        FILES_MSG_SET_FOCUS,
        state->focused_panel == PANEL_FILES ? 1u : 0u,
        0u,
        0u
    );
}

static void set_terminal_live(struct ShellState *state, const char *text, int len) {
    if (len < 0) {
        len = 0;
    }
    if (len >= TERM_COLS) {
        len = TERM_COLS - 1;
    }
    if (len > 0) {
        memcpy(state->terminal_live, text, (size_t)len);
    }
    state->terminal_live[len] = '\0';
    state->terminal_live_len = len;
    state->surfaces[PANEL_TERMINAL].dirty = 1;
    state->needs_present = 1;
}

static void clear_terminal_live(struct ShellState *state) {
    set_terminal_live(state, "", 0);
}

static void terminal_push_visible_char(struct ShellState *state, char ch) {
    char next[TERM_COLS];
    int len = state->terminal_live_len;

    if (len >= TERM_COLS - 1) {
        append_terminal_line(state, state->terminal_live);
        len = 0;
    }
    if (len > 0) {
        memcpy(next, state->terminal_live, (size_t)len);
    }
    next[len++] = ch;
    next[len] = '\0';
    set_terminal_live(state, next, len);
}

static void terminal_backspace_live(struct ShellState *state) {
    if (state->terminal_live_len <= 0) {
        return;
    }
    state->terminal_live_len--;
    state->terminal_live[state->terminal_live_len] = '\0';
    state->surfaces[PANEL_TERMINAL].dirty = 1;
    state->needs_present = 1;
}

static void terminal_commit_live(struct ShellState *state) {
    append_terminal_line(state, state->terminal_live);
    clear_terminal_live(state);
}

static void terminal_consume_output(struct ShellState *state, const uint8_t *buf, size_t len) {
    for (size_t i = 0; i < len; i++) {
        uint8_t ch = buf[i];
        if (ch == '\r') {
            continue;
        }
        if (ch == '\n') {
            terminal_commit_live(state);
            continue;
        }
        if (ch == '\b' || ch == 0x7fu) {
            terminal_backspace_live(state);
            continue;
        }
        if (ch == '\t') {
            for (int pad = 0; pad < 4; pad++) {
                terminal_push_visible_char(state, ' ');
            }
            continue;
        }
        if (ch >= 32u && ch <= 126u) {
            terminal_push_visible_char(state, (char)ch);
        }
    }

    if (!state->terminal_observed_pwd &&
        (strstr(state->terminal_live, "/mnt/ext2") != NULL ||
         (state->terminal_count > 0 &&
          strstr(state->terminal_lines[state->terminal_count - 1], "/mnt/ext2") != NULL))) {
        state->terminal_observed_pwd = 1;
        log_line("[peet-shell] terminal-observed-pwd=/mnt/ext2\n");
    }
}

static int terminal_send_bytes(struct ShellState *state, const void *buf, size_t len) {
    if (state->terminal_master_fd < 0) {
        return -1;
    }
    return write_full(state->terminal_master_fd, buf, len) == (ssize_t)len ? 0 : -1;
}

static int spawn_terminal_shell(struct ShellState *state) {
    int shell_sv[2] = {-1, -1};
    pid_t pid;

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, shell_sv) < 0) {
        return -1;
    }

    pid = fork();
    if (pid < 0) {
        close(shell_sv[0]);
        close(shell_sv[1]);
        return -1;
    }
    if (pid == 0) {
        char *const argv[] = {
            (char *)"sh",
            (char *)"-i",
            0,
        };
        char *const envp[] = {
            (char *)"PATH=/mnt/ext2/bin:/bin",
            (char *)"TERM=peetos",
            (char *)"HOME=/root",
            (char *)"SHELL=/bin/sh",
            (char *)"PS1=/mnt/ext2 $ ",
            0,
        };

        close(shell_sv[0]);
        write_all(2, "[peet-shell-child] starting toybox sh\n", 38);
        if (dup2(shell_sv[1], 0) < 0 || dup2(shell_sv[1], 1) < 0 || dup2(shell_sv[1], 2) < 0) {
            _exit(121);
        }
        if (shell_sv[1] > 2) {
            close(shell_sv[1]);
        }
        chdir("/mnt/ext2");
        execve("/mnt/ext2/bin/toybox", argv, envp);
        write_all(2, "[peet-shell-child] execve failed\n", 33);
        _exit(122);
    }

    close(shell_sv[1]);
    state->terminal_master_fd = shell_sv[0];
    state->terminal_stdout_fd = shell_sv[0];
    state->terminal_child_pid = pid;
    state->terminal_ready = 1;
    log_terminal_shell_spawn(pid);
    return 0;
}

static const uint8_t *glyph_for(char c) {
    static const uint8_t blank[GLYPH_H] = {0, 0, 0, 0, 0, 0, 0};
    static const uint8_t space[GLYPH_H] = {0, 0, 0, 0, 0, 0, 0};
    static const uint8_t dash[GLYPH_H] = {0, 0, 0, 0x0e, 0, 0, 0};
    static const uint8_t dot[GLYPH_H] = {0, 0, 0, 0, 0, 0x0c, 0x0c};
    static const uint8_t slash[GLYPH_H] = {0x01, 0x02, 0x04, 0x08, 0x10, 0, 0};
    static const uint8_t colon[GLYPH_H] = {0, 0x0c, 0x0c, 0, 0x0c, 0x0c, 0};
    static const uint8_t lbracket[GLYPH_H] = {0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e};
    static const uint8_t rbracket[GLYPH_H] = {0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e};
    static const uint8_t digits[10][GLYPH_H] = {
        {0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e},
        {0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e},
        {0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f},
        {0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e},
        {0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02},
        {0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e},
        {0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e},
        {0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08},
        {0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e},
        {0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e},
    };
    static const uint8_t letters[26][GLYPH_H] = {
        {0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11},
        {0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e},
        {0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e},
        {0x1c, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1c},
        {0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f},
        {0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10},
        {0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f},
        {0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11},
        {0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e},
        {0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e},
        {0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11},
        {0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f},
        {0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11},
        {0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11},
        {0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e},
        {0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10},
        {0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d},
        {0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11},
        {0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e},
        {0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04},
        {0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e},
        {0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04},
        {0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a},
        {0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11},
        {0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04},
        {0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f},
    };

    if (c >= 'a' && c <= 'z') {
        c = (char)(c - 'a' + 'A');
    }
    if (c >= 'A' && c <= 'Z') {
        return letters[c - 'A'];
    }
    if (c >= '0' && c <= '9') {
        return digits[c - '0'];
    }
    switch (c) {
        case ' ': return space;
        case '-': return dash;
        case '.': return dot;
        case '/': return slash;
        case ':': return colon;
        case '[': return lbracket;
        case ']': return rbracket;
        default: return blank;
    }
}

static uint32_t *canvas_pixel(struct Canvas *canvas, int x, int y) {
    return canvas->pixels + (size_t)y * (size_t)canvas->stride_pixels + (size_t)x;
}

static void canvas_rect(struct Canvas *canvas, int x, int y, int w, int h, uint32_t color) {
    if (w <= 0 || h <= 0) {
        return;
    }
    if (x < 0) {
        w += x;
        x = 0;
    }
    if (y < 0) {
        h += y;
        y = 0;
    }
    if (x + w > canvas->width) {
        w = canvas->width - x;
    }
    if (y + h > canvas->height) {
        h = canvas->height - y;
    }
    if (w <= 0 || h <= 0) {
        return;
    }
    for (int row = 0; row < h; row++) {
        uint32_t *dst = canvas_pixel(canvas, x, y + row);
        for (int col = 0; col < w; col++) {
            dst[col] = color;
        }
    }
}

static void canvas_frame(struct Canvas *canvas, struct Rect rect, int thickness, uint32_t color) {
    canvas_rect(canvas, rect.x, rect.y, rect.w, thickness, color);
    canvas_rect(canvas, rect.x, rect.y + rect.h - thickness, rect.w, thickness, color);
    canvas_rect(canvas, rect.x, rect.y, thickness, rect.h, color);
    canvas_rect(canvas, rect.x + rect.w - thickness, rect.y, thickness, rect.h, color);
}

static void canvas_gradient(struct Canvas *canvas, uint32_t top, uint32_t bottom) {
    uint8_t tr = (uint8_t)((top >> 16) & 0xffu);
    uint8_t tg = (uint8_t)((top >> 8) & 0xffu);
    uint8_t tb = (uint8_t)(top & 0xffu);
    uint8_t br = (uint8_t)((bottom >> 16) & 0xffu);
    uint8_t bg = (uint8_t)((bottom >> 8) & 0xffu);
    uint8_t bb = (uint8_t)(bottom & 0xffu);
    for (int y = 0; y < canvas->height; y++) {
        uint32_t mix = canvas->height > 1 ? (uint32_t)(y * 255 / (canvas->height - 1)) : 0u;
        uint32_t r = ((uint32_t)tr * (255u - mix) + (uint32_t)br * mix) / 255u;
        uint32_t g = ((uint32_t)tg * (255u - mix) + (uint32_t)bg * mix) / 255u;
        uint32_t b = ((uint32_t)tb * (255u - mix) + (uint32_t)bb * mix) / 255u;
        canvas_rect(canvas, 0, y, canvas->width, 1, (r << 16) | (g << 8) | b);
    }
}

static void canvas_text(struct Canvas *canvas, int x, int y, const char *text, uint32_t fg, uint32_t bg) {
    size_t len = strlen(text);
    for (size_t idx = 0; idx < len; idx++) {
        const uint8_t *glyph = glyph_for(text[idx]);
        for (uint32_t gy = 0; gy < GLYPH_H; gy++) {
            for (uint32_t gx = 0; gx < GLYPH_W; gx++) {
                uint32_t color = (glyph[gy] & (1u << (GLYPH_W - 1u - gx))) != 0 ? fg : bg;
                canvas_rect(
                    canvas,
                    x + (int)(idx * TEXT_ADVANCE) + (int)(gx * GLYPH_SCALE),
                    y + (int)(gy * GLYPH_SCALE),
                    (int)GLYPH_SCALE,
                    (int)GLYPH_SCALE,
                    color
                );
            }
        }
    }
}

static void canvas_blit(struct Canvas *dst, struct Canvas *src, int dst_x, int dst_y) {
    for (int y = 0; y < src->height; y++) {
        if (dst_y + y < 0 || dst_y + y >= dst->height) {
            continue;
        }
        for (int x = 0; x < src->width; x++) {
            if (dst_x + x < 0 || dst_x + x >= dst->width) {
                continue;
            }
            *canvas_pixel(dst, dst_x + x, dst_y + y) = *canvas_pixel(src, x, y);
        }
    }
}

static void draw_cursor(struct Canvas *canvas, int x, int y, uint32_t color) {
    canvas_rect(canvas, x, y, 12, 2, color);
    canvas_rect(canvas, x, y, 2, 16, color);
    canvas_rect(canvas, x + 3, y + 4, 2, 8, color);
}

static void append_terminal_line(struct ShellState *state, const char *line) {
    if (state->terminal_count < TERM_LINES) {
        strncpy(state->terminal_lines[state->terminal_count], line, TERM_COLS - 1);
        state->terminal_lines[state->terminal_count][TERM_COLS - 1] = '\0';
        state->terminal_count++;
        state->surfaces[PANEL_TERMINAL].dirty = 1;
        state->needs_present = 1;
        return;
    }
    for (int i = 1; i < TERM_LINES; i++) {
        memcpy(state->terminal_lines[i - 1], state->terminal_lines[i], TERM_COLS);
    }
    strncpy(state->terminal_lines[TERM_LINES - 1], line, TERM_COLS - 1);
    state->terminal_lines[TERM_LINES - 1][TERM_COLS - 1] = '\0';
    state->surfaces[PANEL_TERMINAL].dirty = 1;
    state->needs_present = 1;
}

static void set_status(struct ShellState *state, const char *line) {
    strncpy(state->status_line, line, TERM_COLS - 1);
    state->status_line[TERM_COLS - 1] = '\0';
    state->surfaces[PANEL_INSPECTOR].dirty = 1;
    state->needs_present = 1;
}

static void raise_surface_to_front(struct ShellState *state, int panel) {
    int next[PANEL_COUNT];
    int out = 0;
    for (int i = 0; i < PANEL_COUNT; i++) {
        if (state->z_order[i] != panel) {
            next[out++] = state->z_order[i];
        }
    }
    next[out] = panel;
    for (int i = 0; i < PANEL_COUNT; i++) {
        state->z_order[i] = next[i];
    }
}

static void rotate_theme(struct ShellState *state, const char *source) {
    char buf[96];
    state->theme_index = (state->theme_index + 1) % (int)(sizeof(THEMES) / sizeof(THEMES[0]));
    snprintf(buf, sizeof(buf), "[peet-shell] theme=%s source=%s\n", THEME_NAMES[state->theme_index], source);
    log_line(buf);
    set_status(state, "THEME ROTATED");
    for (int i = 0; i < PANEL_COUNT; i++) {
        state->surfaces[i].dirty = 1;
    }
    state->needs_present = 1;
}

static void set_focus(struct ShellState *state, int panel, const char *source) {
    char buf[96];
    if (panel < 0 || panel >= PANEL_COUNT) {
        return;
    }
    state->focused_panel = panel;
    raise_surface_to_front(state, panel);
    snprintf(buf, sizeof(buf), "[peet-shell] focus=%s source=%s\n", PANEL_NAMES[panel], source);
    log_line(buf);
    log_surface_raise((enum PanelId)panel, source);
    for (int i = 0; i < PANEL_COUNT; i++) {
        state->surfaces[i].dirty = 1;
    }
    state->needs_present = 1;
    if (state->files_sock >= 0) {
        (void)files_sync_focus(state);
    }
}

static int point_in_rect(int x, int y, struct Rect rect) {
    return x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
}

static int keycode_to_ascii(uint16_t code) {
    switch (code) {
        case KEY_A: return 'a';
        case KEY_B: return 'b';
        case KEY_C: return 'c';
        case KEY_D: return 'd';
        case KEY_E: return 'e';
        case KEY_F: return 'f';
        case KEY_G: return 'g';
        case KEY_H: return 'h';
        case KEY_I: return 'i';
        case KEY_J: return 'j';
        case KEY_K: return 'k';
        case KEY_L: return 'l';
        case KEY_M: return 'm';
        case KEY_N: return 'n';
        case KEY_O: return 'o';
        case KEY_P: return 'p';
        case KEY_Q: return 'q';
        case KEY_R: return 'r';
        case KEY_S: return 's';
        case KEY_T: return 't';
        case KEY_U: return 'u';
        case KEY_V: return 'v';
        case KEY_W: return 'w';
        case KEY_X: return 'x';
        case KEY_Y: return 'y';
        case KEY_Z: return 'z';
        case KEY_0: return '0';
        case KEY_1: return '1';
        case KEY_2: return '2';
        case KEY_3: return '3';
        case KEY_4: return '4';
        case KEY_5: return '5';
        case KEY_6: return '6';
        case KEY_7: return '7';
        case KEY_8: return '8';
        case KEY_9: return '9';
        case KEY_MINUS: return '-';
        case KEY_DOT: return '.';
        case KEY_SLASH: return '/';
        case KEY_SPACE: return ' ';
        default: return 0;
    }
}

static void bootstrap_terminal(struct ShellState *state) {
    append_terminal_line(state, "CONNECTING TO TOYBOX SH");
    append_terminal_line(state, "TAB CHANGES FOCUS");
    append_terminal_line(state, "TYPE DIRECTLY INTO PTY");
    set_status(state, "LAUNCHING TOYBOX SHELL");
}

static void recalc_layout(struct ShellState *state, const struct FramebufferDeviceInfo *info) {
    int width = (int)info->width;
    int height = (int)info->height;

    state->topbar.x = 24;
    state->topbar.y = 18;
    state->topbar.w = width - 48;
    state->topbar.h = 46;
    state->theme_button.x = state->topbar.x + state->topbar.w - 170;
    state->theme_button.y = state->topbar.y + 8;
    state->theme_button.w = 150;
    state->theme_button.h = 30;

    state->dock.x = 24;
    state->dock.y = 92;
    state->dock.w = 82;
    state->dock.h = height - 116;

    state->surfaces[PANEL_TERMINAL].frame = (struct Rect){150, 122, width - 420, height - 170};
    state->surfaces[PANEL_FILES].frame = (struct Rect){width - 330, 112, 250, 238};
    state->surfaces[PANEL_INSPECTOR].frame = (struct Rect){width - 300, 264, 260, 240};

    for (int i = 0; i < PANEL_COUNT; i++) {
        state->surfaces[i].dirty = 1;
    }
    state->needs_present = 1;
}

static int allocate_surface(struct Surface *surface) {
    size_t pixel_count = (size_t)surface->frame.w * (size_t)surface->frame.h;
    surface->canvas.width = surface->frame.w;
    surface->canvas.height = surface->frame.h;
    surface->canvas.stride_pixels = surface->frame.w;
    surface->canvas.pixels = malloc(pixel_count * sizeof(uint32_t));
    if (surface->canvas.pixels == NULL) {
        return -1;
    }
    surface->attached = 1;
    surface->dirty = 1;
    log_surface_attach(surface->id, surface->frame);
    return 0;
}

static void render_panel_chrome(struct Canvas *canvas, const char *title, int focused, const struct Theme *theme) {
    canvas_rect(canvas, 0, PANEL_SHADOW, canvas->width - PANEL_SHADOW, canvas->height - PANEL_SHADOW, theme->panel_bg);
    canvas_frame(canvas, (struct Rect){0, PANEL_SHADOW, canvas->width - PANEL_SHADOW, canvas->height - PANEL_SHADOW}, 2, focused ? theme->panel_focus : theme->panel_border);
    canvas_rect(canvas, 0, PANEL_SHADOW, canvas->width - PANEL_SHADOW, PANEL_TITLE_H, focused ? theme->shell_chrome : theme->panel_border);
    canvas_text(canvas, 12, PANEL_SHADOW + 8, title, theme->shell_text, focused ? theme->shell_chrome : theme->panel_border);
}

static void render_terminal_surface(struct ShellState *state, struct Surface *surface, const struct Theme *theme) {
    const char *footer;
    struct Canvas *canvas = &surface->canvas;
    memset(canvas->pixels, 0, (size_t)canvas->width * (size_t)canvas->height * sizeof(uint32_t));
    render_panel_chrome(canvas, "TERMINAL", state->focused_panel == PANEL_TERMINAL, theme);
    for (int i = 0; i < state->terminal_count; i++) {
        canvas_text(canvas, 14, PANEL_SHADOW + 42 + i * 18, state->terminal_lines[i], theme->shell_text, theme->panel_bg);
    }
    footer = state->input_len > 0 ? state->input_buffer :
        (state->terminal_live_len > 0 ? state->terminal_live : (state->terminal_ready ? "TOYBOX SH" : "SHELL OFFLINE"));
    canvas_text(
        canvas,
        14,
        canvas->height - 32,
        footer,
        state->focused_panel == PANEL_TERMINAL ? theme->accent_2 : theme->muted,
        theme->panel_bg
    );
    if (state->focused_panel == PANEL_TERMINAL) {
        int cursor_x = 14 + (int)strlen(footer) * (int)TEXT_ADVANCE;
        canvas_rect(canvas, cursor_x, canvas->height - 16, (int)GLYPH_SCALE * 3, 3, theme->accent_2);
    }
    surface->dirty = 0;
}

static void render_files_surface(struct ShellState *state, struct Surface *surface, const struct Theme *theme) {
    struct Canvas *canvas = &surface->canvas;
    memset(canvas->pixels, 0, (size_t)canvas->width * (size_t)canvas->height * sizeof(uint32_t));
    render_panel_chrome(canvas, "FILES", state->focused_panel == PANEL_FILES, theme);
    for (int i = 0; i < FILE_ITEMS; i++) {
        uint32_t fill = state->selected_file == i ? theme->accent : theme->panel_bg;
        canvas_rect(canvas, 12, PANEL_SHADOW + 40 + i * 28, canvas->width - 30, 22, fill);
        canvas_text(canvas, 20, PANEL_SHADOW + 46 + i * 28, FILE_LIST[i], state->selected_file == i ? 0x00101820u : theme->shell_text, fill);
    }
    surface->dirty = 0;
}

static void render_inspector_surface(struct ShellState *state, struct Surface *surface, const struct Theme *theme) {
    char buf[64];
    struct Canvas *canvas = &surface->canvas;
    memset(canvas->pixels, 0, (size_t)canvas->width * (size_t)canvas->height * sizeof(uint32_t));
    render_panel_chrome(canvas, "INSPECTOR", state->focused_panel == PANEL_INSPECTOR, theme);
    snprintf(buf, sizeof(buf), "THEME %s", THEME_NAMES[state->theme_index]);
    canvas_text(canvas, 12, PANEL_SHADOW + 42, buf, theme->accent_2, theme->panel_bg);
    snprintf(buf, sizeof(buf), "POINTER %03d %03d", state->pointer_x, state->pointer_y);
    canvas_text(canvas, 12, PANEL_SHADOW + 68, buf, theme->shell_text, theme->panel_bg);
    snprintf(buf, sizeof(buf), "KEYS %d", state->key_presses);
    canvas_text(canvas, 12, PANEL_SHADOW + 94, buf, theme->shell_text, theme->panel_bg);
    snprintf(buf, sizeof(buf), "CLICKS %d", state->click_count);
    canvas_text(canvas, 12, PANEL_SHADOW + 120, buf, theme->shell_text, theme->panel_bg);
    snprintf(buf, sizeof(buf), "LAST %.22s", state->last_command[0] ? state->last_command : "NONE");
    canvas_text(canvas, 12, PANEL_SHADOW + 146, buf, theme->shell_text, theme->panel_bg);
    canvas_text(canvas, 12, canvas->height - 28, state->status_line, theme->muted, theme->panel_bg);
    surface->dirty = 0;
}

static void render_surface(struct ShellState *state, struct Surface *surface, const struct Theme *theme) {
    if (surface->id == PANEL_TERMINAL) {
        render_terminal_surface(state, surface, theme);
    } else if (surface->id == PANEL_FILES) {
        render_files_surface(state, surface, theme);
    } else {
        render_inspector_surface(state, surface, theme);
    }
}

static void draw_desktop(struct ShellState *state, struct Canvas *canvas, const struct Theme *theme) {
    char buf[64];
    canvas_gradient(canvas, theme->bg0, theme->bg1);
    for (int y = 0; y < canvas->height; y += 48) {
        canvas_rect(canvas, 0, y, canvas->width, 1, theme->shell_chrome & 0x00fefefeu);
    }

    canvas_rect(canvas, state->topbar.x, state->topbar.y, state->topbar.w, state->topbar.h, theme->shell_chrome);
    canvas_frame(canvas, state->topbar, 2, theme->panel_border);
    canvas_text(canvas, state->topbar.x + 16, state->topbar.y + 12, "PEETOS SHELL", theme->shell_text, theme->shell_chrome);
    snprintf(buf, sizeof(buf), "FOCUS %s", PANEL_NAMES[state->focused_panel]);
    canvas_text(canvas, state->topbar.x + 220, state->topbar.y + 12, buf, theme->accent_2, theme->shell_chrome);
    canvas_rect(canvas, state->theme_button.x, state->theme_button.y, state->theme_button.w, state->theme_button.h, theme->accent);
    canvas_text(canvas, state->theme_button.x + 12, state->theme_button.y + 9, "ROTATE THEME", 0x00101618u, theme->accent);

    canvas_rect(canvas, state->dock.x, state->dock.y, state->dock.w, state->dock.h, theme->panel_bg);
    canvas_frame(canvas, state->dock, 2, theme->panel_border);
    canvas_text(canvas, state->dock.x + 16, state->dock.y + 14, "APPS", theme->shell_text, theme->panel_bg);
    for (int i = 0; i < PANEL_COUNT; i++) {
        struct Rect tile = {state->dock.x + 11, state->dock.y + 46 + i * 72, state->dock.w - 22, 58};
        uint32_t fill = state->focused_panel == i ? theme->accent : theme->shell_chrome;
        canvas_rect(canvas, tile.x, tile.y, tile.w, tile.h, fill);
        canvas_frame(canvas, tile, 2, state->focused_panel == i ? theme->accent_2 : theme->panel_border);
        canvas_text(canvas, tile.x + 14, tile.y + 22, PANEL_NAMES[i], state->focused_panel == i ? 0x00101820u : theme->shell_text, fill);
    }
}

static void compose_shell(struct ShellState *state, uint8_t *fb, const struct FramebufferDeviceInfo *info) {
    struct Canvas framebuffer = {(uint32_t *)fb, (int)info->width, (int)info->height, (int)(info->pitch / sizeof(uint32_t))};
    const struct Theme *theme = &THEMES[state->theme_index];

    draw_desktop(state, &framebuffer, theme);
    for (int i = 0; i < PANEL_COUNT; i++) {
        int index = state->z_order[i];
        struct Surface *surface = &state->surfaces[index];
        if (!surface->attached) {
            continue;
        }
        if (surface->dirty) {
            render_surface(state, surface, theme);
        }
        canvas_blit(&framebuffer, &surface->canvas, surface->frame.x, surface->frame.y);
    }
    draw_cursor(&framebuffer, state->pointer_x, state->pointer_y, theme->cursor);
}

static int find_top_surface_at(struct ShellState *state, int x, int y) {
    for (int i = PANEL_COUNT - 1; i >= 0; i--) {
        int panel = state->z_order[i];
        if (point_in_rect(x, y, state->surfaces[panel].frame)) {
            return panel;
        }
    }
    return -1;
}

static void handle_click(struct ShellState *state) {
    state->click_count++;
    state->surfaces[PANEL_INSPECTOR].dirty = 1;
    state->needs_present = 1;
    if (point_in_rect(state->pointer_x, state->pointer_y, state->theme_button)) {
        rotate_theme(state, "pointer");
        return;
    }
    for (int i = 0; i < PANEL_COUNT; i++) {
        struct Rect tile = {state->dock.x + 11, state->dock.y + 46 + i * 72, state->dock.w - 22, 58};
        if (point_in_rect(state->pointer_x, state->pointer_y, tile)) {
            set_focus(state, i, "launcher");
            return;
        }
    }

    {
        int panel = find_top_surface_at(state, state->pointer_x, state->pointer_y);
        if (panel >= 0) {
            set_focus(state, panel, "pointer");
            if (panel == PANEL_FILES) {
                int local_y = state->pointer_y - state->surfaces[PANEL_FILES].frame.y - (PANEL_SHADOW + 40);
                int index = local_y / 28;
                if (index >= 0 && index < FILE_ITEMS) {
                    if (state->files_sock >= 0) {
                        (void)files_send_msg(state->files_sock, FILES_MSG_SET_SELECTION, (uint32_t)index, 0u, 0u);
                    }
                }
            }
        }
    }
}

static void handle_key(struct ShellState *state, uint16_t code) {
    int ch;
    char tx[2];
    char logbuf[128];
    state->key_presses++;
    state->surfaces[PANEL_INSPECTOR].dirty = 1;
    state->needs_present = 1;
    if (code == KEY_ESC) {
        state->should_exit = 1;
        log_line("[peet-shell] exit-requested=esc\n");
        return;
    }
    if (code == KEY_TAB) {
        set_focus(state, (state->focused_panel + 1) % PANEL_COUNT, "keyboard");
        return;
    }
    if (code == KEY_T && state->focused_panel != PANEL_TERMINAL) {
        rotate_theme(state, "keyboard");
        return;
    }
    if (code == KEY_ENTER) {
        if (state->focused_panel == PANEL_TERMINAL) {
            if (state->input_len > 0) {
                char line[TERM_COLS];
                snprintf(line, sizeof(line), "$ %.44s", state->input_buffer);
                append_terminal_line(state, line);
            }
            if (state->input_len > 0) {
                strncpy(state->last_command, state->input_buffer, TERM_COLS - 1);
                state->last_command[TERM_COLS - 1] = '\0';
                snprintf(logbuf, sizeof(logbuf), "[peet-shell] terminal-command=%s\n", state->last_command);
                log_line(logbuf);
            }
            if (terminal_send_bytes(state, "\n", 1) < 0) {
                set_status(state, "PTY WRITE FAILED");
            }
            state->input_len = 0;
            state->input_buffer[0] = '\0';
            state->surfaces[PANEL_INSPECTOR].dirty = 1;
            state->needs_present = 1;
        } else if (state->focused_panel == PANEL_FILES) {
            if (state->files_sock >= 0) {
                (void)files_send_msg(state->files_sock, FILES_MSG_ADVANCE, 0u, 0u, 0u);
            }
        }
        return;
    }
    if (code == KEY_BACKSPACE && state->focused_panel == PANEL_TERMINAL) {
        char erase = 0x7f;
        if (state->input_len > 0) {
            state->input_len--;
            state->input_buffer[state->input_len] = '\0';
        }
        (void)terminal_send_bytes(state, &erase, 1);
        return;
    }
    ch = keycode_to_ascii(code);
    if (ch == 0) {
        return;
    }
    if (state->focused_panel == PANEL_TERMINAL && state->input_len < TERM_COLS - 1) {
        state->input_buffer[state->input_len++] = (char)ch;
        state->input_buffer[state->input_len] = '\0';
        tx[0] = (char)ch;
        tx[1] = '\0';
        if (terminal_send_bytes(state, tx, 1) < 0) {
            set_status(state, "PTY WRITE FAILED");
        }
    }
}

int main(void) {
    const char *fb_path = getenv("PEETOS_FB");
    const char *kbd_path = getenv("PEETOS_INPUT_KEYBOARD");
    const char *ptr_path = getenv("PEETOS_INPUT_POINTER");
    struct FramebufferDeviceInfo info;
    struct ShellState state;
    uint8_t *fb = MAP_FAILED;
    int fb_fd = -1;
    int kbd_fd = -1;
    int ptr_fd = -1;
    int epfd = -1;
    int files_sv[2] = {-1, -1};
    struct FilesMsg files_msg;
    int status = 0;

    memset(&state, 0, sizeof(state));
    state.files_sock = -1;
    state.files_child_pid = -1;
    state.terminal_master_fd = -1;
    state.terminal_stdout_fd = -1;
    state.terminal_child_pid = -1;
    state.needs_present = 1;

    signal(SIGPIPE, SIG_IGN);

    if (require_env_equals("PEETOS_DISPLAY_MODE", "compositor-first") < 0 ||
        require_env_equals("PEETOS_FB", "/dev/fb0") < 0 ||
        require_env_equals("PEETOS_INPUT_KEYBOARD", "/dev/input/event0") < 0 ||
        require_env_equals("PEETOS_INPUT_POINTER", "/dev/input/event1") < 0) {
        return 1;
    }

    if (spawn_terminal_shell(&state) < 0) {
        log_errno("terminal shell spawn failed");
        return 2;
    }

    fb_fd = open(fb_path, O_RDWR);
    if (fb_fd < 0) {
        log_errno("open framebuffer failed");
        return 3;
    }
    if (ioctl(fb_fd, FBIOGETINFO, &info) < 0) {
        log_errno("FBIOGETINFO failed");
        return 4;
    }
    if (info.format != FB_FORMAT_XRGB8888 || info.bpp != 32u) {
        log_line("[peet-shell] unexpected framebuffer format\n");
        return 5;
    }
    fb = mmap(NULL, (size_t)info.map_size, PROT_READ | PROT_WRITE, MAP_SHARED, fb_fd, 0);
    if (fb == MAP_FAILED) {
        log_errno("framebuffer mmap failed");
        return 6;
    }

    kbd_fd = open(kbd_path, O_RDONLY | O_NONBLOCK);
    ptr_fd = open(ptr_path, O_RDONLY | O_NONBLOCK);
    if (kbd_fd < 0 || ptr_fd < 0) {
        log_errno("open input device failed");
        return 7;
    }

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, files_sv) < 0) {
        log_errno("files socketpair failed");
        return 8;
    }
    state.files_child_pid = fork();
    if (state.files_child_pid < 0) {
        log_errno("files fork failed");
        return 9;
    }
    if (state.files_child_pid == 0) {
        close(files_sv[0]);
        if (state.terminal_master_fd >= 0) {
            close(state.terminal_master_fd);
        }
        if (state.terminal_stdout_fd >= 0 && state.terminal_stdout_fd != state.terminal_master_fd) {
            close(state.terminal_stdout_fd);
        }
        _exit(files_client_main(files_sv[1]));
    }
    close(files_sv[1]);
    state.files_sock = files_sv[0];

    epfd = epoll_create1(0);
    if (epfd < 0) {
        log_errno("epoll_create1 failed");
        return 10;
    }
    {
        struct epoll_event ev;
        memset(&ev, 0, sizeof(ev));
        ev.events = EPOLLIN;
        ev.data.u32 = 1;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, kbd_fd, &ev) < 0) {
            log_errno("epoll_ctl add keyboard failed");
            return 11;
        }
        ev.data.u32 = 2;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, ptr_fd, &ev) < 0) {
            log_errno("epoll_ctl add pointer failed");
            return 12;
        }
        ev.data.u32 = 3;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, state.files_sock, &ev) < 0) {
            log_errno("epoll_ctl add files socket failed");
            return 13;
        }
        ev.data.u32 = 4;
        if (epoll_ctl(epfd, EPOLL_CTL_ADD, state.terminal_stdout_fd, &ev) < 0) {
            log_errno("epoll_ctl add terminal pipe failed");
            return 14;
        }
    }

    state.theme_index = 0;
    state.focused_panel = PANEL_TERMINAL;
    state.selected_file = 0;
    state.pointer_x = (int)info.width / 2;
    state.pointer_y = (int)info.height / 2;
    state.z_order[0] = PANEL_TERMINAL;
    state.z_order[1] = PANEL_FILES;
    state.z_order[2] = PANEL_INSPECTOR;
    for (int i = 0; i < PANEL_COUNT; i++) {
        state.surfaces[i].id = (enum PanelId)i;
    }
    bootstrap_terminal(&state);
    recalc_layout(&state, &info);
    for (int i = 0; i < PANEL_COUNT; i++) {
        if (allocate_surface(&state.surfaces[i]) < 0) {
            log_errno("surface allocation failed");
            return 15;
        }
    }
    if (files_recv_parent_msg(state.files_sock, &files_msg) < 0 ||
        files_handle_parent_msg(&state, &files_msg) < 0 ||
        files_recv_parent_msg(state.files_sock, &files_msg) < 0 ||
        files_handle_parent_msg(&state, &files_msg) < 0 ||
        files_sync_focus(&state) < 0) {
        log_errno("files client bootstrap failed");
        return 16;
    }
    compose_shell(&state, fb, &info);
    state.needs_present = 0;
    log_line("[peet-shell] ready\n");

    while (!state.should_exit) {
        struct epoll_event events[4];
        int nready = epoll_wait(epfd, events, 4, 100);
        if (nready < 0) {
            log_errno("epoll_wait failed");
            return 17;
        }
        if (nready == 0) {
            if (state.needs_present) {
                compose_shell(&state, fb, &info);
                state.needs_present = 0;
            }
            continue;
        }
        for (int i = 0; i < nready; i++) {
            if (events[i].data.u32 == 1) {
                struct LinuxInputEvent input[8];
                ssize_t n = read(kbd_fd, input, sizeof(input));
                if (n < 0) {
                    if (errno == EAGAIN) {
                        continue;
                    }
                    log_errno("keyboard read failed");
                    return 18;
                }
                for (ssize_t j = 0; j < n / (ssize_t)sizeof(struct LinuxInputEvent); j++) {
                    if (input[j].type == EV_KEY && input[j].value == 1) {
                        handle_key(&state, input[j].code);
                    }
                }
            } else if (events[i].data.u32 == 2) {
                struct LinuxInputEvent input[8];
                ssize_t n = read(ptr_fd, input, sizeof(input));
                int old_pointer_x = state.pointer_x;
                int old_pointer_y = state.pointer_y;
                if (n < 0) {
                    if (errno == EAGAIN) {
                        continue;
                    }
                    log_errno("pointer read failed");
                    return 19;
                }
                for (ssize_t j = 0; j < n / (ssize_t)sizeof(struct LinuxInputEvent); j++) {
                    if (input[j].type == EV_REL && input[j].code == REL_X) {
                        state.pointer_x += input[j].value * 6;
                    } else if (input[j].type == EV_REL && input[j].code == REL_Y) {
                        state.pointer_y += input[j].value * 6;
                    } else if (input[j].type == EV_KEY && input[j].code == BTN_LEFT && input[j].value == 1) {
                        handle_click(&state);
                    }
                }
                if (state.pointer_x < 0) {
                    state.pointer_x = 0;
                }
                if (state.pointer_y < 0) {
                    state.pointer_y = 0;
                }
                if (state.pointer_x > (int)info.width - 16) {
                    state.pointer_x = (int)info.width - 16;
                }
                if (state.pointer_y > (int)info.height - 16) {
                    state.pointer_y = (int)info.height - 16;
                }
                if (state.pointer_x != old_pointer_x || state.pointer_y != old_pointer_y) {
                    state.surfaces[PANEL_INSPECTOR].dirty = 1;
                    state.needs_present = 1;
                }
            } else if (events[i].data.u32 == 3) {
                if (files_recv_parent_msg(state.files_sock, &files_msg) < 0 ||
                    files_handle_parent_msg(&state, &files_msg) < 0) {
                    log_errno("files client recv failed");
                    return 20;
                }
            } else if (events[i].data.u32 == 4) {
                uint8_t pty_buf[256];
                ssize_t n = read(state.terminal_stdout_fd, pty_buf, sizeof(pty_buf));
                if (n < 0) {
                    if (errno == EIO) {
                        n = 0;
                    } else
                    if (errno == EAGAIN) {
                        continue;
                    }
                    log_errno("terminal pty read failed");
                    return 21;
                }
                if (n == 0) {
                    log_line("[peet-shell] terminal-pipe-eof\n");
                    set_status(&state, "TOYBOX SHELL EXITED");
                    state.terminal_ready = 0;
                    continue;
                }
                terminal_consume_output(&state, pty_buf, (size_t)n);
            }
        }
        if (state.needs_present) {
            compose_shell(&state, fb, &info);
            state.needs_present = 0;
        }
    }

    if (state.files_sock >= 0) {
        (void)files_send_msg(state.files_sock, FILES_MSG_QUIT, 0u, 0u, 0u);
        close(state.files_sock);
    }
    if (state.terminal_master_fd >= 0) {
        close(state.terminal_master_fd);
    }
    if (state.terminal_stdout_fd >= 0 && state.terminal_stdout_fd != state.terminal_master_fd) {
        close(state.terminal_stdout_fd);
    }
    if (state.files_child_pid > 0) {
        if (waitpid(state.files_child_pid, &status, 0) < 0) {
            log_errno("files waitpid failed");
            return 22;
        }
    }
    if (state.terminal_child_pid > 0) {
        if (waitpid(state.terminal_child_pid, &status, 0) < 0) {
            log_errno("terminal waitpid failed");
            return 23;
        }
    }
    log_line("[peet-shell] exit\n");
    return 0;
}
