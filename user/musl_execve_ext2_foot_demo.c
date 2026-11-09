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
        (char *)"bin/foot_demo.sh",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        0,
    };

    static const char launch_msg[] =
        "[musl-ext2-foot-demo] launching interactive foot terminal via bin/foot_demo.sh\n";
    write_all(1, launch_msg, sizeof(launch_msg) - 1);
    chdir("/mnt/ext2");
    execve(target, argv, envp);

    static const char fail_msg[] = "[musl-ext2-foot-demo] execve returned -1\n";
    write_all(2, fail_msg, sizeof(fail_msg) - 1);
    _exit(111);
}
