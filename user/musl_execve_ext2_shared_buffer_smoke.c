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
        (char *)"bin/shared_buffer_smoke.sh",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        0,
    };

    static const char launch_msg[] =
        "[musl-ext2-shared-buffer-smoke] launching shared-buffer smoke via bin/shared_buffer_smoke.sh\n";
    write_all(1, launch_msg, sizeof(launch_msg) - 1);
    chdir("/mnt/ext2");
    execve(target, argv, envp);

    write_all(2, "[musl-ext2-shared-buffer-smoke] execve returned -1\n", 52);
    _exit(111);
}
