#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
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

static void log_line(const char *line) {
    write_all(1, line, strlen(line));
}

static void log_errno(const char *tag) {
    char buf[160];
    int n = snprintf(buf, sizeof(buf), "[epoll-smoke] %s errno=%d\n", tag, errno);
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static void log_wait_result(const char *tag, int ready, const struct epoll_event *events) {
    char buf[192];
    unsigned int mask = 0;
    unsigned int id = 0;
    if (ready > 0 && events != NULL) {
        mask = events[0].events;
        id = events[0].data.u32;
    }
    int n = snprintf(
        buf,
        sizeof(buf),
        "[epoll-smoke] %s ready=%d errno=%d event_mask=0x%x event_id=%u\n",
        tag,
        ready,
        errno,
        mask,
        id
    );
    if (n > 0) {
        write_all(1, buf, (size_t)n);
    }
}

static int child_client(int gate_fd, int sock_fd, const char *msg) {
    char gate = 0;
    ssize_t gate_read = read(gate_fd, &gate, 1);
    if (gate_read != 1) {
        char buf[160];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[epoll-smoke] child gate read failed n=%ld errno=%d\n",
            (long)gate_read,
            errno
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
        return 21;
    }
    {
        char buf[160];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[epoll-smoke] child gate read ok byte=0x%02x\n",
            (unsigned int)(unsigned char)gate
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    {
        ssize_t wrote = write(sock_fd, msg, strlen(msg));
        if (wrote != (ssize_t)strlen(msg)) {
            char buf[160];
            int n = snprintf(
                buf,
                sizeof(buf),
                "[epoll-smoke] child socket write failed n=%ld errno=%d\n",
                (long)wrote,
                errno
            );
            if (n > 0) {
                write_all(1, buf, (size_t)n);
            }
            return 22;
        }
    }
    {
        char buf[160];
        int n = snprintf(
            buf,
            sizeof(buf),
            "[epoll-smoke] child socket write ok len=%zu\n",
            strlen(msg)
        );
        if (n > 0) {
            write_all(1, buf, (size_t)n);
        }
    }
    for (;;) {
        gate_read = read(gate_fd, &gate, 1);
        if (gate_read == 0) {
            return 0;
        }
        if (gate_read < 0) {
            char buf[160];
            int n = snprintf(
                buf,
                sizeof(buf),
                "[epoll-smoke] child gate wait-for-close failed n=%ld errno=%d\n",
                (long)gate_read,
                errno
            );
            if (n > 0) {
                write_all(1, buf, (size_t)n);
            }
            return 23;
        }
    }
}

static int expect_idle(int epfd) {
    struct epoll_event events[2];
    int ready = epoll_wait(epfd, events, 2, 50);
    if (ready != 0) {
        log_wait_result("epoll_wait should have timed out while idle", ready, events);
        return -1;
    }
    return 0;
}

static int expect_event(int epfd, int expected_id, int fd, const char *expected_msg) {
    struct epoll_event events[2];
    int ready = epoll_wait(epfd, events, 2, 2000);
    if (ready != 1) {
        log_wait_result("epoll_wait did not yield exactly one ready client", ready, events);
        return -1;
    }
    if ((int)events[0].data.u32 != expected_id) {
        log_wait_result("epoll_wait returned the wrong client id", ready, events);
        return -1;
    }

    char buf[32];
    ssize_t n = read(fd, buf, sizeof(buf));
    if (n != (ssize_t)strlen(expected_msg)) {
        log_line("[epoll-smoke] client payload length mismatch\n");
        return -1;
    }
    if (memcmp(buf, expected_msg, (size_t)n) != 0) {
        log_line("[epoll-smoke] client payload mismatch\n");
        return -1;
    }
    return 0;
}

int main(void) {
    int client1[2] = {-1, -1};
    int client2[2] = {-1, -1};
    int gate1[2] = {-1, -1};
    int gate2[2] = {-1, -1};

    log_line("[epoll-smoke] start\n");

    if (socketpair(AF_UNIX, SOCK_STREAM, 0, client1) < 0) {
        log_errno("socketpair client1 failed");
        return 1;
    }
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, client2) < 0) {
        log_errno("socketpair client2 failed");
        return 2;
    }
    if (pipe(gate1) < 0 || pipe(gate2) < 0) {
        log_errno("pipe setup failed");
        return 3;
    }

    pid_t child1_pid = fork();
    if (child1_pid < 0) {
        log_errno("fork child1 failed");
        return 4;
    }
    if (child1_pid == 0) {
        close(client1[0]);
        close(client2[0]);
        close(client2[1]);
        close(gate1[1]);
        close(gate2[0]);
        close(gate2[1]);
        _exit(child_client(gate1[0], client1[1], "client-one"));
    }

    pid_t child2_pid = fork();
    if (child2_pid < 0) {
        log_errno("fork child2 failed");
        return 5;
    }
    if (child2_pid == 0) {
        close(client2[0]);
        close(client1[0]);
        close(client1[1]);
        close(gate2[1]);
        close(gate1[0]);
        close(gate1[1]);
        _exit(child_client(gate2[0], client2[1], "client-two"));
    }

    close(client1[1]);
    close(client2[1]);
    close(gate1[0]);
    close(gate2[0]);

    int epfd = epoll_create1(0);
    if (epfd < 0) {
        log_errno("epoll_create1 failed");
        return 6;
    }

    struct epoll_event ev1;
    memset(&ev1, 0, sizeof(ev1));
    ev1.events = EPOLLIN | EPOLLHUP;
    ev1.data.u32 = 1;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, client1[0], &ev1) < 0) {
        log_errno("epoll_ctl add client1 failed");
        return 7;
    }

    struct epoll_event ev2;
    memset(&ev2, 0, sizeof(ev2));
    ev2.events = EPOLLIN | EPOLLHUP;
    ev2.data.u32 = 2;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, client2[0], &ev2) < 0) {
        log_errno("epoll_ctl add client2 failed");
        return 8;
    }

    if (expect_idle(epfd) < 0) {
        return 9;
    }

    if (write(gate1[1], "1", 1) != 1) {
        log_errno("gate1 write failed");
        return 10;
    }
    if (expect_event(epfd, 1, client1[0], "client-one") < 0) {
        return 11;
    }

    if (expect_idle(epfd) < 0) {
        return 12;
    }

    if (write(gate2[1], "1", 1) != 1) {
        log_errno("gate2 write failed");
        return 13;
    }
    if (expect_event(epfd, 2, client2[0], "client-two") < 0) {
        return 14;
    }

    close(gate1[1]);
    close(gate2[1]);
    close(client1[0]);
    close(client2[0]);
    close(epfd);

    int status1 = 0;
    int status2 = 0;
    if (waitpid(child1_pid, &status1, 0) < 0 || waitpid(child2_pid, &status2, 0) < 0) {
        log_errno("waitpid failed");
        return 15;
    }
    if (!WIFEXITED(status1) || WEXITSTATUS(status1) != 0) {
        log_line("[epoll-smoke] child1 failed\n");
        return 16;
    }
    if (!WIFEXITED(status2) || WEXITSTATUS(status2) != 0) {
        log_line("[epoll-smoke] child2 failed\n");
        return 17;
    }

    log_line("[epoll-smoke] pass\n");
    return 0;
}
