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
    const char *target = "/mnt/ext2/bin/echo";
    char *const argv[] = {
        (char *)target,
        (char *)"hello",
        (char *)"from-ext2",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/bin",
        (char *)"TERM=peetos",
        0,
    };

    write_all(1, "[musl-ext2-exec] launching /mnt/ext2/bin/echo\n", 45);
    execve(target, argv, envp);

    write_all(2, "[musl-ext2-exec] execve returned -1\n", 36);
    _exit(111);
}
