#include <string.h>
#include <unistd.h>

/* Init for the musl-ext3-journal-smoke profile: runs the staged journal
 * workload (bin/journal_smoke.sh) on an ext3 root. The host harness
 * (scripts/run_toybox_journal_smoke.sh) kills QEMU mid-workload and boots
 * the crashed image a second time to verify journal recovery. */

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
        (char *)"bin/journal_smoke.sh",
        0,
    };
    char *const envp[] = {
        (char *)"PATH=/mnt/ext2/bin:/bin",
        (char *)"TERM=peetos",
        0,
    };

    static const char launch_msg[] =
        "[ext3-journal-smoke] launching journal workload via bin/journal_smoke.sh\n";
    write_all(1, launch_msg, sizeof(launch_msg) - 1);
    chdir("/mnt/ext2");
    execve(target, argv, envp);

    static const char fail_msg[] = "[ext3-journal-smoke] execve returned -1\n";
    write_all(2, fail_msg, sizeof(fail_msg) - 1);
    _exit(111);
}
