# ==========================================================================
# toybox build glue for PeetOS userspace staging
# ==========================================================================

TOYBOX_VERSION ?= 0.8.13
TOYBOX_SRC ?= toybox-src
OUT_DIR ?= $(CURDIR)/out
CONFIG_FRAGMENT ?= $(CURDIR)/configs/echo-only.config
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)
HOSTCC ?= cc
TOYBOX_CFLAGS ?= -Os -fno-stack-protector -fno-omit-frame-pointer -ffunction-sections -fdata-sections
TOYBOX_LDFLAGS ?= -static -no-pie -Wl,-z,max-page-size=0x1000 -Wl,--gc-sections
JOBS ?= $(shell nproc)

CONFIG_FRAGMENT_ABS := $(abspath $(CONFIG_FRAGMENT))

.PHONY: all fetch configure build echo clean distclean info

all: echo

fetch:
	./fetch-toybox.sh --version "$(TOYBOX_VERSION)" --output-dir "$(CURDIR)"

$(TOYBOX_SRC):
	$(MAKE) fetch

configure: $(TOYBOX_SRC)/.config

$(TOYBOX_SRC)/.config: $(TOYBOX_SRC) $(CONFIG_FRAGMENT)
	@if [ ! -f "$(CONFIG_FRAGMENT_ABS)" ]; then \
		echo "Toybox config fragment missing: $(CONFIG_FRAGMENT_ABS)" >&2; \
		exit 1; \
	fi
	@cd $(TOYBOX_SRC) && make distclean >/dev/null && make allnoconfig KCONFIG_ALLCONFIG="$(CONFIG_FRAGMENT_ABS)" >/dev/null

build: $(TOYBOX_SRC)/toybox

$(TOYBOX_SRC)/toybox: $(TOYBOX_SRC)/.config
	@if [ ! -x "$(MUSL_CC)" ]; then \
		echo "musl compiler wrapper not found: $(MUSL_CC)" >&2; \
		echo "Run 'make musl' first so musl/sysroot/bin/musl-gcc is installed." >&2; \
		exit 1; \
	fi
	@cd $(TOYBOX_SRC) && make -j"$(JOBS)" CC="$(MUSL_CC)" HOSTCC="$(HOSTCC)" CFLAGS="$(TOYBOX_CFLAGS)" LDFLAGS="$(TOYBOX_LDFLAGS)"

echo: $(OUT_DIR)/echo

$(OUT_DIR)/echo: $(TOYBOX_SRC)/toybox
	@mkdir -p "$(OUT_DIR)"
	@cp "$(TOYBOX_SRC)/toybox" "$@"
	@chmod 0755 "$@"
	@echo "Built $@"

clean:
	@if [ -d "$(TOYBOX_SRC)" ]; then \
		$(MAKE) -C "$(TOYBOX_SRC)" clean || true; \
	fi
	@rm -f "$(OUT_DIR)/echo"

distclean: clean
	@rm -rf "$(TOYBOX_SRC)" "$(OUT_DIR)" "$(CURDIR)"/toybox-*.tar.gz

info:
	@echo "toybox setup"
	@echo "  version:      $(TOYBOX_VERSION)"
	@echo "  source:       $(CURDIR)/$(TOYBOX_SRC)"
	@echo "  output:       $(OUT_DIR)/echo"
	@echo "  config:       $(CONFIG_FRAGMENT_ABS)"
	@echo "  MUSL_CC:      $(MUSL_CC)"
	@if [ -f "$(OUT_DIR)/echo" ]; then \
		echo "  status:       echo built"; \
	elif [ -d "$(TOYBOX_SRC)" ]; then \
		echo "  status:       source fetched"; \
	else \
		echo "  status:       not fetched"; \
	fi
