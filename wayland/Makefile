# ==========================================================================
# libwayland (+ libffi) build glue for PeetOS userspace
# ==========================================================================
#
# Produces, under out/:
#   bin/wayland-scanner       host scanner (runs on the build machine)
#   include/                  wayland-*.h, generated protocol headers, ffi.h
#   lib/libwayland-server.a   static, musl, x86_64
#   lib/libwayland-client.a
#   lib/libffi.a
#   lib/libwlprotocols.a       generated interface code for wayland-protocols
#                              (xdg-shell), headers in include/
#   share/wayland/wayland.xml

OUT_DIR ?= $(CURDIR)/out
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)

.PHONY: all fetch build clean distclean info

all: build

fetch:
	./fetch-wayland.sh

wayland-src libffi-src wayland-protocols-src:
	$(MAKE) fetch

build: wayland-src libffi-src wayland-protocols-src
	../scripts/build_wayland.sh \
		--wayland-src "$(CURDIR)/wayland-src" \
		--libffi-src "$(CURDIR)/libffi-src" \
		--protocols-src "$(CURDIR)/wayland-protocols-src" \
		--out "$(OUT_DIR)" \
		--musl-cc "$(MUSL_CC)"

clean:
	rm -rf "$(OUT_DIR)" build

distclean: clean
	rm -rf wayland-src libffi-src wayland-protocols-src wayland-*.tar.xz libffi-*.tar.gz

info:
	@echo "OUT_DIR=$(OUT_DIR)"
	@echo "MUSL_CC=$(MUSL_CC)"
	@ls -1 "$(OUT_DIR)/lib" 2>/dev/null || echo "(not built)"
