/*
 * libwayland end-to-end smoke test for PeetOS.
 *
 * Server mode (default): a minimal compositor built on libwayland-server.
 * It listens on $XDG_RUNTIME_DIR/wayland-0, exposes wl_compositor + wl_shm,
 * then forks and execs itself in client mode.
 *
 * Client mode (argv[1] == "--client"): a libwayland-client app that binds
 * the globals, renders two frames through a sealed memfd shm pool (growing
 * the pool between frames), waits for frame callbacks, signals the server
 * with SIGUSR1, and disconnects.
 *
 * Kernel paths exercised: AF_UNIX pathname bind/listen/connect/accept,
 * flock (socket lock file), SO_PEERCRED, SCM_RIGHTS, epoll, eventfd
 * (wl_display_terminate), timerfd (event-loop timers), signalfd +
 * sigprocmask (signal sources), memfd_create(MFD_ALLOW_SEALING) +
 * F_ADD_SEALS, fallocate, ftruncate seal enforcement, MAP_SHARED mmap, and
 * mremap (wl_shm pool resize).
 *
 * Output markers: "[wl-server] ...", "[wl-client] ...", "[wl-smoke] pass"
 * on success, "[wl-smoke] FAIL ..." otherwise.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <wayland-client.h>
#include <wayland-server.h>

#define CLIENT_ARG "--client"
#define ABS_SELF "/mnt/ext2/bin/wayland_smoke"
#define RUNTIME_DIR "/tmp"
#define SURFACE_W 64
#define SURFACE_H 64
#define FRAME1_COLOR 0xff336699u
#define FRAME2_COLOR 0xff00aa55u
#define TIMER_PERIOD_MS 20
#define WATCHDOG_MS 20000

#ifndef F_ADD_SEALS
#define F_ADD_SEALS 1033
#define F_GET_SEALS 1034
#define F_SEAL_SEAL 0x0001
#define F_SEAL_SHRINK 0x0002
#endif
#ifndef MFD_ALLOW_SEALING
#define MFD_ALLOW_SEALING 0x0002u
#endif

/* ======================================================================= */
/* Server                                                                  */
/* ======================================================================= */

struct server {
    struct wl_display *display;
    struct wl_event_source *timer;
    struct wl_event_source *watchdog;
    pid_t child;
    int commits;
    int frames_ok;
    int timer_fires;
    int signal_seen;
    int client_gone;
    int peer_pid_ok;
    int failed;
};

struct surface_state {
    struct server *srv;
    struct wl_resource *pending_buffer;
    struct wl_resource *frame_cb;
};

static void maybe_finish(struct server *srv) {
    if (srv->signal_seen && srv->client_gone) {
        wl_display_terminate(srv->display);
    }
}

static void surface_destroy(struct wl_client *client, struct wl_resource *res) {
    (void)client;
    wl_resource_destroy(res);
}

static void surface_attach(struct wl_client *client, struct wl_resource *res,
                           struct wl_resource *buffer, int32_t x, int32_t y) {
    (void)client;
    (void)x;
    (void)y;
    struct surface_state *s = wl_resource_get_user_data(res);
    s->pending_buffer = buffer;
}

static void surface_damage(struct wl_client *client, struct wl_resource *res, int32_t x,
                           int32_t y, int32_t w, int32_t h) {
    (void)client;
    (void)res;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
}

static void surface_frame(struct wl_client *client, struct wl_resource *res, uint32_t id) {
    struct surface_state *s = wl_resource_get_user_data(res);
    struct wl_resource *cb = wl_resource_create(client, &wl_callback_interface, 1, id);
    if (cb == NULL) {
        wl_client_post_no_memory(client);
        return;
    }
    if (s->frame_cb != NULL) {
        wl_resource_destroy(s->frame_cb);
    }
    s->frame_cb = cb;
}

static void surface_region_noop(struct wl_client *client, struct wl_resource *res,
                                struct wl_resource *region) {
    (void)client;
    (void)res;
    (void)region;
}

static void surface_commit(struct wl_client *client, struct wl_resource *res) {
    (void)client;
    struct surface_state *s = wl_resource_get_user_data(res);
    struct server *srv = s->srv;

    if (s->pending_buffer != NULL) {
        struct wl_shm_buffer *shm = wl_shm_buffer_get(s->pending_buffer);
        if (shm == NULL) {
            printf("[wl-smoke] FAIL server: committed buffer is not wl_shm\n");
            srv->failed = 1;
        } else {
            int32_t w = wl_shm_buffer_get_width(shm);
            int32_t h = wl_shm_buffer_get_height(shm);
            int32_t stride = wl_shm_buffer_get_stride(shm);
            uint32_t want = srv->commits == 0 ? FRAME1_COLOR : FRAME2_COLOR;

            wl_shm_buffer_begin_access(shm);
            const uint32_t *px = wl_shm_buffer_get_data(shm);
            uint32_t first = px[0];
            uint32_t last = px[(size_t)(h - 1) * (size_t)(stride / 4) + (size_t)(w - 1)];
            wl_shm_buffer_end_access(shm);

            srv->commits++;
            printf("[wl-server] commit %d %dx%d stride=%d first=0x%08x last=0x%08x\n",
                   srv->commits, w, h, stride, first, last);
            if (w != SURFACE_W || h != SURFACE_H || (first & 0xffffffu) != (want & 0xffffffu) ||
                (last & 0xffffffu) != (want & 0xffffffu)) {
                printf("[wl-smoke] FAIL server: commit %d pixels/size wrong (want 0x%08x)\n",
                       srv->commits, want);
                srv->failed = 1;
            } else {
                srv->frames_ok++;
            }
        }
        wl_buffer_send_release(s->pending_buffer);
        s->pending_buffer = NULL;
    }

    if (s->frame_cb != NULL) {
        wl_callback_send_done(s->frame_cb, (uint32_t)srv->commits);
        wl_resource_destroy(s->frame_cb);
        s->frame_cb = NULL;
    }
}

static void surface_set_transform(struct wl_client *client, struct wl_resource *res,
                                  int32_t value) {
    (void)client;
    (void)res;
    (void)value;
}

static const struct wl_surface_interface surface_impl = {
    .destroy = surface_destroy,
    .attach = surface_attach,
    .damage = surface_damage,
    .frame = surface_frame,
    .set_opaque_region = surface_region_noop,
    .set_input_region = surface_region_noop,
    .commit = surface_commit,
    .set_buffer_transform = surface_set_transform,
    .set_buffer_scale = surface_set_transform,
    .damage_buffer = surface_damage,
};

static void surface_resource_destroyed(struct wl_resource *res) {
    struct surface_state *s = wl_resource_get_user_data(res);
    free(s);
}

static void region_destroy(struct wl_client *client, struct wl_resource *res) {
    (void)client;
    wl_resource_destroy(res);
}

static void region_rect_noop(struct wl_client *client, struct wl_resource *res, int32_t x,
                             int32_t y, int32_t w, int32_t h) {
    (void)client;
    (void)res;
    (void)x;
    (void)y;
    (void)w;
    (void)h;
}

static const struct wl_region_interface region_impl = {
    .destroy = region_destroy,
    .add = region_rect_noop,
    .subtract = region_rect_noop,
};

static void compositor_create_surface(struct wl_client *client, struct wl_resource *res,
                                      uint32_t id) {
    struct surface_state *s = calloc(1, sizeof(*s));
    struct wl_resource *surface =
        wl_resource_create(client, &wl_surface_interface, wl_resource_get_version(res), id);
    if (s == NULL || surface == NULL) {
        free(s);
        wl_client_post_no_memory(client);
        return;
    }
    s->srv = wl_resource_get_user_data(res);
    wl_resource_set_implementation(surface, &surface_impl, s, surface_resource_destroyed);
    printf("[wl-server] surface created\n");
}

static void compositor_create_region(struct wl_client *client, struct wl_resource *res,
                                     uint32_t id) {
    (void)res;
    struct wl_resource *region = wl_resource_create(client, &wl_region_interface, 1, id);
    if (region == NULL) {
        wl_client_post_no_memory(client);
        return;
    }
    wl_resource_set_implementation(region, &region_impl, NULL, NULL);
}

static const struct wl_compositor_interface compositor_impl = {
    .create_surface = compositor_create_surface,
    .create_region = compositor_create_region,
};

struct client_watch {
    struct wl_listener destroy;
    struct server *srv;
};

static void on_client_destroy(struct wl_listener *listener, void *data) {
    (void)data;
    struct client_watch *watch = wl_container_of(listener, watch, destroy);
    struct server *srv = watch->srv;
    printf("[wl-server] client disconnected\n");
    srv->client_gone = 1;
    free(watch);
    maybe_finish(srv);
}

static void bind_compositor(struct wl_client *client, void *data, uint32_t version,
                            uint32_t id) {
    struct server *srv = data;
    struct wl_resource *res = wl_resource_create(client, &wl_compositor_interface, version, id);
    if (res == NULL) {
        wl_client_post_no_memory(client);
        return;
    }
    wl_resource_set_implementation(res, &compositor_impl, srv, NULL);

    pid_t pid = 0;
    uid_t uid = 0;
    gid_t gid = 0;
    wl_client_get_credentials(client, &pid, &uid, &gid);
    printf("[wl-server] client bound wl_compositor v%u pid=%d uid=%u gid=%u\n", version,
           (int)pid, (unsigned)uid, (unsigned)gid);
    if (pid == srv->child) {
        srv->peer_pid_ok = 1;
    } else {
        printf("[wl-smoke] FAIL server: SO_PEERCRED pid %d, expected child %d\n", (int)pid,
               (int)srv->child);
        srv->failed = 1;
    }

    struct client_watch *watch = calloc(1, sizeof(*watch));
    if (watch != NULL) {
        watch->srv = srv;
        watch->destroy.notify = on_client_destroy;
        wl_client_add_destroy_listener(client, &watch->destroy);
    }
}

static int on_timer(void *data) {
    struct server *srv = data;
    srv->timer_fires++;
    if (srv->timer_fires == 1) {
        printf("[wl-server] timerfd event-loop timer fired\n");
    }
    wl_event_source_timer_update(srv->timer, TIMER_PERIOD_MS);
    return 0;
}

static int on_watchdog(void *data) {
    struct server *srv = data;
    printf("[wl-smoke] FAIL server: watchdog expired (commits=%d signal=%d client_gone=%d)\n",
           srv->commits, srv->signal_seen, srv->client_gone);
    srv->failed = 1;
    wl_display_terminate(srv->display);
    return 0;
}

static int on_sigusr1(int signum, void *data) {
    struct server *srv = data;
    printf("[wl-server] signalfd delivered signal %d\n", signum);
    srv->signal_seen = 1;
    maybe_finish(srv);
    return 0;
}

static int server_main(const char *self) {
    struct server srv;
    memset(&srv, 0, sizeof(srv));

    setenv("XDG_RUNTIME_DIR", RUNTIME_DIR, 1);
    srv.display = wl_display_create();
    if (srv.display == NULL) {
        printf("[wl-smoke] FAIL server: wl_display_create failed errno=%d\n", errno);
        return 1;
    }
    const char *socket_name = wl_display_add_socket_auto(srv.display);
    if (socket_name == NULL) {
        printf("[wl-smoke] FAIL server: wl_display_add_socket_auto failed errno=%d\n", errno);
        return 1;
    }
    if (wl_display_init_shm(srv.display) != 0) {
        printf("[wl-smoke] FAIL server: wl_display_init_shm failed\n");
        return 1;
    }
    if (wl_global_create(srv.display, &wl_compositor_interface, 4, &srv, bind_compositor) == NULL) {
        printf("[wl-smoke] FAIL server: wl_global_create(wl_compositor) failed\n");
        return 1;
    }

    struct wl_event_loop *loop = wl_display_get_event_loop(srv.display);
    srv.timer = wl_event_loop_add_timer(loop, on_timer, &srv);
    srv.watchdog = wl_event_loop_add_timer(loop, on_watchdog, &srv);
    struct wl_event_source *sig = wl_event_loop_add_signal(loop, SIGUSR1, on_sigusr1, &srv);
    if (srv.timer == NULL || srv.watchdog == NULL || sig == NULL) {
        printf("[wl-smoke] FAIL server: event source creation failed errno=%d\n", errno);
        return 1;
    }
    wl_event_source_timer_update(srv.timer, TIMER_PERIOD_MS);
    wl_event_source_timer_update(srv.watchdog, WATCHDOG_MS);
    printf("[wl-server] listening on %s/%s\n", RUNTIME_DIR, socket_name);

    fflush(stdout);
    srv.child = fork();
    if (srv.child < 0) {
        printf("[wl-smoke] FAIL server: fork failed\n");
        return 1;
    }
    if (srv.child == 0) {
        /* The signal source blocked SIGUSR1 in this process; the client
         * should start with a clean mask. */
        sigset_t none;
        sigemptyset(&none);
        sigprocmask(SIG_SETMASK, &none, NULL);
        char *const args[] = {(char *)self, (char *)CLIENT_ARG, NULL};
        execv(self, args);
        execv(ABS_SELF, args);
        _exit(127);
    }

    wl_display_run(srv.display);

    int status = 0;
    pid_t reaped = waitpid(srv.child, &status, 0);
    int client_ok = reaped == srv.child && WIFEXITED(status) && WEXITSTATUS(status) == 0;
    printf("[wl-server] done commits=%d frames_ok=%d timer_fires=%d signal=%d client_gone=%d "
           "peer_pid_ok=%d client_status=%#x\n",
           srv.commits, srv.frames_ok, srv.timer_fires, srv.signal_seen, srv.client_gone,
           srv.peer_pid_ok, status);

    wl_event_source_remove(sig);
    wl_event_source_remove(srv.timer);
    wl_event_source_remove(srv.watchdog);
    wl_display_destroy(srv.display);

    if (srv.failed || !client_ok || srv.frames_ok != 2 || srv.timer_fires < 1 ||
        !srv.signal_seen || !srv.client_gone || !srv.peer_pid_ok) {
        printf("[wl-smoke] FAIL summary\n");
        return 1;
    }
    printf("[wl-smoke] pass\n");
    return 0;
}

/* ======================================================================= */
/* Client                                                                  */
/* ======================================================================= */

struct client {
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    int xrgb_supported;
    int frame_done;
};

static void shm_format(void *data, struct wl_shm *shm, uint32_t format) {
    (void)shm;
    struct client *c = data;
    if (format == WL_SHM_FORMAT_XRGB8888) {
        c->xrgb_supported = 1;
    }
}

static const struct wl_shm_listener shm_listener = {
    .format = shm_format,
};

static void registry_global(void *data, struct wl_registry *registry, uint32_t name,
                            const char *interface, uint32_t version) {
    struct client *c = data;
    if (strcmp(interface, wl_compositor_interface.name) == 0 && version >= 4) {
        c->compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    } else if (strcmp(interface, wl_shm_interface.name) == 0) {
        c->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
        wl_shm_add_listener(c->shm, &shm_listener, c);
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

static void frame_done(void *data, struct wl_callback *cb, uint32_t time) {
    (void)time;
    struct client *c = data;
    c->frame_done = 1;
    wl_callback_destroy(cb);
}

static const struct wl_callback_listener frame_listener = {
    .done = frame_done,
};

static void fill(uint32_t *px, size_t count, uint32_t color) {
    for (size_t i = 0; i < count; i++) {
        px[i] = color;
    }
}

static int present(struct wl_display *display, struct client *c, struct wl_surface *surface,
                   struct wl_buffer *buffer) {
    c->frame_done = 0;
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, SURFACE_W, SURFACE_H);
    struct wl_callback *cb = wl_surface_frame(surface);
    wl_callback_add_listener(cb, &frame_listener, c);
    wl_surface_commit(surface);
    while (!c->frame_done) {
        if (wl_display_dispatch(display) < 0) {
            printf("[wl-smoke] FAIL client: dispatch failed errno=%d\n", errno);
            return -1;
        }
    }
    return 0;
}

static int client_main(void) {
    struct client c;
    memset(&c, 0, sizeof(c));

    struct wl_display *display = wl_display_connect(NULL);
    if (display == NULL) {
        printf("[wl-smoke] FAIL client: wl_display_connect failed errno=%d\n", errno);
        return 1;
    }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, &c);
    if (wl_display_roundtrip(display) < 0 || wl_display_roundtrip(display) < 0) {
        printf("[wl-smoke] FAIL client: roundtrip failed errno=%d\n", errno);
        return 1;
    }
    if (c.compositor == NULL || c.shm == NULL || !c.xrgb_supported) {
        printf("[wl-smoke] FAIL client: missing globals compositor=%p shm=%p xrgb=%d\n",
               (void *)c.compositor, (void *)c.shm, c.xrgb_supported);
        return 1;
    }
    printf("[wl-client] connected; bound wl_compositor v4 and wl_shm (XRGB8888)\n");

    const int stride = SURFACE_W * 4;
    const size_t frame_bytes = (size_t)stride * SURFACE_H;

    int fd = memfd_create("wl-smoke", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    if (fd < 0) {
        printf("[wl-smoke] FAIL client: memfd_create(MFD_ALLOW_SEALING) errno=%d\n", errno);
        return 1;
    }
    int rc = posix_fallocate(fd, 0, (off_t)frame_bytes);
    if (rc != 0) {
        printf("[wl-smoke] FAIL client: posix_fallocate grow rc=%d\n", rc);
        return 1;
    }
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK) != 0) {
        printf("[wl-smoke] FAIL client: F_ADD_SEALS errno=%d\n", errno);
        return 1;
    }
    int seals = fcntl(fd, F_GET_SEALS);
    if (seals < 0 || (seals & F_SEAL_SHRINK) == 0) {
        printf("[wl-smoke] FAIL client: F_GET_SEALS returned %d\n", seals);
        return 1;
    }
    if (ftruncate(fd, (off_t)frame_bytes / 2) == 0 || errno != EPERM) {
        printf("[wl-smoke] FAIL client: shrinking a SEAL_SHRINK memfd was not refused\n");
        return 1;
    }
    printf("[wl-client] memfd sealed (seals=%#x), shrink refused with EPERM\n", seals);

    uint32_t *map = mmap(NULL, frame_bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) {
        printf("[wl-smoke] FAIL client: mmap errno=%d\n", errno);
        return 1;
    }
    fill(map, frame_bytes / 4, FRAME1_COLOR);

    struct wl_shm_pool *pool = wl_shm_create_pool(c.shm, fd, (int32_t)frame_bytes);
    struct wl_buffer *buf1 = wl_shm_pool_create_buffer(pool, 0, SURFACE_W, SURFACE_H, stride,
                                                       WL_SHM_FORMAT_XRGB8888);
    struct wl_surface *surface = wl_compositor_create_surface(c.compositor);
    if (present(display, &c, surface, buf1) < 0) {
        return 1;
    }
    printf("[wl-client] frame 1 presented\n");

    /* Grow the pool: the server remaps it (mremap) on the next access. */
    rc = posix_fallocate(fd, 0, (off_t)(frame_bytes * 2));
    if (rc != 0) {
        printf("[wl-smoke] FAIL client: posix_fallocate grow rc=%d\n", rc);
        return 1;
    }
    wl_shm_pool_resize(pool, (int32_t)(frame_bytes * 2));
    munmap(map, frame_bytes);
    map = mmap(NULL, frame_bytes * 2, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) {
        printf("[wl-smoke] FAIL client: remap after grow errno=%d\n", errno);
        return 1;
    }
    fill(map + frame_bytes / 4, frame_bytes / 4, FRAME2_COLOR);
    struct wl_buffer *buf2 = wl_shm_pool_create_buffer(
        pool, (int32_t)frame_bytes, SURFACE_W, SURFACE_H, stride, WL_SHM_FORMAT_XRGB8888);
    if (present(display, &c, surface, buf2) < 0) {
        return 1;
    }
    printf("[wl-client] frame 2 presented from resized pool\n");

    if (kill(getppid(), SIGUSR1) != 0) {
        printf("[wl-smoke] FAIL client: kill(SIGUSR1) errno=%d\n", errno);
        return 1;
    }

    wl_buffer_destroy(buf2);
    wl_buffer_destroy(buf1);
    wl_surface_destroy(surface);
    wl_shm_pool_destroy(pool);
    wl_shm_destroy(c.shm);
    wl_compositor_destroy(c.compositor);
    wl_registry_destroy(registry);
    wl_display_roundtrip(display);
    wl_display_disconnect(display);
    munmap(map, frame_bytes * 2);
    close(fd);
    printf("[wl-client] disconnected cleanly\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc > 1 && strcmp(argv[1], CLIENT_ARG) == 0) {
        return client_main();
    }
    return server_main(argc > 0 ? argv[0] : ABS_SELF);
}
