# ===========================================================================
# PeetOS Rust Makefile
# ===========================================================================

ASM      := nasm
LD       := ld
OBJCOPY  := objcopy
QEMU     := qemu-system-x86_64

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
USER_EXT2_MUSL_IMG := user/sample_ext2_musl.img

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
USER_EXT2_MUSL_SCRIPT := scripts/build_ext2_musl_execve.sh
USER_TOYBOX_SH_SCRIPT := scripts/build_toybox_sh.sh
USER_TOYBOX_CORE_SCRIPT := scripts/build_toybox_core.sh

PROFILE ?= release
ifeq ($(PROFILE),release)
CARGO_FLAGS := --release
else
CARGO_FLAGS :=
endif

SECTOR_SIZE := 512
STAGE2_SECTORS := 16
DISK_SECTORS := 20480
STAGE2_KERNEL_LOAD_ADDR := 0x18000
LOW_MEM_HOLE_ADDR := 0xA0000

BOOT_STAGE1_SRC := boot/stage1/boot.asm
BOOT_STAGE2_SRC := boot/stage2/stage2.asm
BOOT_CONSTANTS := boot/include/constants.inc
ENTRY_SRC := kernel/arch/x86_64/entry.asm

KERNEL_TARGET_DIR := target/x86_64-unknown-none/$(PROFILE)

KERNEL_FEATURES ?=
ifneq ($(strip $(KERNEL_FEATURES)),)
CARGO_FEATURES := --features $(KERNEL_FEATURES)
else
CARGO_FEATURES :=
endif

.PHONY: all image bootloader kernel user user-musl-smoke user-musl-ext2-exec user-musl-ext2-cmd-smoke user-musl-ext2-ping-smoke clean run run-current run-image run-shell run-net-shell run-net run-net-udp-smoke run-net-tcp-smoke run-net-socket-smoke run-ping-smoke test ext2-image \
	musl-fetch musl-configure musl musl-clean musl-distclean musl-info \
	toybox-fetch toybox-configure toybox toybox-clean toybox-distclean toybox-info run-shell-smoke

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

$(USER_EXT2_MUSL_IMG): $(USER_EXT2_MUSL_SCRIPT) $(USER_TOYBOX_CORE_SCRIPT) toybox/Makefile toybox/fetch-toybox.sh toybox/configs/shell-core.config user/ext2_root/hello.txt user/ext2_root/etc/motd user/ext2_root/bin/readme.txt user/ext2_root/etc/resolv.conf
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

user: $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO) $(USER_EXT2_IMG)

user-musl-smoke: $(USER_MUSL_SMOKE_ELF)
user-musl-ext2-exec: $(USER_MUSL_EXT2_INIT_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-cmd-smoke: $(USER_MUSL_EXT2_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)
user-musl-ext2-ping-smoke: $(USER_MUSL_EXT2_PING_SMOKE_ELF) $(USER_EXT2_MUSL_IMG)

kernel: $(ENTRY_OBJ) $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO) $(USER_EXT2_IMG) $(KERNEL_MUSL_SMOKE_DEP) $(KERNEL_MUSL_EXT2_EXEC_DEP) $(KERNEL_MUSL_EXT2_CMD_SMOKE_DEP) $(KERNEL_MUSL_EXT2_PING_SMOKE_DEP)
	RUSTFLAGS="-C link-arg=$(ENTRY_OBJ) -C link-arg=-T$(abspath linker/kernel.ld) -C link-arg=-no-pie" cargo build -p kernel $(CARGO_FLAGS) $(CARGO_FEATURES)
	@case " $(KERNEL_FEATURES) " in \
		*" musl-ext2-ping-smoke "*) init_dep="musl_execve_ext2_ping_smoke.elf" ;; \
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
	@kernel_size=$$(stat -c%s $(KERNEL_BIN)); \
	stage2_cap=$$(( $(LOW_MEM_HOLE_ADDR) - $(STAGE2_KERNEL_LOAD_ADDR) )); \
	if [ $$kernel_size -gt $$stage2_cap ]; then \
		echo "ERROR: kernel.bin ($$kernel_size bytes) exceeds stage2 staging window ($$stage2_cap bytes)."; \
		echo "Reduce kernel image size or update bootloader staging strategy."; \
		exit 1; \
	fi

# ---------------------------------------------------------------------------
# Disk Image
# ---------------------------------------------------------------------------

image: bootloader kernel
	@dd if=/dev/zero of=$(IMAGE) bs=$(SECTOR_SIZE) count=$(DISK_SECTORS) status=none
	@dd if=$(BOOT_BIN) of=$(IMAGE) conv=notrunc status=none
	@dd if=$(STAGE2_BIN) of=$(IMAGE) bs=$(SECTOR_SIZE) seek=1 conv=notrunc status=none
	@dd if=$(KERNEL_BIN) of=$(IMAGE) bs=$(SECTOR_SIZE) seek=$(shell expr 1 + $(STAGE2_SECTORS)) conv=notrunc status=none
	@echo "Built $(IMAGE)"

run: run-image

run-current:
	@if [ ! -f $(IMAGE) ]; then \
		echo "ERROR: $(IMAGE) not found. Build it first with 'make image'."; \
		exit 1; \
	fi
	$(QEMU) -drive format=raw,file=$(IMAGE) -serial stdio

run-image: image
	$(QEMU) -drive format=raw,file=$(IMAGE) -serial stdio

run-shell:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
	$(QEMU) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-serial stdio -no-reboot -no-shutdown

run-net-shell:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
	$(QEMU) \
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
	$(QEMU) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -no-reboot -no-shutdown

run-net-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-ping-smoke
	$(QEMU) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-udp-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-udp-smoke
	$(QEMU) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-tcp-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-tcp-smoke
	$(QEMU) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-net-socket-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=net-socket-smoke
	$(QEMU) \
		-drive format=raw,file=$(IMAGE) \
		-netdev user,id=net0 \
		-device e1000,netdev=net0 \
		-serial stdio -display none -no-reboot -no-shutdown

run-shell-smoke:
	$(MAKE) image PROFILE=release KERNEL_FEATURES=musl-ext2-cmd-smoke
	$(QEMU) \
		-drive id=disk0,file=$(IMAGE),if=none,format=raw \
		-device ich9-ahci,id=ahci \
		-device ide-hd,drive=disk0,bus=ahci.0 \
		-drive id=disk1,file=$(USER_EXT2_MUSL_IMG),if=none,format=raw \
		-device ide-hd,drive=disk1,bus=ahci.1 \
		-serial stdio -display none -no-reboot -no-shutdown

run-ping-smoke:
	scripts/run_toybox_ping_smoke.sh

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

clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf target
