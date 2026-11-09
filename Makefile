# ===========================================================================
# PeetOS Rust Makefile
# ===========================================================================

ASM      := nasm
LD       := ld
OBJCOPY  := objcopy
QEMU     := qemu-system-x86_64
# Number of virtual CPUs for the `make run*` targets (e.g. `make run QEMU_SMP=4`).
QEMU_SMP ?= 1

BUILD_DIR := build
IMAGE     := $(BUILD_DIR)/peetos.img

BOOT_BIN   := $(BUILD_DIR)/boot.bin
STAGE2_BIN := $(BUILD_DIR)/stage2.bin
ENTRY_OBJ  := $(BUILD_DIR)/entry.o
USER_OBJ   := $(BUILD_DIR)/user_hello.o
USER_DYN_OBJ := $(BUILD_DIR)/user_hello_dyn.o
USER_INTERP_OBJ := $(BUILD_DIR)/user_ld_peetos.o
USER_MUSL_SMOKE_ELF := user/musl_echo.elf
USER_MUSL_EXT2_INIT_ELF := user/musl_execve_ext2.elf
USER_MUSL_EXT2_SMOKE_ELF := user/musl_execve_ext2_smoke.elf
USER_MUSL_EXT2_PING_SMOKE_ELF := user/musl_execve_ext2_ping_smoke.elf
USER_MUSL_EXT2_UNIX_RIGHTS_SMOKE_ELF := user/musl_execve_ext2_unix_rights_smoke.elf
USER_MUSL_EXT2_EPOLL_SMOKE_ELF := user/musl_execve_ext2_epoll_smoke.elf
USER_MUSL_EXT2_SHARED_BUFFER_SMOKE_ELF := user/musl_execve_ext2_shared_buffer_smoke.elf
USER_MUSL_EXT2_CONSOLE_FIRST_SMOKE_ELF := user/musl_execve_ext2_console_first_smoke.elf
USER_MUSL_EXT2_GUI_SMOKE_ELF := user/musl_execve_ext2_gui_smoke.elf
USER_MUSL_EXT2_GUI_DEMO_ELF := user/musl_execve_ext2_gui_demo.elf
USER_MUSL_EXT2_PEET_GUI_SMOKE_ELF := user/musl_execve_ext2_peet_gui_smoke.elf
USER_MUSL_EXT2_PEET_SHELL_DEMO_ELF := user/musl_execve_ext2_peet_shell_demo.elf
USER_MUSL_EXT2_QUICKJS_SMOKE_ELF := user/musl_execve_ext2_quickjs_smoke.elf
USER_MUSL_EXT2_MICROPYTHON_SMOKE_ELF := user/musl_execve_ext2_micropython_smoke.elf
USER_MUSL_EXT2_DROPBEAR_SMOKE_ELF := user/musl_execve_ext2_dropbear_smoke.elf
USER_MUSL_EXT2_FPU_SMOKE_ELF := user/musl_execve_ext2_fpu_smoke.elf
USER_MUSL_EXT2_PROC_SMOKE_ELF := user/musl_execve_ext2_proc_smoke.elf
USER_MUSL_EXT2_WAYLAND_SMOKE_ELF := user/musl_execve_ext2_wayland_smoke.elf
USER_MUSL_EXT2_WLCOMP_SMOKE_ELF := user/musl_execve_ext2_wlcomp_smoke.elf
USER_MUSL_EXT2_WAYLAND_DEMO_ELF := user/musl_execve_ext2_wayland_demo.elf
USER_MUSL_EXT2_DYNLINK_SMOKE_ELF := user/musl_execve_ext2_dynlink_smoke.elf
USER_MUSL_EXT2_DRM_SMOKE_ELF := user/musl_execve_ext2_drm_smoke.elf
USER_MUSL_EXT2_TEXT_SMOKE_ELF := user/musl_execve_ext2_text_smoke.elf
USER_MUSL_EXT2_FOOT_SMOKE_ELF := user/musl_execve_ext2_foot_smoke.elf
USER_MUSL_EXT2_FOOT_DEMO_ELF := user/musl_execve_ext2_foot_demo.elf
USER_MUSL_EXT2_CXX_SMOKE_ELF := user/musl_execve_ext2_cxx_smoke.elf
USER_MUSL_EXT2_PANGO_SMOKE_ELF := user/musl_execve_ext2_pango_smoke.elf
USER_MUSL_EXT2_GTK_SMOKE_ELF := user/musl_execve_ext2_gtk_smoke.elf
USER_MUSL_EXT2_WAYLAND_TESTS_ELF := user/musl_execve_ext2_wayland_tests.elf
USER_MUSL_EXT3_JOURNAL_SMOKE_ELF := user/musl_execve_ext3_journal_smoke.elf
USER_JOURNAL_SMOKE_HELPER_BIN := user/journal_smoke_helper.elf
USER_UNIX_RIGHTS_SMOKE_BIN := user/unix_rights_smoke.elf
USER_EPOLL_COMPOSITOR_SMOKE_BIN := user/epoll_compositor_smoke.elf
USER_SHARED_BUFFER_RIGHTS_SMOKE_BIN := user/shared_buffer_rights_smoke.elf
USER_GUI_COMPOSITOR_SMOKE_BIN := user/gui_compositor_smoke.elf
USER_PEET_GUI_COMPOSITOR_SMOKE_BIN := user/peet_gui_compositor_smoke.elf
USER_PEET_SHELL_DEMO_BIN := user/peet_shell_demo.elf
USER_FPU_CONTEXT_SMOKE_BIN := user/fpu_context_smoke.elf
USER_PROC_MODEL_SMOKE_BIN := user/proc_model_smoke.elf
USER_WAYLAND_SMOKE_BIN := user/wayland_smoke.elf
USER_PEETWL_BIN := user/peetwl.elf
USER_WL_DEMO_BIN := user/wl_demo.elf
USER_DYNLINK_SMOKE_BIN := user/dynlink_smoke.elf
USER_DRM_SMOKE_BIN := user/drm_smoke.elf
USER_TEXT_SMOKE_BIN := user/text_smoke.elf
USER_CXX_SMOKE_BIN := user/cxx_smoke.elf
USER_PANGO_SMOKE_BIN := user/pango_smoke.elf
USER_GTK_SMOKE_BIN := user/gtk_smoke.elf
# PeetOS cross g++ and C++ runtime (toolchain/build-toolchain.sh).
CXX_TOOLCHAIN_STAMP := toolchain/out/bin/x86_64-linux-musl-g++
# Staged sysroot of the software rendering stack (scripts/build_render_stack.sh).
RENDER_STAGE = $(WAYLAND_OUT)/stage
RENDER_STAMP = $(RENDER_STAGE)/usr/lib/libpixman-1.so.0
FOOT_STAMP = $(RENDER_STAGE)/usr/bin/foot
# zlib, libpng, GLib, cairo, pango (scripts/build_cairo_pango.sh).
PANGO_STAMP = $(RENDER_STAGE)/usr/lib/libpangocairo-1.0.so.0
# libepoxy, ATK, gdk-pixbuf, GTK 3 (scripts/build_gtk.sh).
GTK_STAMP = $(RENDER_STAGE)/usr/lib/libgtk-3.so.0
MUSL_SHARED_LIBC := musl/shared/lib/libc.so
WAYLAND_OUT := wayland/out
# Shared builds (soname files), linked by the dynamic peetwl/wl_demo and
# staged into the ext2 image's /usr/lib.
WAYLAND_SHLIBS := $(WAYLAND_OUT)/lib/libwayland-server.so.0 $(WAYLAND_OUT)/lib/libwayland-client.so.0 $(WAYLAND_OUT)/lib/libxkbcommon.so.0 $(WAYLAND_OUT)/lib/libffi.so.8 $(WAYLAND_OUT)/lib/libdrm.so.2
WAYLAND_LIBS := $(WAYLAND_OUT)/lib/libwayland-server.a $(WAYLAND_OUT)/lib/libwayland-client.a $(WAYLAND_OUT)/lib/libwlprotocols.a $(WAYLAND_OUT)/lib/libxkbcommon.a $(WAYLAND_OUT)/lib/libffi.a
# Upstream libwayland test suite binaries (scripts/build_wayland_tests.sh); TESTS is the stamp.
WAYLAND_TESTS_STAMP := $(WAYLAND_OUT)/tests/TESTS
USER_EXT2_MUSL_IMG := user/sample_ext2_musl.img
USER_QUICKJS_BIN := user/qjs
USER_MICROPYTHON_BIN := user/micropython
USER_DROPBEAR_BIN := user/dropbear
USER_DROPBEARKEY_BIN := user/dropbearkey
USER_DROPBEAR_HOSTKEY := user/dropbear_ed25519_host_key

KERNEL_ELF := $(BUILD_DIR)/kernel.elf
KERNEL_BIN := $(BUILD_DIR)/kernel.bin
USER_ELF   := user/hello.elf
USER_DYN_ELF := user/hello_dyn.elf
USER_INTERP_SO := user/ld-peetos.so
USER_EXT2_IMG := user/sample_ext2.img
USER_ASM   := user/hello.asm
USER_DYN_ASM := user/hello_dyn.asm
USER_INTERP_ASM := user/ld_peetos.asm
USER_EXT2_SCRIPT := scripts/build_ext2_sample.sh
USER_MUSL_SMOKE_SCRIPT := scripts/build_musl_echo.sh
USER_MUSL_DYN_SCRIPT := scripts/build_musl_dyn.sh
USER_EXT2_MUSL_SCRIPT := scripts/build_ext2_musl_execve.sh
USER_TOYBOX_SH_SCRIPT := scripts/build_toybox_sh.sh
USER_TOYBOX_CORE_SCRIPT := scripts/build_toybox_core.sh
USER_EXT2_MUSL_OPTIONAL_DEPS :=

PROFILE ?= release
ifeq ($(PROFILE),release)
CARGO_FLAGS := --release
else
CARGO_FLAGS :=
endif

SECTOR_SIZE := 512
STAGE2_SECTORS := 16
DISK_SECTORS := 20480

BOOT_STAGE1_SRC := boot/stage1/boot.asm
BOOT_STAGE2_SRC := boot/stage2/stage2.asm
BOOT_CONSTANTS := boot/include/constants.inc
ENTRY_SRC := kernel/arch/x86_64/entry.asm

KERNEL_TARGET_DIR := target/x86_64-unknown-none/$(PROFILE)

KERNEL_FEATURES ?=
empty :=
space := $(empty) $(empty)
comma := ,
ifneq ($(strip $(KERNEL_FEATURES)),)
CARGO_FEATURES := --features $(subst $(space),$(comma),$(strip $(KERNEL_FEATURES)))
else
CARGO_FEATURES :=
endif

ALL_KERNEL_FEATURES := $(strip $(shell awk '\
	BEGIN { in_features = 0; } \
	/^\[features\]/ { in_features = 1; next; } \
	/^\[/ { if (in_features) exit; } \
	in_features && /^[A-Za-z0-9][A-Za-z0-9_-]*[[:space:]]*=/ { \
		split($$0, parts, "="); \
		gsub(/[[:space:]]/, "", parts[1]); \
		printf "%s ", parts[1]; \
	}' kernel/Cargo.toml))
RUN_ALL_FEATURES ?= $(filter-out default test-exit vga compositor-first sched-trace musl-ext2-foot-demo %-smoke,$(ALL_KERNEL_FEATURES))

ifneq ($(findstring musl-ext2-quickjs-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_QUICKJS_BIN) quickjs/test_hello.js quickjs/test_math.js
endif

ifneq ($(findstring musl-ext2-micropython-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_MICROPYTHON_BIN) micropython/test_hello.py micropython/test_math.py
endif

ifneq ($(findstring musl-ext2-unix-rights-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_UNIX_RIGHTS_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-epoll-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_EPOLL_COMPOSITOR_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-shared-buffer-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_SHARED_BUFFER_RIGHTS_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-gui-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_GUI_COMPOSITOR_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-gui-demo,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_GUI_COMPOSITOR_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-peet-gui-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEET_GUI_COMPOSITOR_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-peet-shell-demo,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEET_SHELL_DEMO_BIN)
endif

ifneq ($(findstring musl-ext2-dropbear-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN)
endif

ifneq ($(findstring musl-ext2-fpu-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_FPU_CONTEXT_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-proc-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PROC_MODEL_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-wayland-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_WAYLAND_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-wlcomp-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN)
endif

ifneq ($(findstring musl-ext2-wayland-demo,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN)
endif

ifneq ($(findstring musl-ext2-dynlink-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_DYNLINK_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-drm-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_DRM_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-text-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_TEXT_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-gtk-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEETWL_BIN) $(USER_GTK_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-pango-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PANGO_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-cxx-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_CXX_SMOKE_BIN)
endif

ifneq ($(findstring musl-ext2-foot-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(FOOT_STAMP)
endif

ifneq ($(findstring musl-ext2-foot-demo,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_PEETWL_BIN) $(FOOT_STAMP)
endif

ifneq ($(findstring musl-ext2-wayland-tests,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(WAYLAND_TESTS_STAMP)
endif

ifneq ($(findstring musl-ext3-journal-smoke,$(KERNEL_FEATURES)),)
USER_EXT2_MUSL_OPTIONAL_DEPS += $(USER_JOURNAL_SMOKE_HELPER_BIN)
endif

.PHONY: all image bootloader kernel user user-elf user-musl-smoke user-musl-ext2-exec user-musl-ext2-cmd-smoke user-musl-ext2-ping-smoke user-musl-ext2-unix-rights-smoke user-musl-ext2-epoll-smoke user-musl-ext2-gui-smoke user-musl-ext2-gui-demo user-musl-ext2-shared-buffer-smoke user-musl-ext2-console-first-smoke user-musl-ext2-peet-gui-smoke user-musl-ext2-peet-shell-demo user-musl-ext2-quickjs-smoke user-musl-ext2-micropython-smoke user-musl-ext2-dropbear-smoke user-musl-ext2-fpu-smoke user-musl-ext2-proc-smoke user-musl-ext2-wayland-smoke user-musl-ext2-wayland-tests user-musl-ext2-wlcomp-smoke user-musl-ext2-wayland-demo user-musl-ext2-dynlink-smoke user-musl-ext2-drm-smoke user-musl-ext2-text-smoke user-musl-ext2-foot-smoke user-musl-ext2-foot-demo user-musl-ext2-cxx-smoke user-musl-ext2-pango-smoke user-musl-ext2-gtk-smoke clean run run-current run-image run-tablet run-shell run-net-shell run-net run-net-udp-smoke run-net-tcp-smoke run-net-socket-smoke run-ping-smoke run-unix-rights-smoke run-epoll-smoke run-gui-smoke run-gui-demo run-shared-buffer-smoke run-console-first-smoke run-gui-fallback-smoke run-peet-gui-smoke run-peet-shell-demo run-peet-shell-demo-smoke run-quickjs-smoke run-micropython-smoke run-dropbear-smoke run-fpu-smoke run-proc-smoke run-wayland-smoke run-wayland-tests run-wlcomp-smoke run-wayland-demo run-dynlink-smoke run-drm-smoke run-text-smoke run-foot-smoke run-foot-demo run-cxx-smoke run-pango-smoke run-gtk-smoke user-musl-ext3-journal-smoke run-journal-smoke run-ext4-ro-smoke test ext2-image \
	musl-fetch musl-configure musl musl-clean musl-distclean musl-info \
	toybox-fetch toybox-configure toybox toybox-clean toybox-distclean toybox-info \
	quickjs-fetch quickjs quickjs-clean quickjs-distclean quickjs-info \
	micropython-fetch micropython micropython-clean micropython-distclean micropython-info \
	dropbear-fetch dropbear dropbear-clean dropbear-distclean dropbear-info run-shell-smoke

all: image

$(BUILD_DIR):
	@mkdir -p $(BUILD_DIR)

# ---------------------------------------------------------------------------
# Bootloader
# ---------------------------------------------------------------------------

$(BOOT_BIN): $(BOOT_STAGE1_SRC) $(BOOT_CONSTANTS) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $<

BOOT_STAGE2_INCLUDES := $(wildcard boot/stage2/*.asm) $(BOOT_CONSTANTS)
$(STAGE2_BIN): $(BOOT_STAGE2_INCLUDES) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $(BOOT_STAGE2_SRC)

bootloader: $(BOOT_BIN) $(STAGE2_BIN)

# ---------------------------------------------------------------------------
# Kernel
# ---------------------------------------------------------------------------

$(ENTRY_OBJ): $(ENTRY_SRC) | $(BUILD_DIR)
	$(ASM) -f elf64 -g -o $@ $<

$(USER_ELF): $(USER_ASM) | $(BUILD_DIR)
	$(ASM) -f elf64 -g -o $(USER_OBJ) $<
	$(LD) -nostdlib -static -z max-page-size=0x1000 -Ttext 0x400000 \
		-o $@ $(USER_OBJ)

$(USER_DYN_ELF): $(USER_DYN_ASM) | $(BUILD_DIR)
	$(ASM) -f elf64 -g -o $(USER_DYN_OBJ) $<
	$(LD) -nostdlib -pie -z max-page-size=0x1000 --dynamic-linker /lib/ld-peetos.so \
		-o $@ $(USER_DYN_OBJ)

$(USER_INTERP_SO): $(USER_INTERP_ASM) | $(BUILD_DIR)
	$(ASM) -f elf64 -g -o $(USER_INTERP_OBJ) $<
	$(LD) -nostdlib -shared -z max-page-size=0x1000 -e _start \
		-o $@ $(USER_INTERP_OBJ)

$(USER_EXT2_IMG):
	$(USER_EXT2_SCRIPT)

$(USER_MUSL_SMOKE_ELF): user/musl_echo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT)

$(USER_MUSL_EXT2_INIT_ELF): user/musl_execve_ext2.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_SMOKE_ELF): user/musl_execve_ext2_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_PING_SMOKE_ELF): user/musl_execve_ext2_ping_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_UNIX_RIGHTS_SMOKE_ELF): user/musl_execve_ext2_unix_rights_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_EPOLL_SMOKE_ELF): user/musl_execve_ext2_epoll_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_SHARED_BUFFER_SMOKE_ELF): user/musl_execve_ext2_shared_buffer_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_CONSOLE_FIRST_SMOKE_ELF): user/musl_execve_ext2_console_first_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_GUI_SMOKE_ELF): user/musl_execve_ext2_gui_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_GUI_DEMO_ELF): user/musl_execve_ext2_gui_demo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_PEET_GUI_SMOKE_ELF): user/musl_execve_ext2_peet_gui_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_PEET_SHELL_DEMO_ELF): user/musl_execve_ext2_peet_shell_demo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_QUICKJS_SMOKE_ELF): user/musl_execve_ext2_quickjs_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_MICROPYTHON_SMOKE_ELF): user/musl_execve_ext2_micropython_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_DROPBEAR_SMOKE_ELF): user/musl_execve_ext2_dropbear_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_FPU_SMOKE_ELF): user/musl_execve_ext2_fpu_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_FPU_CONTEXT_SMOKE_BIN): user/fpu_context_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_PROC_SMOKE_ELF): user/musl_execve_ext2_proc_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_PROC_MODEL_SMOKE_BIN): user/proc_model_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_WAYLAND_SMOKE_ELF): user/musl_execve_ext2_wayland_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT3_JOURNAL_SMOKE_ELF): user/musl_execve_ext3_journal_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_JOURNAL_SMOKE_HELPER_BIN): user/journal_smoke_helper.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(WAYLAND_LIBS) $(WAYLAND_SHLIBS): $(MUSL_SHARED_LIBC)
	$(MAKE) -C wayland

$(USER_WAYLAND_SMOKE_BIN): user/wayland_smoke.c $(USER_MUSL_SMOKE_SCRIPT) $(WAYLAND_LIBS)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@ -- -I$(WAYLAND_OUT)/include $(WAYLAND_LIBS) -lm

# peetwl and wl_demo link dynamically against the shared libwayland,
# libxkbcommon and libffi (protocol glue stays static).
$(USER_PEETWL_BIN): user/peetwl.c user/font8x8.h $(USER_MUSL_DYN_SCRIPT) $(WAYLAND_LIBS) $(WAYLAND_SHLIBS)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- -I$(WAYLAND_OUT)/include -I$(WAYLAND_OUT)/include/libdrm -L$(WAYLAND_OUT)/lib $(WAYLAND_OUT)/lib/libwlprotocols.a -lwayland-server -lxkbcommon -ldrm -lm

$(USER_WL_DEMO_BIN): user/wl_demo.c $(USER_MUSL_DYN_SCRIPT) $(WAYLAND_LIBS) $(WAYLAND_SHLIBS)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- -I$(WAYLAND_OUT)/include -L$(WAYLAND_OUT)/lib $(WAYLAND_OUT)/lib/libwlprotocols.a -lwayland-client -lxkbcommon -lm

$(USER_MUSL_EXT2_WLCOMP_SMOKE_ELF): user/musl_execve_ext2_wlcomp_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_WAYLAND_DEMO_ELF): user/musl_execve_ext2_wayland_demo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_DYNLINK_SMOKE_ELF): user/musl_execve_ext2_dynlink_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(MUSL_SHARED_LIBC):
	$(MAKE) -C musl shared

$(USER_MUSL_EXT2_DRM_SMOKE_ELF): user/musl_execve_ext2_drm_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_TEXT_SMOKE_ELF): user/musl_execve_ext2_text_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_CXX_SMOKE_ELF): user/musl_execve_ext2_cxx_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(CXX_TOOLCHAIN_STAMP): $(MUSL_SHARED_LIBC)
	$(MAKE) -C toolchain

$(USER_CXX_SMOKE_BIN): user/cxx_smoke.cpp user/cxx/peetcxx.h user/cxx/libpeetcxx.cpp user/cxx/plugin.cpp scripts/build_cxx_smoke.sh $(CXX_TOOLCHAIN_STAMP)
	scripts/build_cxx_smoke.sh

$(RENDER_STAMP) $(FOOT_STAMP) $(PANGO_STAMP) $(GTK_STAMP): $(WAYLAND_SHLIBS)
	$(MAKE) -C wayland

$(USER_MUSL_EXT2_FOOT_SMOKE_ELF): user/musl_execve_ext2_foot_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_FOOT_DEMO_ELF): user/musl_execve_ext2_foot_demo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_PANGO_SMOKE_ELF): user/musl_execve_ext2_pango_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_MUSL_EXT2_GTK_SMOKE_ELF): user/musl_execve_ext2_gtk_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_GTK_SMOKE_BIN): user/gtk_smoke.c $(USER_MUSL_DYN_SCRIPT) $(GTK_STAMP)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- $$(PKG_CONFIG_LIBDIR=$(RENDER_STAGE)/usr/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(abspath $(RENDER_STAGE)) pkg-config --cflags --libs gtk+-3.0 gtk+-wayland-3.0) -Wl,-rpath-link,$(abspath $(RENDER_STAGE))/usr/lib

$(USER_PANGO_SMOKE_BIN): user/pango_smoke.c $(USER_MUSL_DYN_SCRIPT) $(PANGO_STAMP)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- $$(PKG_CONFIG_LIBDIR=$(RENDER_STAGE)/usr/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(abspath $(RENDER_STAGE)) pkg-config --cflags --libs pangocairo gio-2.0 cairo-pdf cairo-svg libdrm) -Wl,-rpath-link,$(abspath $(RENDER_STAGE))/usr/lib

$(USER_TEXT_SMOKE_BIN): user/text_smoke.c $(USER_MUSL_DYN_SCRIPT) $(RENDER_STAMP)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- -I$(RENDER_STAGE)/usr/include -I$(RENDER_STAGE)/usr/include/freetype2 -I$(RENDER_STAGE)/usr/include/harfbuzz -I$(RENDER_STAGE)/usr/include/pixman-1 -I$(RENDER_STAGE)/usr/include/libdrm -L$(RENDER_STAGE)/usr/lib -Wl,-rpath-link,$(abspath $(RENDER_STAGE))/usr/lib -lfontconfig -lharfbuzz -lfreetype -lpixman-1 -ldrm

$(USER_DRM_SMOKE_BIN): user/drm_smoke.c $(USER_MUSL_DYN_SCRIPT) $(WAYLAND_SHLIBS)
	$(USER_MUSL_DYN_SCRIPT) --src $< --out $@ -- -I$(WAYLAND_OUT)/include -I$(WAYLAND_OUT)/include/libdrm -L$(WAYLAND_OUT)/lib -ldrm

$(USER_DYNLINK_SMOKE_BIN): user/dynlink_smoke.c user/dynlink/libpeetdyn.c user/dynlink/plugin.c scripts/build_dynlink_smoke.sh $(MUSL_SHARED_LIBC)
	scripts/build_dynlink_smoke.sh

$(USER_MUSL_EXT2_WAYLAND_TESTS_ELF): user/musl_execve_ext2_wayland_tests.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(WAYLAND_TESTS_STAMP): scripts/build_wayland_tests.sh wayland/peetos/tests-config.h $(WAYLAND_LIBS)
	scripts/build_wayland_tests.sh

$(USER_UNIX_RIGHTS_SMOKE_BIN): user/unix_rights_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_EPOLL_COMPOSITOR_SMOKE_BIN): user/epoll_compositor_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_SHARED_BUFFER_RIGHTS_SMOKE_BIN): user/shared_buffer_rights_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_GUI_COMPOSITOR_SMOKE_BIN): user/gui_compositor_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_PEET_GUI_COMPOSITOR_SMOKE_BIN): user/peet_gui_compositor_smoke.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_PEET_SHELL_DEMO_BIN): user/peet_shell_demo.c $(USER_MUSL_SMOKE_SCRIPT)
	$(USER_MUSL_SMOKE_SCRIPT) --src $< --out $@

$(USER_QUICKJS_BIN): scripts/build_quickjs_qjs.sh quickjs/Makefile quickjs/fetch-quickjs.sh quickjs/peetos/Makefile.peetos quickjs/peetos/repl_stub.c
	$(MAKE) quickjs

$(USER_MICROPYTHON_BIN): scripts/build_micropython_unix.sh micropython/Makefile micropython/fetch-micropython.sh
	$(MAKE) micropython

$(USER_DROPBEAR_BIN): scripts/build_dropbear_multi.sh dropbear/Makefile dropbear/fetch-dropbear.sh dropbear/peetos/localoptions.h
	$(MAKE) dropbear

$(USER_DROPBEARKEY_BIN): scripts/build_dropbear_multi.sh dropbear/Makefile dropbear/fetch-dropbear.sh dropbear/peetos/localoptions.h
	$(MAKE) dropbear

$(USER_EXT2_MUSL_IMG): $(USER_EXT2_MUSL_SCRIPT) $(USER_TOYBOX_CORE_SCRIPT) toybox/Makefile toybox/fetch-toybox.sh toybox/configs/shell-core.config user/ext2_root/hello.txt user/ext2_root/etc/motd user/ext2_root/bin/readme.txt user/ext2_root/etc/resolv.conf $(USER_EXT2_MUSL_OPTIONAL_DEPS)
	$(USER_EXT2_MUSL_SCRIPT)

ifneq ($(findstring musl-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_SMOKE_DEP := $(USER_MUSL_SMOKE_ELF)
else
KERNEL_MUSL_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-exec,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_EXEC_DEP := $(USER_MUSL_EXT2_INIT_ELF) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_EXEC_DEP :=
endif

ifneq ($(findstring musl-ext2-cmd-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_CMD_SMOKE_DEP := $(USER_MUSL_EXT2_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_CMD_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-ping-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_PING_SMOKE_DEP := $(USER_MUSL_EXT2_PING_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_PING_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-unix-rights-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_UNIX_RIGHTS_SMOKE_DEP := $(USER_MUSL_EXT2_UNIX_RIGHTS_SMOKE_ELF) $(USER_UNIX_RIGHTS_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_UNIX_RIGHTS_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-epoll-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_EPOLL_SMOKE_DEP := $(USER_MUSL_EXT2_EPOLL_SMOKE_ELF) $(USER_EPOLL_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_EPOLL_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-shared-buffer-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_SHARED_BUFFER_SMOKE_DEP := $(USER_MUSL_EXT2_SHARED_BUFFER_SMOKE_ELF) $(USER_SHARED_BUFFER_RIGHTS_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_SHARED_BUFFER_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-console-first-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_CONSOLE_FIRST_SMOKE_DEP := $(USER_MUSL_EXT2_CONSOLE_FIRST_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_CONSOLE_FIRST_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-gui-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_GUI_SMOKE_DEP := $(USER_MUSL_EXT2_GUI_SMOKE_ELF) $(USER_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_GUI_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-gui-demo,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_GUI_DEMO_DEP := $(USER_MUSL_EXT2_GUI_DEMO_ELF) $(USER_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_GUI_DEMO_DEP :=
endif

ifneq ($(findstring musl-ext2-peet-gui-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_PEET_GUI_SMOKE_DEP := $(USER_MUSL_EXT2_PEET_GUI_SMOKE_ELF) $(USER_PEET_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_PEET_GUI_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-peet-shell-demo,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_PEET_SHELL_DEMO_DEP := $(USER_MUSL_EXT2_PEET_SHELL_DEMO_ELF) $(USER_PEET_SHELL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_PEET_SHELL_DEMO_DEP :=
endif

ifneq ($(findstring musl-ext2-quickjs-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_QUICKJS_SMOKE_DEP := $(USER_MUSL_EXT2_QUICKJS_SMOKE_ELF) $(USER_QUICKJS_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_QUICKJS_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-micropython-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_MICROPYTHON_SMOKE_DEP := $(USER_MUSL_EXT2_MICROPYTHON_SMOKE_ELF) $(USER_MICROPYTHON_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_MICROPYTHON_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-dropbear-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_DROPBEAR_SMOKE_DEP := $(USER_MUSL_EXT2_DROPBEAR_SMOKE_ELF) $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN) $(USER_DROPBEAR_HOSTKEY) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_DROPBEAR_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-fpu-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_FPU_SMOKE_DEP := $(USER_MUSL_EXT2_FPU_SMOKE_ELF) $(USER_FPU_CONTEXT_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_FPU_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-proc-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_PROC_SMOKE_DEP := $(USER_MUSL_EXT2_PROC_SMOKE_ELF) $(USER_PROC_MODEL_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_PROC_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-wayland-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_WAYLAND_SMOKE_DEP := $(USER_MUSL_EXT2_WAYLAND_SMOKE_ELF) $(USER_WAYLAND_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_WAYLAND_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-wlcomp-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_WLCOMP_SMOKE_DEP := $(USER_MUSL_EXT2_WLCOMP_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_WLCOMP_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-wayland-demo,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_WAYLAND_DEMO_DEP := $(USER_MUSL_EXT2_WAYLAND_DEMO_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_WAYLAND_DEMO_DEP :=
endif

ifneq ($(findstring musl-ext2-dynlink-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_DYNLINK_SMOKE_DEP := $(USER_MUSL_EXT2_DYNLINK_SMOKE_ELF) $(USER_DYNLINK_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_DYNLINK_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-drm-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_DRM_SMOKE_DEP := $(USER_MUSL_EXT2_DRM_SMOKE_ELF) $(USER_DRM_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_DRM_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-text-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_TEXT_SMOKE_DEP := $(USER_MUSL_EXT2_TEXT_SMOKE_ELF) $(USER_TEXT_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_TEXT_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-gtk-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_GTK_SMOKE_DEP := $(USER_MUSL_EXT2_GTK_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_GTK_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_GTK_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-pango-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_PANGO_SMOKE_DEP := $(USER_MUSL_EXT2_PANGO_SMOKE_ELF) $(USER_PANGO_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_PANGO_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-cxx-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_CXX_SMOKE_DEP := $(USER_MUSL_EXT2_CXX_SMOKE_ELF) $(USER_CXX_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_CXX_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-foot-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_FOOT_SMOKE_DEP := $(USER_MUSL_EXT2_FOOT_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(FOOT_STAMP) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_FOOT_SMOKE_DEP :=
endif

ifneq ($(findstring musl-ext2-foot-demo,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_FOOT_DEMO_DEP := $(USER_MUSL_EXT2_FOOT_DEMO_ELF) $(USER_PEETWL_BIN) $(FOOT_STAMP) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_FOOT_DEMO_DEP :=
endif

ifneq ($(findstring musl-ext2-wayland-tests,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT2_WAYLAND_TESTS_DEP := $(USER_MUSL_EXT2_WAYLAND_TESTS_ELF) $(WAYLAND_TESTS_STAMP) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT2_WAYLAND_TESTS_DEP :=
endif

ifneq ($(findstring musl-ext3-journal-smoke,$(KERNEL_FEATURES)),)
KERNEL_MUSL_EXT3_JOURNAL_SMOKE_DEP := $(USER_MUSL_EXT3_JOURNAL_SMOKE_ELF) $(USER_JOURNAL_SMOKE_HELPER_BIN) $(USER_EXT2_MUSL_IMG)
else
KERNEL_MUSL_EXT3_JOURNAL_SMOKE_DEP :=
endif

user: user-elf $(USER_EXT2_IMG)

# Default-build embedded user binaries (needed by `cargo test -p kernel`).
user-elf: $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO)

user-musl-smoke: $(USER_MUSL_SMOKE_ELF)
user-musl-ext2-exec: $(USER_MUSL_EXT2_INIT_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-cmd-smoke: $(USER_MUSL_EXT2_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-ping-smoke: $(USER_MUSL_EXT2_PING_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-unix-rights-smoke: $(USER_MUSL_EXT2_UNIX_RIGHTS_SMOKE_ELF) $(USER_UNIX_RIGHTS_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-epoll-smoke: $(USER_MUSL_EXT2_EPOLL_SMOKE_ELF) $(USER_EPOLL_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-shared-buffer-smoke: $(USER_MUSL_EXT2_SHARED_BUFFER_SMOKE_ELF) $(USER_SHARED_BUFFER_RIGHTS_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-console-first-smoke: $(USER_MUSL_EXT2_CONSOLE_FIRST_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-gui-smoke: $(USER_MUSL_EXT2_GUI_SMOKE_ELF) $(USER_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-gui-demo: $(USER_MUSL_EXT2_GUI_DEMO_ELF) $(USER_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-peet-gui-smoke: $(USER_MUSL_EXT2_PEET_GUI_SMOKE_ELF) $(USER_PEET_GUI_COMPOSITOR_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-peet-shell-demo: $(USER_MUSL_EXT2_PEET_SHELL_DEMO_ELF) $(USER_PEET_SHELL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-quickjs-smoke: $(USER_MUSL_EXT2_QUICKJS_SMOKE_ELF) $(USER_QUICKJS_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-micropython-smoke: $(USER_MUSL_EXT2_MICROPYTHON_SMOKE_ELF) $(USER_MICROPYTHON_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-dropbear-smoke: $(USER_MUSL_EXT2_DROPBEAR_SMOKE_ELF) $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN) $(USER_DROPBEAR_HOSTKEY) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-fpu-smoke: $(USER_MUSL_EXT2_FPU_SMOKE_ELF) $(USER_FPU_CONTEXT_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-proc-smoke: $(USER_MUSL_EXT2_PROC_SMOKE_ELF) $(USER_PROC_MODEL_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-wayland-smoke: $(USER_MUSL_EXT2_WAYLAND_SMOKE_ELF) $(USER_WAYLAND_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-wlcomp-smoke: $(USER_MUSL_EXT2_WLCOMP_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-wayland-demo: $(USER_MUSL_EXT2_WAYLAND_DEMO_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-dynlink-smoke: $(USER_MUSL_EXT2_DYNLINK_SMOKE_ELF) $(USER_DYNLINK_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-drm-smoke: $(USER_MUSL_EXT2_DRM_SMOKE_ELF) $(USER_DRM_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-text-smoke: $(USER_MUSL_EXT2_TEXT_SMOKE_ELF) $(USER_TEXT_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-foot-smoke: $(USER_MUSL_EXT2_FOOT_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_WL_DEMO_BIN) $(FOOT_STAMP) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-gtk-smoke: $(USER_MUSL_EXT2_GTK_SMOKE_ELF) $(USER_PEETWL_BIN) $(USER_GTK_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-pango-smoke: $(USER_MUSL_EXT2_PANGO_SMOKE_ELF) $(USER_PANGO_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-cxx-smoke: $(USER_MUSL_EXT2_CXX_SMOKE_ELF) $(USER_CXX_SMOKE_BIN) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-foot-demo: $(USER_MUSL_EXT2_FOOT_DEMO_ELF) $(USER_PEETWL_BIN) $(FOOT_STAMP) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-wayland-tests: $(USER_MUSL_EXT2_WAYLAND_TESTS_ELF) $(WAYLAND_TESTS_STAMP) $(USER_EXT2_MUSL_IMG)
user-musl-ext3-journal-smoke: $(USER_MUSL_EXT3_JOURNAL_SMOKE_ELF) $(USER_JOURNAL_SMOKE_HELPER_BIN) $(USER_EXT2_MUSL_IMG)

kernel: $(ENTRY_OBJ) $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO) $(USER_EXT2_IMG) $(KERNEL_MUSL_SMOKE_DEP) $(KERNEL_MUSL_EXT2_EXEC_DEP) $(KERNEL_MUSL_EXT2_CMD_SMOKE_DEP) $(KERNEL_MUSL_EXT2_PING_SMOKE_DEP) $(KERNEL_MUSL_EXT2_UNIX_RIGHTS_SMOKE_DEP) $(KERNEL_MUSL_EXT2_EPOLL_SMOKE_DEP) $(KERNEL_MUSL_EXT2_SHARED_BUFFER_SMOKE_DEP) $(KERNEL_MUSL_EXT2_CONSOLE_FIRST_SMOKE_DEP) $(KERNEL_MUSL_EXT2_GUI_SMOKE_DEP) $(KERNEL_MUSL_EXT2_GUI_DEMO_DEP) $(KERNEL_MUSL_EXT2_PEET_GUI_SMOKE_DEP) $(KERNEL_MUSL_EXT2_PEET_SHELL_DEMO_DEP) $(KERNEL_MUSL_EXT2_QUICKJS_SMOKE_DEP) $(KERNEL_MUSL_EXT2_MICROPYTHON_SMOKE_DEP) $(KERNEL_MUSL_EXT2_DROPBEAR_SMOKE_DEP) $(KERNEL_MUSL_EXT2_FPU_SMOKE_DEP) $(KERNEL_MUSL_EXT2_PROC_SMOKE_DEP) $(KERNEL_MUSL_EXT2_WAYLAND_SMOKE_DEP) $(KERNEL_MUSL_EXT2_WLCOMP_SMOKE_DEP) $(KERNEL_MUSL_EXT2_WAYLAND_DEMO_DEP) $(KERNEL_MUSL_EXT2_DYNLINK_SMOKE_DEP) $(KERNEL_MUSL_EXT2_DRM_SMOKE_DEP) $(KERNEL_MUSL_EXT2_TEXT_SMOKE_DEP) $(KERNEL_MUSL_EXT2_FOOT_SMOKE_DEP) $(KERNEL_MUSL_EXT2_FOOT_DEMO_DEP) $(KERNEL_MUSL_EXT2_CXX_SMOKE_DEP) $(KERNEL_MUSL_EXT2_PANGO_SMOKE_DEP) $(KERNEL_MUSL_EXT2_GTK_SMOKE_DEP) $(KERNEL_MUSL_EXT2_WAYLAND_TESTS_DEP) $(KERNEL_MUSL_EXT3_JOURNAL_SMOKE_DEP)
	RUSTFLAGS="-C link-arg=$(ENTRY_OBJ) -C link-arg=-T$(abspath linker/kernel.ld) -C link-arg=-no-pie" cargo build -p kernel $(CARGO_FLAGS) $(CARGO_FEATURES)
	@case " $(KERNEL_FEATURES) " in \
		*" musl-ext2-gtk-smoke "*) init_dep="musl_execve_ext2_gtk_smoke.elf" ;; \
		*" musl-ext2-pango-smoke "*) init_dep="musl_execve_ext2_pango_smoke.elf" ;; \
		*" musl-ext2-cxx-smoke "*) init_dep="musl_execve_ext2_cxx_smoke.elf" ;; \
		*" musl-ext3-journal-smoke "*) init_dep="musl_execve_ext3_journal_smoke.elf" ;; \
		*" musl-ext2-dynlink-smoke "*) init_dep="musl_execve_ext2_dynlink_smoke.elf" ;; \
		*" musl-ext2-drm-smoke "*) init_dep="musl_execve_ext2_drm_smoke.elf" ;; \
		*" musl-ext2-text-smoke "*) init_dep="musl_execve_ext2_text_smoke.elf" ;; \
		*" musl-ext2-foot-smoke "*) init_dep="musl_execve_ext2_foot_smoke.elf" ;; \
		*" musl-ext2-foot-demo "*) init_dep="musl_execve_ext2_foot_demo.elf" ;; \
		*" musl-ext2-wlcomp-smoke "*) init_dep="musl_execve_ext2_wlcomp_smoke.elf" ;; \
		*" musl-ext2-wayland-demo "*) init_dep="musl_execve_ext2_wayland_demo.elf" ;; \
		*" musl-ext2-wayland-smoke "*) init_dep="musl_execve_ext2_wayland_smoke.elf" ;; \
		*" musl-ext2-wayland-tests "*) init_dep="musl_execve_ext2_wayland_tests.elf" ;; \
		*" musl-ext2-proc-smoke "*) init_dep="musl_execve_ext2_proc_smoke.elf" ;; \
		*" musl-ext2-fpu-smoke "*) init_dep="musl_execve_ext2_fpu_smoke.elf" ;; \
		*" musl-ext2-dropbear-smoke "*) init_dep="musl_execve_ext2_dropbear_smoke.elf" ;; \
		*" musl-ext2-micropython-smoke "*) init_dep="musl_execve_ext2_micropython_smoke.elf" ;; \
		*" musl-ext2-quickjs-smoke "*) init_dep="musl_execve_ext2_quickjs_smoke.elf" ;; \
		*" musl-ext2-ping-smoke "*) init_dep="musl_execve_ext2_ping_smoke.elf" ;; \
		*" musl-ext2-shared-buffer-smoke "*) init_dep="musl_execve_ext2_shared_buffer_smoke.elf" ;; \
		*" musl-ext2-epoll-smoke "*) init_dep="musl_execve_ext2_epoll_smoke.elf" ;; \
		*" musl-ext2-console-first-smoke "*) init_dep="musl_execve_ext2_console_first_smoke.elf" ;; \
		*" musl-ext2-gui-demo "*) init_dep="musl_execve_ext2_gui_demo.elf" ;; \
		*" musl-ext2-gui-smoke "*) init_dep="musl_execve_ext2_gui_smoke.elf" ;; \
		*" musl-ext2-peet-gui-smoke "*) init_dep="musl_execve_ext2_peet_gui_smoke.elf" ;; \
		*" musl-ext2-peet-shell-demo "*) init_dep="musl_execve_ext2_peet_shell_demo.elf" ;; \
		*" musl-ext2-unix-rights-smoke "*) init_dep="musl_execve_ext2_unix_rights_smoke.elf" ;; \
		*" musl-ext2-cmd-smoke "*) init_dep="musl_execve_ext2_smoke.elf" ;; \
		*" musl-ext2-exec "*) init_dep="musl_execve_ext2.elf" ;; \
		*" musl-smoke "*) init_dep="musl_echo.elf" ;; \
		*) init_dep="hello_dyn.elf" ;; \
	esac; \
	kernel_artifact=""; \
	for depfile in $(KERNEL_TARGET_DIR)/deps/kernel-*.d; do \
		[ -f "$$depfile" ] || continue; \
		if ! grep -q "$$init_dep" "$$depfile"; then \
			continue; \
		fi; \
		candidate="$${depfile%.d}"; \
		[ -f "$$candidate" ] || continue; \
		if [ -z "$$kernel_artifact" ] || [ "$$candidate" -nt "$$kernel_artifact" ]; then \
			kernel_artifact="$$candidate"; \
		fi; \
	done; \
	if [ -z "$$kernel_artifact" ]; then \
		echo "ERROR: failed to locate kernel artifact for init dependency '$$init_dep' in $(KERNEL_TARGET_DIR)/deps"; \
		exit 1; \
	fi; \
	cp "$$kernel_artifact" $(KERNEL_ELF)
	$(OBJCOPY) -O binary $(KERNEL_ELF) $(KERNEL_BIN)

# ---------------------------------------------------------------------------
# Disk Image
# ---------------------------------------------------------------------------

image: bootloader kernel
	@dd if=/dev/zero of=$(IMAGE) bs=$(SECTOR_SIZE) count=$(DISK_SECTORS) status=none
	@dd if=$(BOOT_BIN) of=$(IMAGE) conv=notrunc status=none
	@dd if=$(STAGE2_BIN) of=$(IMAGE) bs=$(SECTOR_SIZE) seek=1 conv=notrunc status=none
	@kernel_size=$$(stat -c%s $(KERNEL_BIN)); \
	kernel_sectors=$$(( (kernel_size + $(SECTOR_SIZE) - 1) / $(SECTOR_SIZE) )); \
	if [ $$kernel_sectors -gt 65535 ]; then \
		echo "ERROR: kernel payload uses $$kernel_sectors sectors; stage2 metadata format supports at most 65535."; \
		exit 1; \
	fi; \
	kernel_meta_lba=$$((1 + $(STAGE2_SECTORS))); \
	kernel_payload_lba=$$((kernel_meta_lba + 1)); \
	if [ $$((kernel_payload_lba + kernel_sectors)) -gt $(DISK_SECTORS) ]; then \
		echo "ERROR: kernel payload does not fit in disk image ($$kernel_sectors sectors starting at LBA $$kernel_payload_lba)."; \
		exit 1; \
	fi; \
	kernel_meta_bin="$(BUILD_DIR)/kernel.meta.bin"; \
	dd if=/dev/zero of="$$kernel_meta_bin" bs=$(SECTOR_SIZE) count=1 status=none; \
	printf 'PKRN' | dd of="$$kernel_meta_bin" bs=1 seek=0 conv=notrunc status=none; \
	printf '%b' "$$(printf '\\%03o\\%03o\\%03o\\%03o' \
		$$((kernel_size & 0xFF)) \
		$$(((kernel_size >> 8) & 0xFF)) \
		$$(((kernel_size >> 16) & 0xFF)) \
		$$(((kernel_size >> 24) & 0xFF)))" \
		| dd of="$$kernel_meta_bin" bs=1 seek=4 conv=notrunc status=none; \
	printf '%b' "$$(printf '\\%03o\\%03o' \
		$$((kernel_sectors & 0xFF)) \
		$$(((kernel_sectors >> 8) & 0xFF)))" \
		| dd of="$$kernel_meta_bin" bs=1 seek=8 conv=notrunc status=none; \
	dd if="$$kernel_meta_bin" of=$(IMAGE) bs=$(SECTOR_SIZE) seek=$$kernel_meta_lba conv=notrunc status=none; \
	dd if=$(KERNEL_BIN) of=$(IMAGE) bs=$(SECTOR_SIZE) seek=$$kernel_payload_lba conv=notrunc status=none
	@echo "Built $(IMAGE)"

run:
	$(MAKE) image PROFILE=release KERNEL_FEATURES="$(RUN_ALL_FEATURES)"
	$(QEMU) -smp $(QEMU_SMP) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-usb \
		-device usb-tablet \
		-serial stdio -no-reboot -no-shutdown

run-current:
	@if [ ! -f $(IMAGE) ]; then \
		echo "ERROR: $(IMAGE) not found. Build it first with 'make image'."; \
		exit 1; \
	fi
	$(QEMU) -smp $(QEMU_SMP) -drive format=raw,file=$(IMAGE) -serial stdio

run-image: image
	$(QEMU) -smp $(QEMU_SMP) -drive format=raw,file=$(IMAGE) -serial stdio

run-tablet:
	$(MAKE) image PROFILE=release KERNEL_FEATURES="$(strip $(KERNEL_FEATURES) usb-tablet)"
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-usb \
		-device usb-tablet \
		-serial stdio -no-reboot -no-shutdown

run-shell:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
	$(QEMU) -smp $(QEMU_SMP) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-serial stdio -no-reboot -no-shutdown

run-net-shell:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
	$(QEMU) -smp $(QEMU_SMP) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -no-reboot -no-shutdown

run-net:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=$(if $(KERNEL_FEATURES),$(KERNEL_FEATURES),)
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -no-reboot -no-shutdown

run-net-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-ping-smoke
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-udp-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-udp-smoke
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-tcp-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-tcp-smoke
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-socket-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-socket-smoke
	$(QEMU) -smp $(QEMU_SMP) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-shell-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-cmd-smoke
	$(QEMU) -smp $(QEMU_SMP) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-serial stdio -display none -no-reboot -no-shutdown

run-ping-smoke:
	scripts/run_toybox_ping_smoke.sh

run-unix-rights-smoke:
	scripts/run_toybox_unix_rights_smoke.sh

run-epoll-smoke:
	scripts/run_toybox_epoll_smoke.sh

run-shared-buffer-smoke:
	scripts/run_toybox_shared_buffer_smoke.sh

run-console-first-smoke:
	scripts/run_toybox_console_first_smoke.sh

run-gui-smoke:
	scripts/run_toybox_gui_smoke.sh

run-gui-demo:
	scripts/run_toybox_gui_demo.sh

run-gui-fallback-smoke:
	scripts/run_toybox_gui_fallback_smoke.sh

run-peet-gui-smoke:
	scripts/run_toybox_peet_gui_smoke.sh

run-peet-shell-demo:
	$(MAKE) image PROFILE=release KERNEL_FEATURES="compositor-first musl-ext2-peet-shell-demo"
	$(QEMU) -smp $(QEMU_SMP) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-serial stdio -display gtk -no-reboot -no-shutdown

run-peet-shell-demo-smoke:
	scripts/run_toybox_peet_shell_demo_smoke.sh

run-quickjs-smoke:
	scripts/run_toybox_quickjs_smoke.sh

run-micropython-smoke:
	scripts/run_toybox_micropython_smoke.sh

run-dropbear-smoke:
	scripts/run_toybox_dropbear_smoke.sh

run-fpu-smoke:
	scripts/run_toybox_fpu_smoke.sh

run-proc-smoke:
	scripts/run_toybox_proc_smoke.sh

run-wayland-smoke:
	scripts/run_toybox_wayland_smoke.sh

run-wlcomp-smoke:
	scripts/run_toybox_wlcomp_smoke.sh

run-wayland-demo:
	scripts/run_toybox_wayland_demo.sh

run-dynlink-smoke:
	scripts/run_toybox_dynlink_smoke.sh

run-drm-smoke:
	scripts/run_toybox_drm_smoke.sh

run-text-smoke:
	scripts/run_toybox_text_smoke.sh

run-cxx-smoke:
	scripts/run_toybox_cxx_smoke.sh

run-pango-smoke:
	scripts/run_toybox_pango_smoke.sh

run-gtk-smoke:
	scripts/run_toybox_gtk_smoke.sh

run-foot-smoke:
	scripts/run_toybox_foot_smoke.sh

run-foot-demo:
	scripts/run_toybox_foot_demo.sh

run-wayland-tests:
	scripts/run_toybox_wayland_tests.sh
run-journal-smoke:
	scripts/run_toybox_journal_smoke.sh

run-ext4-ro-smoke:
	scripts/run_toybox_ext4_ro_smoke.sh

test:
	@tests/run.sh

ext2-image: $(USER_EXT2_SCRIPT) user/ext2_root/hello.txt user/ext2_root/etc/motd user/ext2_root/bin/readme.txt
	$(USER_EXT2_SCRIPT)

# ---------------------------------------------------------------------------
# musl user-space runtime bootstrap (stage 9 prep)
# ---------------------------------------------------------------------------

musl-fetch:
	$(MAKE) -C musl fetch

musl-configure:
	$(MAKE) -C musl configure

musl:
	$(MAKE) -C musl all

musl-clean:
	$(MAKE) -C musl clean

musl-distclean:
	$(MAKE) -C musl distclean

musl-info:
	$(MAKE) -C musl info

# ---------------------------------------------------------------------------
# toybox userspace staging (stage 9 prep)
# ---------------------------------------------------------------------------

toybox-fetch:
	$(MAKE) -C toybox fetch

toybox-configure:
	$(MAKE) -C toybox configure

toybox:
	$(MAKE) -C toybox all

toybox-clean:
	$(MAKE) -C toybox clean

toybox-distclean:
	$(MAKE) -C toybox distclean

toybox-info:
	$(MAKE) -C toybox info

# ---------------------------------------------------------------------------
# quickjs userspace staging (stage 9 prep)
# ---------------------------------------------------------------------------

quickjs-fetch:
	$(MAKE) -C quickjs fetch

quickjs:
	$(MAKE) -C quickjs qjs
	cp quickjs/out/qjs $(USER_QUICKJS_BIN)
	chmod 0755 $(USER_QUICKJS_BIN)

quickjs-clean:
	$(MAKE) -C quickjs clean
	rm -f $(USER_QUICKJS_BIN)

quickjs-distclean:
	$(MAKE) -C quickjs distclean
	rm -f $(USER_QUICKJS_BIN)

quickjs-info:
	$(MAKE) -C quickjs info

# ---------------------------------------------------------------------------
# micropython userspace staging (stage 9 prep)
# ---------------------------------------------------------------------------

micropython-fetch:
	$(MAKE) -C micropython fetch

micropython:
	$(MAKE) -C micropython micropython
	cp micropython/out/micropython $(USER_MICROPYTHON_BIN)
	chmod 0755 $(USER_MICROPYTHON_BIN)

micropython-clean:
	$(MAKE) -C micropython clean
	rm -f $(USER_MICROPYTHON_BIN)

micropython-distclean:
	$(MAKE) -C micropython distclean
	rm -f $(USER_MICROPYTHON_BIN)

micropython-info:
	$(MAKE) -C micropython info

# ---------------------------------------------------------------------------
# dropbear userspace staging (stage 9 prep)
# ---------------------------------------------------------------------------

dropbear-fetch:
	$(MAKE) -C dropbear fetch

dropbear:
	$(MAKE) -C dropbear dropbear
	cp dropbear/out/dropbear $(USER_DROPBEAR_BIN)
	cp dropbear/out/dropbearkey $(USER_DROPBEARKEY_BIN)
	chmod 0755 $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN)

$(USER_DROPBEAR_HOSTKEY): $(USER_DROPBEARKEY_BIN)
	rm -f $(USER_DROPBEAR_HOSTKEY) $(USER_DROPBEAR_HOSTKEY).pub
	./$(USER_DROPBEARKEY_BIN) -t ed25519 -f $(USER_DROPBEAR_HOSTKEY) >/dev/null
	chmod 0600 $(USER_DROPBEAR_HOSTKEY)

dropbear-clean:
	$(MAKE) -C dropbear clean
	rm -f $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN) $(USER_DROPBEAR_HOSTKEY) $(USER_DROPBEAR_HOSTKEY).pub

dropbear-distclean:
	$(MAKE) -C dropbear distclean
	rm -f $(USER_DROPBEAR_BIN) $(USER_DROPBEARKEY_BIN) $(USER_DROPBEAR_HOSTKEY) $(USER_DROPBEAR_HOSTKEY).pub

dropbear-info:
	$(MAKE) -C dropbear info

clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf target
