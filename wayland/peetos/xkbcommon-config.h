/*
 * libxkbcommon build configuration for PeetOS (static, musl, x86_64).
 *
 * Replaces the config.h that libxkbcommon's meson build would generate. The
 * same file is used for the host build that verifies the staged keymap data
 * (glibc provides the same functions).
 */
#ifndef PEETOS_XKBCOMMON_CONFIG_H
#define PEETOS_XKBCOMMON_CONFIG_H

#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif

#define EXIT_INVALID_USAGE 2
#define LIBXKBCOMMON_VERSION "1.7.0"
#define LIBXKBCOMMON_TOOL_PATH "/usr/libexec/xkbcommon"

/* xkeyboard-config subset staged on the ext2 image (see docs/wayland.md). */
#define DFLT_XKB_CONFIG_ROOT "/usr/share/X11/xkb"
#define DFLT_XKB_CONFIG_EXTRA_PATH "/etc/xkb"
#define XLOCALEDIR "/usr/share/X11/locale"

#define DEFAULT_XKB_RULES "evdev"
#define DEFAULT_XKB_MODEL "pc105"
#define DEFAULT_XKB_LAYOUT "us"
#define DEFAULT_XKB_VARIANT NULL
#define DEFAULT_XKB_OPTIONS NULL

#define HAVE_UNISTD_H 1
#define HAVE___BUILTIN_EXPECT 1
#define HAVE_EACCESS 1
#define HAVE_EUIDACCESS 1
#define HAVE_MMAP 1
#define HAVE_MKOSTEMP 1
#define HAVE_POSIX_FALLOCATE 1
#define HAVE_STRNDUP 1
#define HAVE_ASPRINTF 1
#define HAVE_SECURE_GETENV 1

#endif
