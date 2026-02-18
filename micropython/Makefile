# ==========================================================================
# micropython build glue for PeetOS userspace staging
# ==========================================================================

MICROPY_VERSION ?= 1.24.1
MICROPY_SRC ?= micropython-src
OUT_DIR ?= $(CURDIR)/out
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)
MICROPY_VARIANT ?= minimal
MICROPY_ARCHIVE ?=
JOBS ?= $(shell nproc)

FETCH_ARGS := --version "$(MICROPY_VERSION)" --output-dir "$(CURDIR)"
ifneq ($(strip $(MICROPY_ARCHIVE)),)
FETCH_ARGS += --archive "$(MICROPY_ARCHIVE)"
endif

.PHONY: all fetch build micropython clean distclean info

all: micropython

fetch:
	./fetch-micropython.sh $(FETCH_ARGS)

$(MICROPY_SRC):
	$(MAKE) fetch

build: $(OUT_DIR)/micropython

$(OUT_DIR)/micropython: $(MICROPY_SRC) ../scripts/build_micropython_unix.sh
	../scripts/build_micropython_unix.sh \
		--micropython-src "$(CURDIR)/$(MICROPY_SRC)" \
		--out "$@" \
		--musl-cc "$(MUSL_CC)" \
		--variant "$(MICROPY_VARIANT)" \
		--jobs "$(JOBS)"

micropython: build

clean:
	@if [ -d "$(MICROPY_SRC)/ports/unix" ]; then \
		$(MAKE) -C "$(MICROPY_SRC)/ports/unix" clean >/dev/null 2>&1 || true; \
	fi
	@if [ -d "$(MICROPY_SRC)/mpy-cross" ]; then \
		$(MAKE) -C "$(MICROPY_SRC)/mpy-cross" clean >/dev/null 2>&1 || true; \
	fi
	@rm -f "$(OUT_DIR)/micropython"

distclean: clean
	@rm -rf "$(MICROPY_SRC)" "$(OUT_DIR)" "$(CURDIR)"/micropython-*.tar.gz

info:
	@echo "micropython setup"
	@echo "  version:      $(MICROPY_VERSION)"
	@echo "  source:       $(CURDIR)/$(MICROPY_SRC)"
	@echo "  output:       $(OUT_DIR)/micropython"
	@echo "  variant:      $(MICROPY_VARIANT)"
	@echo "  MUSL_CC:      $(MUSL_CC)"
	@if [ -f "$(OUT_DIR)/micropython" ]; then \
		echo "  status:       micropython built"; \
	elif [ -d "$(MICROPY_SRC)" ]; then \
		echo "  status:       source fetched"; \
	else \
		echo "  status:       not fetched"; \
	fi
