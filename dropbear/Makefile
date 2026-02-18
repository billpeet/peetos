# ==========================================================================
# dropbear build glue for PeetOS userspace staging
# ==========================================================================

DROPBEAR_VERSION ?= 2024.86
DROPBEAR_SRC ?= dropbear-src
OUT_DIR ?= $(CURDIR)/out
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)
DROPBEAR_ARCHIVE ?=
JOBS ?= $(shell nproc)
PROGRAMS ?= dropbear dropbearkey dbclient scp

FETCH_ARGS := --version "$(DROPBEAR_VERSION)" --output-dir "$(CURDIR)"
ifneq ($(strip $(DROPBEAR_ARCHIVE)),)
FETCH_ARGS += --archive "$(DROPBEAR_ARCHIVE)"
endif

.PHONY: all fetch build dropbear clean distclean info

all: dropbear

fetch:
	./fetch-dropbear.sh $(FETCH_ARGS)

$(DROPBEAR_SRC):
	$(MAKE) fetch

build: $(OUT_DIR)/dropbear $(OUT_DIR)/dropbearkey

$(OUT_DIR)/dropbear $(OUT_DIR)/dropbearkey: $(DROPBEAR_SRC) ../scripts/build_dropbear_multi.sh peetos/localoptions.h
	../scripts/build_dropbear_multi.sh \
		--dropbear-src "$(CURDIR)/$(DROPBEAR_SRC)" \
		--out-dir "$(OUT_DIR)" \
		--musl-cc "$(MUSL_CC)" \
		--jobs "$(JOBS)" \
		--programs "$(PROGRAMS)"

dropbear: build

clean:
	@if [ -d "$(DROPBEAR_SRC)" ] && [ -f "$(DROPBEAR_SRC)/Makefile" ]; then \
		$(MAKE) -C "$(DROPBEAR_SRC)" clean >/dev/null 2>&1 || true; \
	fi
	@rm -rf "$(OUT_DIR)"

distclean: clean
	@if [ -d "$(DROPBEAR_SRC)" ] && [ -f "$(DROPBEAR_SRC)/Makefile" ]; then \
		$(MAKE) -C "$(DROPBEAR_SRC)" distclean >/dev/null 2>&1 || true; \
	fi
	@rm -rf "$(DROPBEAR_SRC)" "$(CURDIR)"/dropbear-*.tar.bz2

info:
	@echo "dropbear setup"
	@echo "  version:      $(DROPBEAR_VERSION)"
	@echo "  source:       $(CURDIR)/$(DROPBEAR_SRC)"
	@echo "  output:       $(OUT_DIR)"
	@echo "  programs:     $(PROGRAMS)"
	@echo "  MUSL_CC:      $(MUSL_CC)"
	@if [ -f "$(OUT_DIR)/dropbear" ]; then \
		echo "  status:       dropbear built"; \
	elif [ -d "$(DROPBEAR_SRC)" ]; then \
		echo "  status:       source fetched"; \
	else \
		echo "  status:       not fetched"; \
	fi
