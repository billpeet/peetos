#include <string.h>
#include <unistd.h>

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

int main(void) {
    const char *target = "/mnt/ext2/bin/toybox";
    char *const argv[] = {
        (char *)"sh",
        (char *)"bin/peet_gui_smoke.sh",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        (char *)"PEETOS_DISPLAY_MODE=compositor-first",
        (char *)"PEETOS_FB=/dev/fb0",
        (char *)"PEETOS_INPUT_KEYBOARD=/dev/input/event0",
        (char *)"PEETOS_INPUT_POINTER=/dev/input/event1",
        0,
    };

    static const char launch_msg[] =
        "[musl-ext2-peet-gui-smoke] launching peet-gui compositor MVP via bin/peet_gui_smoke.sh\n";
    write_all(1, launch_msg, sizeof(launch_msg) - 1);
    chdir("/mnt/ext2");
    execve(target, argv, envp);

    write_all(2, "[musl-ext2-peet-gui-smoke] execve returned -1\n", 47);
    _exit(111);
}
