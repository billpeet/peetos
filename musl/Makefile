# ==========================================================================
# musl libc build glue for PeetOS (Rust port)
# ==========================================================================

MUSL_VERSION ?= 1.2.5
MUSL_SRC ?= musl-src
SYSROOT ?= $(CURDIR)/sysroot
TARGET ?= x86_64-linux-musl
CROSS_PREFIX ?=

ifneq ($(origin CC),default)
MUSL_CC := $(CC)
endif
ifneq ($(origin AR),default)
MUSL_AR := $(AR)
endif
ifneq ($(origin RANLIB),default)
MUSL_RANLIB := $(RANLIB)
endif

CONFIGURE_ARGS := --musl-dir "$(CURDIR)/$(MUSL_SRC)" --sysroot "$(SYSROOT)" --target "$(TARGET)"
ifneq ($(strip $(CROSS_PREFIX)),)
CONFIGURE_ARGS += --cross-prefix "$(CROSS_PREFIX)"
endif
ifneq ($(strip $(MUSL_CC)),)
CONFIGURE_ARGS += --cc "$(MUSL_CC)"
endif
ifneq ($(strip $(MUSL_AR)),)
CONFIGURE_ARGS += --ar "$(MUSL_AR)"
endif
ifneq ($(strip $(MUSL_RANLIB)),)
CONFIGURE_ARGS += --ranlib "$(MUSL_RANLIB)"
endif

.PHONY: all fetch configure build install shared clean distclean info

all: install

fetch:
	./fetch-musl.sh --version "$(MUSL_VERSION)" --output-dir "$(CURDIR)"

configure: $(MUSL_SRC)/config.mak

$(MUSL_SRC)/config.mak: $(MUSL_SRC)
	./configure-musl.sh $(CONFIGURE_ARGS)

$(MUSL_SRC):
	$(MAKE) fetch

build: $(MUSL_SRC)/lib/libc.a

$(MUSL_SRC)/lib/libc.a: $(MUSL_SRC)/config.mak
	$(MAKE) -C $(MUSL_SRC)

install: $(SYSROOT)/lib/libc.a

# Shared libc / dynamic loader (ld-musl-x86_64.so.1), built out of tree into
# shared/ so the static sysroot is untouched. See scripts/build_musl_shared.sh.
shared: shared/lib/libc.so

shared/lib/libc.so: $(MUSL_SRC)
	../scripts/build_musl_shared.sh

$(SYSROOT)/lib/libc.a: $(MUSL_SRC)/lib/libc.a
	$(MAKE) -C $(MUSL_SRC) install

clean:
	@if [ -d "$(MUSL_SRC)" ]; then \
		$(MAKE) -C "$(MUSL_SRC)" clean || true; \
	fi

distclean: clean
	@rm -rf "$(MUSL_SRC)" "$(SYSROOT)" "$(CURDIR)"/musl-*.tar.gz

info:
	@echo "musl setup"
	@echo "  version:       $(MUSL_VERSION)"
	@echo "  source:        $(CURDIR)/$(MUSL_SRC)"
	@echo "  sysroot:       $(SYSROOT)"
	@echo "  target:        $(TARGET)"
	@echo "  cross-prefix:  $(CROSS_PREFIX)"
	@if [ -f "$(SYSROOT)/lib/libc.a" ]; then \
		echo "  status:        built"; \
	elif [ -d "$(MUSL_SRC)" ]; then \
		echo "  status:        source fetched"; \
	else \
		echo "  status:        not fetched"; \
	fi
