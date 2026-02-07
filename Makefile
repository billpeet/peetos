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

KERNEL_ELF := $(BUILD_DIR)/kernel.elf
KERNEL_BIN := $(BUILD_DIR)/kernel.bin

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
ENTRY_SRC := kernel/arch/x86_64/entry.asm

KERNEL_TARGET_DIR := target/x86_64-unknown-none/$(PROFILE)
KERNEL_TARGET := $(KERNEL_TARGET_DIR)/kernel

.PHONY: all image bootloader kernel clean run

all: image

$(BUILD_DIR):
	@mkdir -p $(BUILD_DIR)

# ---------------------------------------------------------------------------
# Bootloader
# ---------------------------------------------------------------------------

$(BOOT_BIN): $(BOOT_STAGE1_SRC) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $<

$(STAGE2_BIN): $(BOOT_STAGE2_SRC) | $(BUILD_DIR)
	$(ASM) -f bin -g -o $@ $<

bootloader: $(BOOT_BIN) $(STAGE2_BIN)

# ---------------------------------------------------------------------------
# Kernel
# ---------------------------------------------------------------------------

$(ENTRY_OBJ): $(ENTRY_SRC) | $(BUILD_DIR)
	$(ASM) -f elf64 -g -o $@ $<

kernel: $(ENTRY_OBJ)
	RUSTFLAGS="-C link-arg=$(ENTRY_OBJ) -C link-arg=-T$(abspath linker/kernel.ld) -C link-arg=-no-pie" cargo build -p kernel $(CARGO_FLAGS)
	@cp $(KERNEL_TARGET) $(KERNEL_ELF)
	$(OBJCOPY) -O binary $(KERNEL_ELF) $(KERNEL_BIN)

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

clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf target
