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
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        (char *)"PS1=\x1b[32m\\w $ \x1b[0m",
        0,
    };

    chdir("/mnt/ext2");
    execve(target, argv, envp);

    write_all(2, "[musl-ext2-exec] execve returned -1\n", 36);
    _exit(111);
}
