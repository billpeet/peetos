/*
 * Build configuration for upstream libwayland's test suite on PeetOS.
 *
 * Force-included (-include) ahead of every test source by
 * scripts/build_wayland_tests.sh. It pulls in the normal PeetOS config.h
 * first; the include guard then turns the tests' own `#include "config.h"`
 * into a no-op, so the overrides below stick without editing upstream files.
 */
#ifndef PEETOS_WAYLAND_TESTS_CONFIG_H
#define PEETOS_WAYLAND_TESTS_CONFIG_H

#include "config.h"

/*
 * test-runner.c's is_debugger_attached() (selected by HAVE_SYS_PRCTL_H) forks
 * a child that PTRACE_ATTACHes to the runner and treats any failure as "a
 * debugger is attached", which silently disables the fd-leak checks and the
 * per-test SIGALRM timeouts. PeetOS has no ptrace, so take the generic
 * branch, which reports "no debugger" and keeps both checks on.
 * (test-helpers.c also skips prctl(PR_SET_DUMPABLE); PeetOS never dumps core.)
 */
#undef HAVE_SYS_PRCTL_H

#endif
