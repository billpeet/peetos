/* Minimal <linux/types.h> for PeetOS: the musl sysroot ships no Linux uapi
 * headers, and libdrm's drm.h only needs the fixed-width kernel types. */
#ifndef PEETOS_UAPI_LINUX_TYPES_H
#define PEETOS_UAPI_LINUX_TYPES_H

#include <stddef.h>
#include <stdint.h>

typedef int8_t __s8;
typedef uint8_t __u8;
typedef int16_t __s16;
typedef uint16_t __u16;
typedef int32_t __s32;
typedef uint32_t __u32;
typedef int64_t __s64;
typedef uint64_t __u64;
typedef __u16 __le16;
typedef __u16 __be16;
typedef __u32 __le32;
typedef __u32 __be32;
typedef __u64 __le64;
typedef __u64 __be64;
typedef size_t __kernel_size_t;
/* x86_64 only: <asm/bitsperlong.h> and <asm/posix_types.h> equivalents,
 * which <linux/input.h> uses to pick its struct input_event layout. */
#define __BITS_PER_LONG 64
typedef long __kernel_long_t;
typedef unsigned long __kernel_ulong_t;

#endif
