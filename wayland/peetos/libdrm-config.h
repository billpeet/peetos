/* libdrm config.h for PeetOS (musl, x86_64): what libdrm's meson build would
 * generate for this target. Only the core library is built, without udev,
 * valgrind or any GPU-specific driver helpers. */
#define _GNU_SOURCE 1
#define HAVE_SYS_SYSCTL_H 0
#define HAVE_SYS_SELECT_H 1
#define HAVE_ALLOCA_H 1
#define MAJOR_IN_SYSMACROS 1
#define HAVE_OPEN_MEMSTREAM 1
#define HAVE_VISIBILITY 1
#define HAVE_LIBDRM_ATOMIC_PRIMITIVES 1
#define HAVE_LIB_ATOMIC_OPS 0
#define UDEV 0
#define HAVE_EXYNOS 0
#define HAVE_FREEDRENO_KGSL 0
#define HAVE_INTEL 0
#define HAVE_NOUVEAU 0
#define HAVE_RADEON 0
#define HAVE_VC4 0
#define HAVE_VMWGFX 0
#define HAVE_CAIRO 0
#define HAVE_VALGRIND 0
