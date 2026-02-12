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

int main(int argc, char **argv) {
    for (int i = 1; i < argc; ++i) {
        if (i > 1) {
            write_all(1, " ", 1);
        }
        write_all(1, argv[i], strlen(argv[i]));
    }
    write_all(1, "\n", 1);

    return 0;
}
