# GUI and Wayland Compatibility Plan (Living Document)

Last updated: 2026-04-12
Status: Draft roadmap, Phase 1 implementation complete; Phase 2 implementation and required tests complete; Phase 3 userspace device ABI now includes `/dev/fb0` metadata + mmap plus pollable `/dev/input/event{0,1}` queues, kernel display ownership now supports explicit console-first vs compositor-first handoff, compositor-first userspace init has dedicated ext2 smokes for framebuffer/input delivery plus post-exit console fallback, and Phase 4 has started with a compositor-core smoke that renders two independent surfaces, tracks focus, and routes injected keyboard/pointer input over the compositor-first profile

## Purpose

This document tracks the current GUI/display/input implementation and a staged
plan to reach practical Wayland compatibility over time.

This is a living plan. Update it as code lands, assumptions change, or phase
scope is adjusted.

## Goals

- Move from kernel-rendered framebuffer console UX to a userspace GUI stack.
- Keep kernel responsibilities minimal and protocol-agnostic where practical.
- Build incrementally with testable milestones instead of a single large jump.
- Reach "close to Wayland compatibility" in stable, measurable phases.

## Non-Goals (Near Term)

- Full Linux graphics stack parity in one release.
- Immediate GPU acceleration / full DRM/KMS parity.
- Running complex desktop environments early (GNOME/KDE class targets).

## Current State Summary

## Display Path (Today)

- Bootloader selects one VESA mode and passes framebuffer metadata.
- Kernel maps a fixed framebuffer window (`16 MiB`) and assumes `32-bpp`.
- Rendering API is minimal (`set_pixel`, `clear`), no mode switching.
- No double buffering, no damage tracking, no compositor abstraction.

## Console/UI Path (Today)

- Kernel `fb_console` draws a styled terminal panel directly to framebuffer.
- Cursor blink and text rendering are in kernel.
- Mouse cursor is software-composited in kernel over framebuffer pixels.
- Main loop in `kmain()` ticks console and cursor directly.

## Input Path (Today)

- PS/2 keyboard set-1 path with baseline decode.
- PS/2 mouse relative packets plus optional USB tablet absolute mapping.
- Input still feeds the kernel TTY path, but it now also exposes `/dev/input/event0`
  (keyboard) and `/dev/input/event1` (pointer) for userspace consumers.
- Pollable evdev-style records are available to userspace, including
  `SYN_DROPPED` overflow signalling.

## Userspace/ABI Readiness (Today)

- PTY support is present (`/dev/ptmx`, `/dev/pts/N`) with baseline polling.
- Networking sockets now include a baseline `AF_UNIX` local stream path for
  `socket()` + `socketpair()`, plus pathname `bind`/`listen`/`connect`/`accept`.
- Missing/partial pieces for Wayland-style IPC:
  - `sendmsg`/`recvmsg` now include baseline `SCM_RIGHTS` fd passing over `AF_UNIX` streams.
  - ancillary coverage is still partial (`SCM_CREDENTIALS` and full Linux corner semantics pending).
  - `epoll` is now available as a baseline level-triggered implementation (`epoll_create1`/`epoll_ctl`/`epoll_wait`), with advanced flags/variants still pending.

## Architecture Decision

Implement GUI protocol and compositor logic in userspace, not in kernel.

Kernel should provide:

- Display and input device primitives.
- Local IPC primitives required by GUI protocols.
- Memory/buffer-sharing primitives.
- Scheduling/polling primitives robust enough for event-driven compositor loops.

Userspace should provide:

- Compositor and window/surface management.
- Protocol server (initially custom, then Wayland protocol).
- Input focus/policy.
- Rendering/composition policy.

## Phased Implementation Plan

## Phase 0: Foundation Cleanup and Baselines

Scope:

- Reconcile framebuffer documentation/constants and ensure docs match code.
- Introduce GUI roadmap tracking tags/status markers in this file.
- Define feature-gating strategy for early GUI components.
- Add baseline GUI readiness test checklist (manual + automated).

Acceptance criteria:

- This document has owners/status per phase.
- Docs and current behavior are consistent for framebuffer mode/mapping.
- Build remains clean for default non-GUI profile.

Limitations / Simplifications:

- No behavior change yet, planning/doc alignment only.

## Phase 1: Kernel IPC Primitives for Local GUI Protocols

Scope:

- Add `AF_UNIX` socket support (`SOCK_STREAM` first, then datagram/seqpacket if needed).
- Implement `socketpair`.
- Implement `sendmsg` and extend `recvmsg` for ancillary data.
- Implement `SCM_RIGHTS` fd passing path.
- Add/confirm nonblocking behavior consistency for socket IPC paths.

Detailed checklist:

### 1. ABI and syscall surface

- [x] Add baseline ABI constants needed for local socket create/pair paths (`AF_UNIX`).
- [x] Complete ancillary-message ABI coverage for fd passing (`cmsghdr`, `SCM_RIGHTS`).
- [x] Extend syscall argument parsing to support `sockaddr_un` length and path handling.
- [x] Define behavior for unsupported combinations (clear `-EAFNOSUPPORT`, `-EINVAL`,
  `-EPROTONOSUPPORT`, `-ENOSYS` where applicable).

### 2. Core `AF_UNIX` socket support

- [x] Add Unix-domain socket state and endpoint model in kernel socket layer.
- [x] Implement `socket(AF_UNIX, SOCK_STREAM, 0)` create path.
- [x] Implement `bind` for pathname sockets.
- [x] Implement `listen` and `accept/accept4` for `AF_UNIX` stream sockets.
- [x] Implement `connect` for `AF_UNIX` stream sockets.
- [x] Implement baseline local stream data path (`send`/`recv`, `read`/`write` route via socket fd).
- [x] Implement baseline poll readiness/hangup behavior for local stream sockets.
- [x] Implement `shutdown` and close semantics for peer disconnect/hangup reporting.
- [x] Add pathname lifecycle and conflict handling for bound socket paths in VFS namespace.

### 3. `socketpair`

- [x] Implement `socketpair(AF_UNIX, SOCK_STREAM, 0, sv)` with connected peer endpoints.
- [x] Ensure correct fd allocation, rollback-on-failure, and reference counting.
- [x] Validate nonblocking and close-on-exec flags on both returned descriptors.

### 4. `sendmsg`/`recvmsg` baseline

- [x] Implement `sendmsg` data path over `AF_UNIX` stream sockets with iovec support.
- [x] Extend `recvmsg` to report flags/lengths consistently for local socket messages.
- [x] Enforce robust bounds checking for user-provided iovec and control buffers.
- [x] Return correct truncation/error semantics for short control/data buffers.

### 5. `SCM_RIGHTS` fd passing

- [x] Implement control-message parser for `SOL_SOCKET` + `SCM_RIGHTS`.
- [x] Implement fd duplication/transfer semantics into receiver fd table.
- [x] Enforce validation rules for invalid/stale/non-transferable descriptors.
- [x] Ensure lifetime correctness: transferred fds remain valid even if sender closes.
- [x] Add limits/guards for max fds per message and malformed cmsg layouts.

### 6. Nonblocking and polling behavior

- [x] Ensure `O_NONBLOCK` behavior is consistent across `connect`, `accept`,
  `sendmsg`, `recvmsg`, `sendto`, `recvfrom`.
- [x] Expose baseline local-stream readiness through existing `poll`/`ppoll`/`select` paths.
- [x] Verify baseline `POLLIN`/`POLLOUT`/`POLLHUP` transitions on peer close and
  buffer state changes.

### 7. Tests (required before Phase 1 close)

- [x] Add host kernel unit tests for local socketpair send/recv/hangup queue behavior.
- [x] Add host kernel unit tests for pathname `AF_UNIX` bind/listen/connect/accept flow.
- [x] Add host unit tests for sockaddr/cmsg parsing and validation edge cases.
- [x] Add kernel-level tests for socketpair + sendmsg/recvmsg fd-passing mechanics.
- [x] Add QEMU smoke test:
  userspace process A sends one or more fds to process B via `AF_UNIX`,
  process B uses received fds successfully.
- [x] Add negative-path smoke coverage:
  malformed cmsg, invalid fd, closed sender fd after transfer, buffer truncation.
- [x] Keep existing networking/TTY smokes green (`make test` baseline unaffected).

### 8. Documentation and tracking

- [x] Update `docs/syscall.md` with new syscall behavior and limitations.
- [x] Update `docs/ipc.md` and/or socket-related docs with Unix socket and cmsg model.
- [x] Update `docs/PORTING_PLAN.md` Phase/Roadmap status once merged.
- [x] Record known gaps kept for later phase (for example `SCM_CREDENTIALS`).

Phase 1 exit checklist:

- [x] `AF_UNIX` stream sockets usable for compositor/client local IPC.
- [x] `socketpair` functional.
- [x] `sendmsg` implemented for data + `SCM_RIGHTS`.
- [x] `recvmsg` supports ancillary fd receive path.
- [x] Automated tests cover happy-path and key error-path behavior.

Acceptance criteria:

- Userspace can establish local socket sessions via `AF_UNIX`.
- FDs can be passed between processes via ancillary messages.
- Host and QEMU smoke tests cover success/failure paths for fd passing.

Limitations / Simplifications:

- Credential passing (`SCM_CREDENTIALS`) can be deferred.
- Security policy can remain minimal initially (single-user assumption).

## Phase 2: Shared Buffer and Event Loop Primitives

Scope:

- Add stable shared-buffer strategy for GUI surfaces:
  - Prefer `memfd_create`-like semantics, or robust tmpfs-backed equivalent.
- Ensure `mmap` + writable shared mappings are reliable for compositor/client flow.
- Implement `epoll` (or define strong temporary constraints using `poll`).
- Tighten `fcntl`/flags behavior needed by compositor-style event loops.

Detailed checklist:

### 1. ABI and syscall surface

- [x] Add ABI constants and syscall wiring for shared buffer primitives (`memfd_create`-style path or equivalent).
- [x] Confirm fd/seal/flag constants exposed to userspace headers are coherent with kernel behavior.
- [x] Define and document precise error returns for unsupported flag combinations and invalid arguments (`-EINVAL`, `-ENOSYS`, `-EMFILE`, `-ENFILE`).

### 2. Shared buffer object creation and lifetime

- [x] Implement anonymous shared-buffer fd creation path suitable for compositor/client exchange.
- [x] Ensure created buffer objects are VFS-visible in a controlled way (anonymous or tmpfs-backed as designed).
- [x] Enforce robust reference/lifetime behavior across `dup`, `fork`, `exec`, and close races.
- [x] Add explicit size validation and upper bounds to prevent oversized allocation abuse.

### 3. `mmap` + writable shared mapping reliability

- [x] Support mapping shared-buffer fds with writable shared mappings (`MAP_SHARED` + `PROT_WRITE`) across multiple processes.
- [x] Validate page alignment, offset, and length semantics with clear errors for invalid mappings.
- [x] Ensure dirty writes from one process are visible to peer mappings predictably.
- [x] Verify unmap/remap and partial unmap behavior does not leak stale mappings or corrupt bookkeeping.

### 4. `epoll` core implementation

- [x] Implement `epoll_create1`.
- [x] Implement `epoll_ctl` (`ADD`, `MOD`, `DEL`) with correct duplicate/add-remove semantics.
- [x] Implement `epoll_wait` with timeout behavior, signal interruption semantics, and readiness delivery.
- [x] Support baseline level-triggered behavior for sockets, pipes, PTYs, and shared-buffer signaling fds if used.
- [x] Handle close/hangup/error transitions correctly so dead peers do not wedge compositor loops.

### 5. Event-loop flag and descriptor semantics

- [x] Tighten `fcntl(F_GETFL/F_SETFL)` handling for `O_NONBLOCK` consistency across sockets, pipes, and PTYs.
- [x] Ensure `FD_CLOEXEC` behavior is reliable for event-loop descriptors and transferred fds.
- [x] Verify `accept4`/`socket`/`socketpair` flag behavior remains consistent with Phase 1 expectations.
- [x] Confirm `poll`/`ppoll`/`select` and `epoll` readiness observations stay coherent for the same fd states.

### 6. Resource limits and hardening

- [x] Add accounting/limits for number of shared buffers, mapped pages, and epoll watch entries per process.
- [x] Add defensive checks for malformed or adversarial watcher registration patterns.
- [x] Ensure kernel teardown paths free epoll interest records and shared-buffer mappings without leaks.

### 7. Tests (required before Phase 2 close)

- [x] Add host unit tests for shared-buffer create/resize/map/unmap lifecycle and error paths.
- [x] Add host unit tests for multi-process visibility semantics on writable shared mappings.
- [x] Add host unit tests for `epoll_ctl` add/modify/delete and duplicate registration edge cases.
- [x] Add host unit tests for `epoll_wait` timeout, wakeup, hangup, and close-race behavior.
- [x] Add QEMU smoke test: compositor-like process waits on multiple client IPC fds via `epoll` and handles wakeups without busy loop.
- [x] Add QEMU smoke test: shared pixel buffer passed over `AF_UNIX` with `SCM_RIGHTS`, mapped by receiver, and validated for write/read coherence.
- [x] Keep existing `make test` baseline green (TTY/networking/Phase 1 IPC regressions blocked).

### 8. Documentation and tracking

- [x] Update `docs/syscall.md` for new shared-buffer and `epoll` syscall behavior and limitations.
- [x] Update `docs/ipc.md` and memory/VM docs for fd-backed shared mapping model and lifecycle semantics.
- [x] Update `docs/PORTING_PLAN.md` with Phase 2 status and explicit deferred items.
- [x] Record Phase 2 known gaps (for example advanced sealing semantics, edge-triggered `epoll` deferral if not yet implemented).

Phase 2 exit checklist:

- [x] Shared-buffer fd strategy implemented and usable by independent processes.
- [x] Writable shared mappings are stable under repeated map/unmap churn.
- [x] `epoll` supports compositor-style multi-fd wait loops without busy polling.
- [x] Tests cover happy-path and key error-path behavior for buffers + event loops.

Acceptance criteria:

- Client and compositor processes can share pixel buffers safely.
- Event loop scales beyond trivial fd counts without busy polling.
- Stress tests verify repeated map/unmap and fd lifecycle safety.

Limitations / Simplifications:

- Advanced memory sealing semantics can be deferred.
- Early implementation may cap buffer size/count with explicit limits.

## Phase 3: Display and Input Device Interfaces for Userspace

Scope:

- Introduce userspace-facing display device interface (for example `/dev/fb0`).
- Introduce userspace-facing input event interfaces (keyboard/pointer events).
- Move direct kernel cursor composition out of the critical path for GUI mode.
- Keep framebuffer console path available as fallback mode.

Detailed checklist:

### 1. GUI mode gating and boot profile selection

- [x] Define explicit mode selection for console-first vs compositor-first boot
  (currently via kernel feature profile; boot-arg path still pending).
- [x] Keep default boot path console-first when GUI mode is not requested.
- [x] Define failure policy: if compositor launch fails or compositor-owned init exits,
  return to visible console fallback without reboot.

### 2. Display device ABI surface

- [x] Define `/dev/fb0` userspace contract (width/height/stride/bpp/format/map size).
- [x] Implement or confirm metadata query path (`ioctl` and/or documented fixed contract)
  with clear errors for invalid arguments.
- [x] Ensure framebuffer `mmap` write visibility is stable for compositor redraw loops.
- [x] Define baseline permission/ownership model for framebuffer access in single-user mode.

### 3. Input event device ABI

- [x] Introduce `/dev/input/eventN`-style devices (or documented equivalent) for keyboard and pointer.
- [x] Define event record format and timestamp policy for baseline compatibility.
- [x] Route existing PS/2 keyboard and mouse paths into per-device event queues.
- [x] Support nonblocking reads plus coherent readiness via `poll`/`ppoll`/`select`/`epoll`.
- [x] Define queue overflow/drop semantics and expose them predictably to userspace.

### 4. Cursor ownership transition

- [x] Disable kernel software cursor composition while GUI mode is active.
- [x] Preserve existing kernel cursor/console behavior in console mode.
- [x] Ensure transitions do not leave stale cursor artifacts or corrupt framebuffer state.

### 5. Console fallback and coexistence rules

- [x] Preserve existing shell/TTY behavior when GUI mode is disabled.
- [x] Keep panic/oops output readable and available regardless of GUI mode setting.
- [x] Document single display-owner policy for Phase 3 baseline (console or compositor, not both).

### 6. Userspace bring-up integration

- [x] Add compositor-first boot/service profile wiring in userspace init flow.
- [x] Ensure compositor process receives required device access and startup environment.
- [x] Define behavior for compositor exit/crash (fallback console handoff policy).

### 7. Tests (required before Phase 3 close)

- [x] Add host unit tests for input event queue ordering, overflow, and error paths.
- [x] Add host unit tests for framebuffer metadata/query behavior and invalid argument handling.
- [x] Add QEMU smoke test: console-first profile boots and keeps shell workflows intact.
- [x] Add QEMU smoke test: compositor-first profile receives keyboard/pointer events and updates framebuffer.
- [x] Add QEMU smoke test: compositor crash/exit returns system to visible fallback console behavior.
- [x] Keep `make test` baseline green for both default and GUI-enabled test profiles.

### 8. Documentation and tracking

- [x] Update `docs/syscall.md` and device docs with display/input ABI behavior and limitations.
- [x] Add or refresh subsystem docs in `docs/` for framebuffer device and input event model.
- [x] Update `README.md` with GUI profile boot/test instructions.
- [x] Update `docs/PORTING_PLAN.md` and this roadmap table with Phase 3 progress/deferred items.

Phase 3 exit checklist:

- [x] Userspace compositor can map display output and render without kernel UI loop ownership.
- [x] Userspace receives stable, pollable keyboard/pointer event streams in GUI mode.
- [x] Console-first workflows remain intact when GUI mode is disabled.
- [x] QEMU smokes cover console-first, compositor-first, and compositor-failure fallback behavior.

Acceptance criteria:

- Userspace compositor can read input events and draw output without kernel UI loop.
- Existing shell/console workflows still work when GUI mode is disabled.
- QEMU smoke can boot into both console-first and compositor-first profiles.

Limitations / Simplifications:
- Explicit GUI mode selection currently uses a kernel feature/build profile (`compositor-first`), not a runtime boot arg.
- The current fallback policy covers init-launch failure before userspace takeover, compositor-owned init exit/crash after takeover, plus fatal panic/exception paths.
- The compositor-first smoke validates environment/device wiring, framebuffer mmap/write, and injected keyboard/pointer delivery, but it remains a single-client smoke rather than a compositor lifecycle or crash-recovery test.
- Console fallback redraw is fresh-state only; there is no preserved compositor-era backbuffer or scrollback restore.

- Initial display path can remain fixed-mode framebuffer (no mode setting).
- Input device model can start with core events only (no advanced HID features).
- Single-display and single-seat assumptions are acceptable in this phase.
- Device hotplug and multi-head routing can be deferred.

## Phase 4: Userspace Compositor MVP (Protocol-Agnostic)

Scope:

- Build a minimal userspace compositor process.
- Start with a small internal protocol (`peet-gui`) to validate architecture:
  - surface creation/destruction
  - buffer attach/commit
  - damage notification
  - input dispatch and focus
- Implement software composition with dirty region tracking.

Current slice landed:

- A dedicated compositor-first smoke profile now boots `peet_gui_compositor_smoke`.
- The smoke creates two overlapping userspace surfaces, composites them onto `/dev/fb0`,
  validates initial top-surface ordering, highlights focus, validates a damage-limited partial
  redraw/commit on one surface, raises the focused surface after injected pointer activity so
  keyboard delivery follows the new frontmost surface, and explicitly detaches one client surface
  after input delivery so the compositor proves it can redraw and continue with the remaining surface.
- `make run-peet-gui-smoke` exercises this path end to end in QEMU.
- A separate compositor-first shell demo now boots `peet_shell_demo` for manual visual iteration:
  it draws desktop chrome, a launcher strip, terminal/files/inspector panels, a software pointer
  cursor, keyboard focus changes, theme rotation, and terminal-style command feedback directly on
  `/dev/fb0`. The current slice now composes those panels from independent offscreen surfaces and
  raises the focused panel in z-order. The newest slice moves the `FILES` panel into a separate
  forked userspace client over an `AF_UNIX socketpair` control path, with `make run-peet-shell-demo`
  for interactive use and a small smoke runner to verify the shell reaches its ready state and reacts
  to injected keyboard input.

Acceptance criteria:

- Multiple test clients can render independent surfaces.
- Pointer and keyboard focus model is functional.
- Compositor survives client crash/disconnect without kernel instability.

Limitations / Simplifications:

- The current Phase 4 smoke is a compositor-core harness, not yet a true multi-process
  `peet-gui` server/client transport. Surface storage is local userspace memory rather than
  shared-buffer fd handoff, and protocol messages are currently exercised through the
  in-process queue harness rather than kernel-backed IPC.
- The visual shell scaffold is still only a partial split: `FILES` now runs out-of-process over a
  real message path, but `TERMINAL` and `INSPECTOR` are still local to the compositor process, and
  there is not yet shared-buffer handoff between processes.
- Decorations/window-management policy can be minimal.
- No hardware acceleration requirement.

## Phase 5: Wayland Core Protocol Bring-Up

Scope:

- Add Wayland wire protocol server support in userspace.
- Implement initial required globals/interfaces:
  - `wl_display`
  - `wl_registry`
  - `wl_compositor`
  - `wl_shm`
  - `wl_surface`
  - `wl_buffer`
  - `wl_callback`
- Add `xdg_wm_base` / `xdg_surface` / `xdg_toplevel` minimal support.
- Keep internal protocol optional during transition.

Acceptance criteria:

- A minimal `wl_shm` client can connect, allocate buffers, render, and present.
- Core roundtrip/registry behavior is stable across reconnect cycles.
- Protocol compliance tests pass for implemented subset.

Limitations / Simplifications:

- No EGL/OpenGL path required in this phase.
- Protocol subset only; unsupported interfaces must fail clearly and safely.

## Phase 6: "Close to Wayland Compatibility" Target

Scope:

- Expand protocol coverage needed by common lightweight Wayland clients:
  - `wl_seat` (keyboard/pointer)
  - `wl_data_device` baseline
  - better `xdg-shell` behavior and state transitions
- Improve compositor robustness:
  - frame pacing
  - better damage tracking
  - resource cleanup hardening
- Add compatibility-focused integration matrix for representative apps.

Acceptance criteria:

- Multiple independent Wayland test clients run without protocol errors.
- Long-running compositor session remains stable under connect/disconnect churn.
- Input focus and surface lifecycle behavior are predictable and documented.

Limitations / Simplifications:

- Complex desktop-environment features may still be out of scope.
- Performance tuning and acceleration can remain iterative.

## Cross-Phase Testing Strategy

- Host unit tests:
  - IPC encoding/decoding
  - ancillary message parsing and fd transfer validation
  - protocol message parser/serializer behavior
- QEMU smoke tests:
  - compositor boot smoke
  - multi-client connect/render smoke
  - input-to-surface interaction smoke
  - client crash/restart resilience
- Regression gates:
  - preserve existing shell/TTY smoke coverage
  - preserve default non-GUI build behavior

## Risks and Dependencies

- Runtime maturity gaps can block GUI workloads:
  - thread semantics
  - signal completeness
  - memory reclaim/paging behavior under GUI pressure
- Device model evolution is required:
  - display and input interfaces currently console-centric
- Protocol compatibility requires strict message/ordering correctness.

## Tracking Template

Use this table for ongoing updates:

| Phase | Status | Owner | Last Updated | Notes |
| --- | --- | --- | --- | --- |
| 0 | Planned | TBD | 2026-02-28 | Doc and baseline alignment |
| 1 | Complete | TBD | 2026-04-12 | AF_UNIX + SCM_RIGHTS implementation, host coverage, negative-path tests, QEMU smokes, and `make test` regression coverage are green |
| 2 | Complete | TBD | 2026-04-12 | Shared-buffer lifecycle, writable shared `mmap`, baseline `epoll`, fd/flag semantics, hardening/resource caps, host coverage, and QEMU compositor-loop/shared-buffer smokes are complete and green |
| 3 | Complete | TBD | 2026-04-12 | Device ABI includes `/dev/fb0` + `FBIOGETINFO` + shared framebuffer `mmap`, plus `/dev/input/event0` keyboard and `/dev/input/event1` pointer queues with nonblocking read + poll/select/epoll readiness + `SYN_DROPPED` overflow signalling. Kernel display ownership is explicitly gated (`compositor-first` profile), userspace init has compositor-first smokes for input/rendering and post-exit fallback, a dedicated console-first shell smoke keeps default ownership intact, and `make test` now includes these paths cleanly |
| 4 | In Progress | TBD | 2026-04-12 | Initial compositor-core smoke landed: two overlapping userspace surfaces render to `/dev/fb0`, explicit attach/damage/commit/focus/key message flow drives focus/input routing, initial z-order plus raise-on-focus behavior are validated, a partial damage/commit redraw is validated, one client detach path is validated end to end, and `make run-peet-gui-smoke` covers the compositor-first path. A separate `make run-peet-shell-demo` visual shell scaffold now exercises desktop chrome, launcher/panel layout, terminal/files/inspector surfaces, focus raise, theme switching, and terminal-style feedback on the same compositor-first ABI; `FILES` is now a separate forked client over `AF_UNIX socketpair`; pending: shared-buffer handoff and broader multi-process `peet-gui` transport |
| 5 | Planned | TBD | 2026-02-28 | Wayland core subset |
| 6 | Planned | TBD | 2026-02-28 | Near-compat target |

## Update Rules

- Update this file when phase scope/status changes.
- Record concrete blockers and changed assumptions in the phase notes.
- Keep acceptance criteria aligned with available tests.
- Keep "Limitations / Simplifications" current in each phase section.
