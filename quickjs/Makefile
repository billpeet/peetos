# ==========================================================================
# quickjs build glue for PeetOS userspace staging
# ==========================================================================

QUICKJS_VERSION ?= 2024-01-13
QUICKJS_SRC ?= quickjs-src
OUT_DIR ?= $(CURDIR)/out
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)
CONFIG_VERSION ?= $(QUICKJS_VERSION)-peetos
QUICKJS_ARCHIVE ?=

FETCH_ARGS := --version "$(QUICKJS_VERSION)" --output-dir "$(CURDIR)"
ifneq ($(strip $(QUICKJS_ARCHIVE)),)
FETCH_ARGS += --archive "$(QUICKJS_ARCHIVE)"
endif

.PHONY: all fetch build qjs clean distclean info

all: qjs

fetch:
	./fetch-quickjs.sh $(FETCH_ARGS)

$(QUICKJS_SRC):
	$(MAKE) fetch

build: $(OUT_DIR)/qjs

$(OUT_DIR)/qjs: $(QUICKJS_SRC) ../scripts/build_quickjs_qjs.sh peetos/Makefile.peetos peetos/repl_stub.c
	../scripts/build_quickjs_qjs.sh \
		--quickjs-src "$(CURDIR)/$(QUICKJS_SRC)" \
		--out "$@" \
		--musl-cc "$(MUSL_CC)" \
		--version "$(QUICKJS_VERSION)" \
		--config-version "$(CONFIG_VERSION)"

qjs: build

clean:
	@if [ -d "$(QUICKJS_SRC)" ] && [ -f "$(QUICKJS_SRC)/Makefile.peetos" ]; then \
		$(MAKE) -C "$(QUICKJS_SRC)" -f Makefile.peetos clean || true; \
	fi
	@rm -f "$(OUT_DIR)/qjs"

distclean: clean
	@rm -rf "$(QUICKJS_SRC)" "$(OUT_DIR)" "$(CURDIR)"/quickjs-*.tar.xz

info:
	@echo "quickjs setup"
	@echo "  version:      $(QUICKJS_VERSION)"
	@echo "  source:       $(CURDIR)/$(QUICKJS_SRC)"
	@echo "  output:       $(OUT_DIR)/qjs"
	@echo "  MUSL_CC:      $(MUSL_CC)"
	@if [ -f "$(OUT_DIR)/qjs" ]; then \
		echo "  status:       qjs built"; \
	elif [ -d "$(QUICKJS_SRC)" ]; then \
		echo "  status:       source fetched"; \
	else \
		echo "  status:       not fetched"; \
	fi
