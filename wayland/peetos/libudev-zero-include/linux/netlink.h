/* Minimal <linux/netlink.h> for libudev-zero's monitor (musl ships no Linux
 * uapi headers): the netlink socket address, message header and protocol
 * numbers, with the Linux ABI. The PeetOS kernel implements only
 * NETLINK_KOBJECT_UEVENT sockets.
 *
 * Deliberately not in the shared uapi shims: GLib's configure would see
 * <linux/netlink.h> and build its netlink network monitor, which needs
 * <linux/rtnetlink.h>. */
#ifndef _LINUX_NETLINK_H
#define _LINUX_NETLINK_H

#include <linux/types.h>

#define NETLINK_ROUTE 0
#define NETLINK_KOBJECT_UEVENT 15

struct sockaddr_nl {
	unsigned short nl_family; /* AF_NETLINK */
	unsigned short nl_pad;
	__u32 nl_pid;
	__u32 nl_groups;
};

struct nlmsghdr {
	__u32 nlmsg_len;
	__u16 nlmsg_type;
	__u16 nlmsg_flags;
	__u32 nlmsg_seq;
	__u32 nlmsg_pid;
};

#endif
