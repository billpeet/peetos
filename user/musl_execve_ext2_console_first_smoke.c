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
        (char *)"bin/console_first_smoke.sh",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        (char *)"PEETOS_DISPLAY_MODE=console-first",
        0,
    };

    static const char launch_msg[] =
        "[musl-ext2-console-first-smoke] launching console-first shell smoke via bin/console_first_smoke.sh\n";

    chdir("/mnt/ext2");
    write_all(1, launch_msg, sizeof(launch_msg) - 1);
    execve(target, argv, envp);

    write_all(2, "[musl-ext2-console-first-smoke] execve returned -1\n", 51);
    _exit(111);
}
