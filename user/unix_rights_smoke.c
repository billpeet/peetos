#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef MSG_CTRUNC
#define MSG_CTRUNC 0x08
#endif

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

static void log_line(const char *line) {
    write_all(1, line, strlen(line));
}

static void log_errno(const char *tag) {
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "[unix-rights] %s errno=%d\n", tag, errno);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static int send_one_fd(int sock, int fd_to_send) {
    char byte = 'R';
    struct iovec iov = {
        .iov_base = &byte,
        .iov_len = 1,
    };
    char ctrl[CMSG_SPACE(sizeof(int))];
    memset(ctrl, 0, sizeof(ctrl));

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl;
    msg.msg_controllen = sizeof(ctrl);

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    if (!cmsg) {
        return -1;
    }
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(cmsg), &fd_to_send, sizeof(fd_to_send));

    ssize_t sent = sendmsg(sock, &msg, 0);
    if (sent < 0) {
        return -1;
    }
    return 0;
}

static int recv_one_fd(int sock, int *out_fd, int *out_flags, void *ctrl, size_t ctrl_len) {
    char byte = 0;
    struct iovec iov = {
        .iov_base = &byte,
        .iov_len = 1,
    };
    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl;
    msg.msg_controllen = ctrl_len;

    ssize_t n = recvmsg(sock, &msg, 0);
    if (n < 0) {
        return -1;
    }
    if (out_flags) {
        *out_flags = msg.msg_flags;
    }
    if (out_fd) {
        *out_fd = -1;
        struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
        if (cmsg && cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
            if (cmsg->cmsg_len >= CMSG_LEN(sizeof(int))) {
                memcpy(out_fd, CMSG_DATA(cmsg), sizeof(int));
            }
        }
    }

    return (int)n;
}

static int test_sendmsg_invalid_fd(void) {
    int sv[2] = {-1, -1};
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        log_errno("socketpair invalid-fd test failed");
        return -1;
    }

    char byte = 'x';
    struct iovec iov = {.iov_base = &byte, .iov_len = 1};
    char ctrl[CMSG_SPACE(sizeof(int))];
    memset(ctrl, 0, sizeof(ctrl));

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl;
    msg.msg_controllen = sizeof(ctrl);

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    int bad_fd = -1;
    memcpy(CMSG_DATA(cmsg), &bad_fd, sizeof(bad_fd));

    errno = 0;
    ssize_t rc = sendmsg(sv[0], &msg, 0);
    int ok = (rc == -1 && errno == EBADF);

    close(sv[0]);
    close(sv[1]);

    if (!ok) {
        log_errno("invalid-fd sendmsg did not return EBADF");
        return -1;
    }
    return 0;
}

static int test_sendmsg_malformed_cmsg(void) {
    int sv[2] = {-1, -1};
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        log_errno("socketpair malformed-cmsg test failed");
        return -1;
    }

    char byte = 'm';
    struct iovec iov = {.iov_base = &byte, .iov_len = 1};
    char ctrl[sizeof(struct cmsghdr)];
    memset(ctrl, 0, sizeof(ctrl));

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl;
    msg.msg_controllen = sizeof(ctrl);

    struct cmsghdr *cmsg = (struct cmsghdr *)ctrl;
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = sizeof(struct cmsghdr) - 1;

    errno = 0;
    ssize_t rc = sendmsg(sv[0], &msg, 0);
    int ok = (rc == -1 && errno == EINVAL);

    close(sv[0]);
    close(sv[1]);

    if (!ok) {
        log_errno("malformed-cmsg sendmsg did not return EINVAL");
        return -1;
    }
    return 0;
}

static int test_recvmsg_ctrunc_drops_rights(void) {
    int sv[2] = {-1, -1};
    int p[2] = {-1, -1};
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        log_errno("socketpair ctrunc test failed");
        return -1;
    }
    if (pipe(p) < 0) {
        log_errno("pipe ctrunc test failed");
        close(sv[0]);
        close(sv[1]);
        return -1;
    }

    if (send_one_fd(sv[0], p[0]) < 0) {
        log_errno("send_one_fd ctrunc test failed");
        close(p[0]);
        close(p[1]);
        close(sv[0]);
        close(sv[1]);
        return -1;
    }

    char ctrl[sizeof(struct cmsghdr)];
    int got_fd = -1;
    int msg_flags = 0;
    int n = recv_one_fd(sv[1], &got_fd, &msg_flags, ctrl, sizeof(ctrl));

    close(p[0]);
    close(p[1]);
    close(sv[0]);
    close(sv[1]);

    if (n != 1 || got_fd != -1 || (msg_flags & MSG_CTRUNC) == 0) {
        log_line("[unix-rights] recvmsg ctrunc behavior mismatch\n");
        return -1;
    }

    return 0;
}

static int child_recv_and_use_fd(int sock) {
    char ctrl[CMSG_SPACE(sizeof(int))];
    int got_fd = -1;
    int msg_flags = 0;
    int n = recv_one_fd(sock, &got_fd, &msg_flags, ctrl, sizeof(ctrl));
    if (n != 1 || got_fd < 0 || (msg_flags & MSG_CTRUNC) != 0) {
        return 21;
    }

    char buf[16];
    ssize_t r = read(got_fd, buf, sizeof(buf));
    close(got_fd);
    if (r != 6) {
        return 22;
    }
    if (memcmp(buf, "phase1", 6) != 0) {
        return 23;
    }

    return 0;
}

static int test_cross_process_fd_passing(void) {
    int sv[2] = {-1, -1};
    int p[2] = {-1, -1};

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        log_errno("socketpair cross-process test failed");
        return -1;
    }
    if (pipe(p) < 0) {
        log_errno("pipe cross-process test failed");
        close(sv[0]);
        close(sv[1]);
        return -1;
    }

    pid_t pid = fork();
    if (pid < 0) {
        log_errno("fork cross-process test failed");
        close(p[0]);
        close(p[1]);
        close(sv[0]);
        close(sv[1]);
        return -1;
    }

    if (pid == 0) {
        close(sv[0]);
        close(p[0]);
        close(p[1]);
        int rc = child_recv_and_use_fd(sv[1]);
        close(sv[1]);
        _exit(rc);
    }

    close(sv[1]);
    if (send_one_fd(sv[0], p[0]) < 0) {
        log_errno("send_one_fd cross-process test failed");
        close(p[0]);
        close(p[1]);
        close(sv[0]);
        return -1;
    }

    // Sender closes the original fd after transfer; receiver copy must remain valid.
    close(p[0]);
    if (write(p[1], "phase1", 6) != 6) {
        log_errno("write to transferred pipe failed");
        close(p[1]);
        close(sv[0]);
        return -1;
    }
    close(p[1]);
    close(sv[0]);

    int status = 0;
    if (waitpid(pid, &status, 0) < 0) {
        log_errno("waitpid failed");
        return -1;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        char buf[128];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[unix-rights] child failed status=0x%x code=%d\\n",
            status,
            WEXITSTATUS(status)
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
        return -1;
    }

    return 0;
}

int main(void) {
    log_line("[unix-rights] start\n");

    if (test_sendmsg_invalid_fd() < 0) {
        return 1;
    }
    if (test_sendmsg_malformed_cmsg() < 0) {
        return 2;
    }
    if (test_recvmsg_ctrunc_drops_rights() < 0) {
        return 3;
    }
    if (test_cross_process_fd_passing() < 0) {
        return 4;
    }

    log_line("[unix-rights] pass\n");
    return 0;
}
