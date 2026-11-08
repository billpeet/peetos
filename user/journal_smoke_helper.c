/* ext3 journal crash/recovery workload (musl-ext3-journal-smoke).
 *
 *   journal_workload write   - boot 1: sync a baseline file and a directory
 *                              of BIG_COUNT hard links (large enough to need
 *                              an htree index), then write files in small
 *                              chunks until the kernel has committed (but not
 *                              checkpointed) journal transactions, announce
 *                              the crash point and wait to be killed.
 *   journal_workload verify  - boot 2: after kernel journal recovery, the
 *                              baseline must be intact and every surviving
 *                              file must be a byte-exact prefix of what was
 *                              written (writes are not atomic across
 *                              write() calls, but must never be garbage).
 *                              Every big-directory name must resolve, and
 *                              getdents64 must list them all.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define FILE_COUNT 48
#define LINES_PER_FILE 64
#define CHUNK 512
#define MAX_CONTENT (LINES_PER_FILE * 96)
#define BIG_COUNT 1500

static void say(const char *msg) {
    write(1, msg, strlen(msg));
}

static size_t gen(const char *name, char *out) {
    size_t len = 0;
    for (int i = 0; i < LINES_PER_FILE; i++) {
        len += (size_t)snprintf(out + len, MAX_CONTENT - len,
                                "ext3 journal smoke file %s line %d padding-padding-padding\n",
                                name, i);
    }
    return len;
}

static void say_errno(const char *what, const char *path) {
    char msg[160];
    snprintf(msg, sizeof(msg), "[ext3-journal-smoke] %s %s failed errno=%d\n", what, path, errno);
    say(msg);
}

static int write_file(const char *path, const char *data, size_t len) {
    int fd = open(path, O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (fd < 0) {
        say_errno("open", path);
        return -1;
    }
    size_t done = 0;
    while (done < len) {
        size_t chunk = len - done < CHUNK ? len - done : CHUNK;
        ssize_t n = write(fd, data + done, chunk);
        if (n <= 0) {
            say_errno("write", path);
            close(fd);
            return -1;
        }
        done += (size_t)n;
    }
    close(fd);
    return 0;
}

/* Returns bytes read, or -1 if the file does not exist. */
static long read_file(const char *path, char *out, size_t cap) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    size_t total = 0;
    for (;;) {
        ssize_t n = read(fd, out + total, cap - total);
        if (n <= 0) {
            break;
        }
        total += (size_t)n;
        if (total == cap) {
            break;
        }
    }
    close(fd);
    return (long)total;
}

static void big_name(int n, char *out, size_t cap) {
    snprintf(out, cap, "jsmoke/big/entry_%05d_with_a_longer_name", n);
}

static int write_bigdir(void) {
    char path[96];
    if (mkdirat(AT_FDCWD, "jsmoke/big", 0755) != 0 && errno != EEXIST) {
        say_errno("mkdir", "jsmoke/big");
        return -1;
    }
    if (write_file("jsmoke/big/hub", "hub\n", 4) != 0) {
        return -1;
    }
    for (int n = 0; n < BIG_COUNT; n++) {
        big_name(n, path, sizeof(path));
        if (linkat(AT_FDCWD, "jsmoke/big/hub", AT_FDCWD, path, 0) != 0) {
            say_errno("link", path);
            return -1;
        }
    }
    return 0;
}

static int verify_bigdir(void) {
    char path[96];
    char msg[128];
    struct stat st;
    for (int n = 0; n < BIG_COUNT; n++) {
        big_name(n, path, sizeof(path));
        if (stat(path, &st) != 0) {
            say_errno("stat", path);
            return -1;
        }
    }
    if (stat("jsmoke/big/hub", &st) != 0 || st.st_nlink != BIG_COUNT + 1) {
        say("[ext3-journal-smoke] FAIL bigdir hub link count\n");
        return -1;
    }
    DIR *dir = opendir("jsmoke/big");
    if (!dir) {
        say_errno("opendir", "jsmoke/big");
        return -1;
    }
    int listed = 0;
    struct dirent *de;
    while ((de = readdir(dir)) != NULL) {
        if (strcmp(de->d_name, ".") != 0 && strcmp(de->d_name, "..") != 0) {
            listed++;
        }
    }
    closedir(dir);
    if (listed != BIG_COUNT + 1) {
        snprintf(msg, sizeof(msg), "[ext3-journal-smoke] FAIL bigdir lists %d entries\n", listed);
        say(msg);
        return -1;
    }
    snprintf(msg, sizeof(msg), "[ext3-journal-smoke] bigdir verified %d entries\n", listed);
    say(msg);
    return 0;
}

static void wait_for_kill(void) {
    for (;;) {
        sleep(60);
    }
}

static int do_write(void) {
    static char content[MAX_CONTENT];
    char path[64];

    /* mkdirat: the kernel implements the *at syscall family (legacy
     * SYS_mkdir returns ENOSYS). */
    if (mkdirat(AT_FDCWD, "jsmoke", 0755) != 0 && errno != EEXIST) {
        say_errno("mkdir", "jsmoke");
    }
    size_t len = gen("baseline", content);
    if (write_file("jsmoke/baseline", content, len) != 0) {
        say("[ext3-journal-smoke] FAIL cannot write baseline\n");
        return 1;
    }
    if (write_bigdir() != 0) {
        say("[ext3-journal-smoke] FAIL cannot fill bigdir\n");
        return 1;
    }
    sync();
    say("[ext3-journal-smoke] baseline synced\n");

    for (int n = 0; n < FILE_COUNT; n++) {
        char name[16];
        snprintf(name, sizeof(name), "%d", n);
        snprintf(path, sizeof(path), "jsmoke/f%d", n);
        len = gen(name, content);
        if (write_file(path, content, len) != 0) {
            say("[ext3-journal-smoke] FAIL write error\n");
            return 1;
        }
    }
    say("[ext3-journal-smoke] crash-point\n");
    wait_for_kill();
    return 0;
}

static int do_verify(void) {
    static char expected[MAX_CONTENT];
    static char actual[MAX_CONTENT + 1];
    char path[64];
    char msg[128];
    int fail = 0;
    int survived = 0;

    say("[ext3-journal-smoke] recovery boot\n");
    size_t len = gen("baseline", expected);
    long got = read_file("jsmoke/baseline", actual, sizeof(actual));
    if (got != (long)len || memcmp(actual, expected, len) != 0) {
        say("[ext3-journal-smoke] FAIL baseline damaged after recovery\n");
        fail = 1;
    }
    if (verify_bigdir() != 0) {
        fail = 1;
    }

    for (int n = 0; n < FILE_COUNT; n++) {
        char name[16];
        snprintf(name, sizeof(name), "%d", n);
        snprintf(path, sizeof(path), "jsmoke/f%d", n);
        got = read_file(path, actual, sizeof(actual));
        if (got < 0) {
            continue;
        }
        survived++;
        len = gen(name, expected);
        if ((size_t)got > len || memcmp(actual, expected, (size_t)got) != 0) {
            snprintf(msg, sizeof(msg),
                     "[ext3-journal-smoke] FAIL %s is not a prefix of its data (%ld bytes)\n",
                     path, got);
            say(msg);
            fail = 1;
        }
    }
    snprintf(msg, sizeof(msg), "[ext3-journal-smoke] verified %d surviving files\n", survived);
    say(msg);

    write_file("jsmoke/recovered", "recovered\n", 10);
    sync();
    say("[ext3-journal-smoke] done\n");
    if (fail) {
        say("[ext3-journal-smoke] failures\n");
    }
    wait_for_kill();
    return 0;
}

int main(int argc, char **argv) {
    if (argc >= 2 && strcmp(argv[1], "write") == 0) {
        return do_write();
    }
    if (argc >= 2 && strcmp(argv[1], "verify") == 0) {
        return do_verify();
    }
    say("usage: journal_workload write|verify\n");
    return 2;
}
