#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef SYS_memfd_create
#define SYS_memfd_create 319
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
    char buf[160];
    int n = snprintf(buf, sizeof(buf), "[shared-buffer] %s errno=%d\n", tag, errno);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static int send_one_fd(int sock, int fd_to_send) {
    char byte = 'B';
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

    return sendmsg(sock, &msg, 0) == 1 ? 0 : -1;
}

static int recv_one_fd(int sock) {
    char byte = 0;
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

    if (recvmsg(sock, &msg, 0) != 1) {
        return -1;
    }

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    if (!cmsg || cmsg->cmsg_level != SOL_SOCKET || cmsg->cmsg_type != SCM_RIGHTS) {
        return -1;
    }
    if (cmsg->cmsg_len < CMSG_LEN(sizeof(int))) {
        return -1;
    }

    int fd = -1;
    memcpy(&fd, CMSG_DATA(cmsg), sizeof(fd));
    return fd;
}

int main(void) {
    const size_t map_len = 4096;
    int sv[2] = {-1, -1};

    log_line("[shared-buffer] start\n");

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        log_errno("socketpair failed");
        return 1;
    }

    int memfd = (int)syscall(SYS_memfd_create, "wayland-buffer", 0);
    if (memfd < 0) {
        log_errno("memfd_create failed");
        return 2;
    }
    if (ftruncate(memfd, (off_t)map_len) < 0) {
        log_errno("ftruncate failed");
        return 3;
    }

    uint32_t *sender = mmap(0, map_len, PROT_READ | PROT_WRITE, MAP_SHARED, memfd, 0);
    if (sender == MAP_FAILED) {
        log_errno("sender mmap failed");
        return 4;
    }

    sender[0] = 0x11223344u;
    sender[1] = 0x55667788u;
    sender[2] = 0x99AABBCCu;
    sender[3] = 0xDDEEFF00u;

    pid_t pid = fork();
    if (pid < 0) {
        log_errno("fork failed");
        return 5;
    }

    if (pid == 0) {
        close(sv[0]);
        close(memfd);
        if (munmap(sender, map_len) < 0) {
            _exit(20);
        }

        int recv_fd = recv_one_fd(sv[1]);
        if (recv_fd < 0) {
            _exit(21);
        }

        uint32_t *receiver = mmap(0, map_len, PROT_READ | PROT_WRITE, MAP_SHARED, recv_fd, 0);
        if (receiver == MAP_FAILED) {
            _exit(22);
        }
        if (receiver[0] != 0x11223344u || receiver[3] != 0xDDEEFF00u) {
            _exit(23);
        }

        receiver[0] = 0xCAFEBABEu;
        receiver[3] = 0x0BADF00Du;
        if (write(sv[1], "A", 1) != 1) {
            _exit(24);
        }

        char cmd = 0;
        if (read(sv[1], &cmd, 1) != 1 || cmd != 'B') {
            _exit(25);
        }
        if (receiver[1] != 0x12345678u || receiver[2] != 0x87654321u) {
            _exit(26);
        }
        if (write(sv[1], "C", 1) != 1) {
            _exit(27);
        }
        _exit(0);
    }

    close(sv[1]);
    if (send_one_fd(sv[0], memfd) < 0) {
        log_errno("sendmsg rights transfer failed");
        return 6;
    }
    close(memfd);

    char ack = 0;
    if (read(sv[0], &ack, 1) != 1 || ack != 'A') {
        log_line("[shared-buffer] child did not acknowledge first mutation\n");
        return 7;
    }
    if (sender[0] != 0xCAFEBABEu || sender[3] != 0x0BADF00Du) {
        log_line("[shared-buffer] parent mapping did not observe child writes\n");
        return 8;
    }

    sender[1] = 0x12345678u;
    sender[2] = 0x87654321u;
    if (write(sv[0], "B", 1) != 1) {
        log_errno("parent ack write failed");
        return 9;
    }

    ack = 0;
    if (read(sv[0], &ack, 1) != 1 || ack != 'C') {
        log_line("[shared-buffer] child did not observe parent writes\n");
        return 10;
    }

    int status = 0;
    if (waitpid(pid, &status, 0) < 0) {
        log_errno("waitpid failed");
        return 11;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        log_line("[shared-buffer] child failed\n");
        return 12;
    }

    close(sv[0]);
    munmap(sender, map_len);
    log_line("[shared-buffer] pass\n");
    return 0;
}
