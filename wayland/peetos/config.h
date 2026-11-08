/*
 * libwayland build configuration for PeetOS (static, musl, x86_64).
 *
 * Replaces the config.h that libwayland's meson build would generate.
 * Values reflect what the musl sysroot provides and what the PeetOS kernel
 * implements (see docs/wayland.md).
 */
#ifndef PEETOS_WAYLAND_CONFIG_H
#define PEETOS_WAYLAND_CONFIG_H

#define PACKAGE "wayland"
#define PACKAGE_VERSION "1.23.1"

/* Headers. Linux-style SO_PEERCRED is used; no BSD credential headers. */
#define HAVE_SYS_PRCTL_H 1
#undef HAVE_SYS_PROCTL_H
#undef HAVE_SYS_UCRED_H

/* libc functions backed by PeetOS syscalls. */
#define HAVE_ACCEPT4 1
#define HAVE_MKOSTEMP 1
#define HAVE_POSIX_FALLOCATE 1
#define HAVE_PRCTL 1
#define HAVE_MEMFD_CREATE 1
#define HAVE_MREMAP 1
#define HAVE_STRNDUP 1

#define HAVE_XUCRED_CR_PID 0
#define HAVE_BROKEN_MSG_CMSG_CLOEXEC 0

#endif
