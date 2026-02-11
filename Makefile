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
KERNEL_TARGET := $(KERNEL_TARGET_DIR)/kernel

KERNEL_FEATURES ?=
ifneq ($(strip $(KERNEL_FEATURES)),)
CARGO_FEATURES := --features $(KERNEL_FEATURES)
else
CARGO_FEATURES :=
endif

.PHONY: all image bootloader kernel user clean run test ext2-image

all: image

$(BUILD_DIR):
	@mkdir -p $(BUILD_DIR)

# ---------------------------------------------------------------------------
# Bootloader
# ---------------------------------------------------------------------------

$(BOOT_BIN): $(BOOT_STAGE1_SRC) $(BOOT_CONSTANTS) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $<

$(STAGE2_BIN): $(BOOT_STAGE2_SRC) $(BOOT_CONSTANTS) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $<

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

user: $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO) $(USER_EXT2_IMG)

kernel: $(ENTRY_OBJ) $(USER_ELF) $(USER_DYN_ELF) $(USER_INTERP_SO) $(USER_EXT2_IMG)
	RUSTFLAGS="-C link-arg=$(ENTRY_OBJ) -C link-arg=-T$(abspath linker/kernel.ld) -C link-arg=-no-pie" cargo build -p kernel $(CARGO_FLAGS) $(CARGO_FEATURES)
	@cp $(KERNEL_TARGET) $(KERNEL_ELF)
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

run: image
	$(QEMU) -drive format=raw,file=$(IMAGE) -serial stdio

test:
	@tests/run.sh

ext2-image: $(USER_EXT2_SCRIPT) user/ext2_root/hello.txt user/ext2_root/etc/motd user/ext2_root/bin/readme.txt
	$(USER_EXT2_SCRIPT)

clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf target
