# Userspace Compositor MVP

This document tracks the current landed Phase 4 userspace compositor MVP
baseline.

**Where It Lives**
- `user/gui_compositor_smoke.c`
- `user/musl_execve_ext2_gui_demo.c`
- `user/musl_execve_ext2_gui_smoke.c`
- `kernel/src/gui_protocol.rs`
- `scripts/build_ext2_musl_execve.sh`
- `scripts/run_toybox_gui_demo.sh`
- `scripts/run_toybox_gui_smoke.sh`
- `tests/musl_ext2_gui_demo_setup.sh`
- `tests/musl_ext2_gui_smoke.sh`
- `tests/musl_ext2_gui_smoke_setup.sh`

**Current Scope**
- The `musl-ext2-gui-smoke` profile now boots a dedicated userspace compositor
  process instead of a single framebuffer helper process.
- A separate `musl-ext2-gui-demo` profile reuses the same compositor binary in a
  more presentation-oriented layout for manual bring-up.
- The compositor owns:
  - `/dev/fb0`
  - `/dev/input/event0`
  - `/dev/input/event1`
- The smoke launcher creates multiple client processes in addition to the compositor.
- The current QEMU smoke topology uses four client slots:
  - two long-lived rendering clients that must succeed
  - one transient client that commits once and disconnects
  - one malformed client that sends an attach message without a buffer fd
- The processes communicate over inherited `AF_UNIX` `SOCK_STREAM`
  `socketpair()` channels.

**Current Smoke Protocol**
- The protocol is internal-only and smoke-focused.
- Each client sends fixed-size lifecycle messages over the same stream:
  - `CREATE`
  - `ATTACH`
  - `COMMIT`
  - `DAMAGE`
  - `DESTROY`
- The compositor can also send fixed-size input-delivery messages back over
  the same stream:
  - `INPUT_POINTER`
  - `INPUT_KEY`
- The messages carry:
  - protocol magic/version
  - client id
  - surface id
  - rectangle position
  - rectangle size
  - byte stride
- `ATTACH` is followed by one shared-memory buffer fd on the same `AF_UNIX`
  stream using `sendmsg(... SCM_RIGHTS ...)`.
- The shared buffer is currently created with `memfd_create`, resized with
  `ftruncate`, filled by the client, and mapped read-only by the compositor.
- The compositor keeps explicit pending vs committed surface state:
  - `CREATE` defines surface metadata
  - `ATTACH` stages a pending buffer
  - `COMMIT` promotes the pending buffer to the visible surface and replies
    with a one-byte acknowledgement
  - `DAMAGE` marks a client-provided dirty rectangle relative to the
    committed surface
  - `DESTROY` drops the surface and redraws without it
- After the compositor reaches its success condition, it closes the client
  sockets; clients treat socket close as session completion.

**Current Composition Model**
- The compositor maps `/dev/fb0` with shared writable access.
- Rendering is software-only.
- The current scene contains:
  - a solid background
  - one keyboard-status rectangle
  - one pointer-status rectangle
  - one blitted shared-memory surface per registered client
- One client currently maps to one top-level surface.
- The compositor enforces the current explicit MVP bounds:
  - at most `4` client slots in the smoke session
  - `1` surface per client
  - one attached buffer per client
  - `16 MiB` maximum attached buffer size
- Registration order is the current fixed z-order.
- Surface creation, buffer attach, commit, damage, and destroy are now
  distinct protocol steps.
- Redraw work is now bounded by a single accumulated dirty rectangle per poll
  cycle:
  - client `DAMAGE` messages contribute clipped surface-relative rectangles
  - compositor-owned keyboard/pointer status changes contribute their own
    status-bar rectangles
  - overlap is handled conservatively by repainting the union box and then
    re-blitting visible surfaces in stable slot order
- Off-screen damage is clipped to both surface bounds and the framebuffer
  bounds before presentation.
- Committed surfaces remain visible until they are replaced by a later commit,
  explicitly destroyed, or disconnected.
- Repeated client `DAMAGE` notifications are drained and conservatively
  coalesced into one accumulated redraw box per poll cycle.

**Current Input Model**
- The compositor polls `/dev/input/event0` and `/dev/input/event1`.
- The headless QEMU smoke still relies on the kernel's synthetic keyboard and
  pointer injection hook.
- Pointer motion now performs topmost-surface hit-testing and updates hover
  pointer focus independently of keyboard focus.
- Pointer button presses keep the current hover target as keyboard focus using
  a baseline click-to-focus rule.
- The compositor currently routes pointer coordinates/buttons and key
  press/release events to one focused input-aware client surface.
- Compositor-owned keyboard/pointer status indicators still update in parallel
  with routed client input.
- Focused-surface destroy/disconnect immediately clears any affected focus
  slots before the scene is redrawn.

**Current Test Coverage**
- `cargo test -p kernel` now covers a host-side protocol/scene-state model for:
  - fixed-struct header parsing and unknown-message rejection
  - client/surface object-id validation
  - `create -> attach -> commit -> destroy` transitions
  - clipped `DAMAGE` validation
  - dirty-rectangle accumulation and presentation coalescing
  - stable slot-order stacking for committed surfaces
  - pointer-hit focus assignment
  - focus cleanup on destroy/disconnect
- `tests/musl_ext2_gui_smoke_setup.sh` statically checks the staged launcher,
  compositor smoke helper, and QEMU runner wiring.
- `tests/musl_ext2_gui_smoke.sh` runs the full QEMU smoke and asserts:
  - compositor readiness
  - explicit surface creation for committed clients
  - transient client create/attach/commit/destroy cleanup
  - malformed client cleanup after missing-fd attach
  - malformed client cleanup after invalid buffer-fd attach
  - two shared-buffer attach events
  - two client commit markers
  - two compositor-observed client damage markers after first commit
  - transient client stale `DAMAGE` rejection after `DESTROY`
  - compositor-assigned hover pointer focus before click activation
  - compositor-assigned keyboard focus on pointer button input
  - compositor-routed pointer and keyboard delivery to the focused client
  - focused client acknowledgement of routed pointer and keyboard messages
  - compositor-side keyboard consumption
  - compositor-side pointer consumption
  - compositor pass marker
  - console fallback after compositor exit
- `tests/musl_ext2_gui_demo_setup.sh` statically checks the separate demo
  launcher, staging script, and Makefile runner wiring.

Limitations / Simplifications
- The current transport is inherited `socketpair()` setup from the smoke
  launcher, not a long-lived pathname listener/socket directory.
- Clients still use one shared-memory buffer each, and the current smoke only
  exercises one post-commit damage update per stable client; there is no
  buffer release event or sustained repaint stream yet.
- The compositor is single-threaded and smoke-oriented; it is not yet packaged
  as a reusable userspace service binary/library split.
- Surface lifecycle is intentionally narrow: one top-level surface per client,
  fixed placement, fixed z-order, and one explicit create/destroy object path.
- Client rejection/disconnect handling is currently slot-oriented rather than
  object-oriented: the compositor drops the whole client slot and redraws.
- Dirty tracking is intentionally conservative: it coalesces to one bounding
  rectangle instead of keeping a disjoint region list.
- Peer close handling now drains queued readable messages before honoring the
  hangup, but there is still no general backpressure or fairness policy beyond
  the current smoke-oriented loop.
- Input routing is intentionally minimal: hover pointer focus plus click-to-
  focus keyboard activation and direct event delivery to one focused top-level
  surface, with no seat abstraction,
  grabs, text-input protocol, or interactive move/resize policy yet.
- Window management is intentionally minimal: fixed positions, fixed z-order,
  and no interactive move/resize policy.
- Headless regression still depends on synthetic kernel input because the QEMU
  harness does not yet inject host keyboard/pointer events directly.
