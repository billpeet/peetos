#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

static int (*real_linkat_fn)(int, const char *, int, const char *, int);
static int (*real_link_fn)(const char *, const char *);
static int (*real_rename_fn)(const char *, const char *);
static int (*real_renameat_fn)(int, const char *, int, const char *);

static void init_real_fns(void) {
    if (!real_linkat_fn) real_linkat_fn = dlsym(RTLD_NEXT, "linkat");
    if (!real_link_fn) real_link_fn = dlsym(RTLD_NEXT, "link");
    if (!real_rename_fn) real_rename_fn = dlsym(RTLD_NEXT, "rename");
    if (!real_renameat_fn) real_renameat_fn = dlsym(RTLD_NEXT, "renameat");
}

static int copy_all_fds(int src_fd, int dst_fd) {
    char buf[1 << 16];
    for (;;) {
        ssize_t n = read(src_fd, buf, sizeof(buf));
        if (n == 0) return 0;
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        ssize_t off = 0;
        while (off < n) {
            ssize_t w = write(dst_fd, buf + off, (size_t)(n - off));
            if (w < 0) {
                if (errno == EINTR) continue;
                return -1;
            }
            off += w;
        }
    }
}

static int copy_file_at(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, bool replace) {
    int src_fd = openat(olddirfd, oldpath, O_RDONLY);
    if (src_fd < 0) return -1;

    struct stat st;
    if (fstat(src_fd, &st) != 0) {
        int e = errno;
        close(src_fd);
        errno = e;
        return -1;
    }

    int open_flags = O_WRONLY | O_CREAT | (replace ? O_TRUNC : O_EXCL);
    int dst_fd = openat(newdirfd, newpath, open_flags, st.st_mode & 0777);
    if (dst_fd < 0) {
        int e = errno;
        close(src_fd);
        errno = e;
        return -1;
    }

    if (copy_all_fds(src_fd, dst_fd) != 0) {
        int e = errno;
        close(dst_fd);
        close(src_fd);
        unlinkat(newdirfd, newpath, 0);
        errno = e;
        return -1;
    }

    if (close(dst_fd) != 0) {
        int e = errno;
        close(src_fd);
        unlinkat(newdirfd, newpath, 0);
        errno = e;
        return -1;
    }

    close(src_fd);
    return 0;
}

int linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags) {
    init_real_fns();
    int rc = real_linkat_fn(olddirfd, oldpath, newdirfd, newpath, flags);
    if (rc == 0 || errno != EXDEV) return rc;
    return copy_file_at(olddirfd, oldpath, newdirfd, newpath, false);
}

int link(const char *oldpath, const char *newpath) {
    init_real_fns();
    int rc = real_link_fn(oldpath, newpath);
    if (rc == 0 || errno != EXDEV) return rc;
    return copy_file_at(AT_FDCWD, oldpath, AT_FDCWD, newpath, false);
}

int renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath) {
    init_real_fns();
    int rc = real_renameat_fn(olddirfd, oldpath, newdirfd, newpath);
    if (rc == 0 || errno != EXDEV) return rc;

    if (copy_file_at(olddirfd, oldpath, newdirfd, newpath, true) != 0) return -1;
    if (unlinkat(olddirfd, oldpath, 0) != 0) return -1;
    return 0;
}

int rename(const char *oldpath, const char *newpath) {
    init_real_fns();
    int rc = real_rename_fn(oldpath, newpath);
    if (rc == 0 || errno != EXDEV) return rc;

    if (copy_file_at(AT_FDCWD, oldpath, AT_FDCWD, newpath, true) != 0) return -1;
    if (unlink(oldpath) != 0) return -1;
    return 0;
}

int renameat2(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, unsigned int flags) {
    long rc = syscall(SYS_renameat2, olddirfd, oldpath, newdirfd, newpath, flags);
    if (rc == 0 || errno != EXDEV) return (int)rc;
    if (flags != 0) {
        errno = EXDEV;
        return -1;
    }

    if (copy_file_at(olddirfd, oldpath, newdirfd, newpath, true) != 0) return -1;
    if (unlinkat(olddirfd, oldpath, 0) != 0) return -1;
    return 0;
}
