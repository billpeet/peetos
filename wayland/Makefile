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
#   lib/libxkbcommon.a         keymap compiler/state machine, headers in
#                              include/xkbcommon/
#   lib/libdrm.a, libdrm.so.2 libdrm core (xf86drm, xf86drmMode); headers in
#                              include/ (xf86drm*.h) and include/libdrm/
#   stage/                     staged /usr + /etc sysroot for meson builds:
#                              everything above plus expat, freetype,
#                              fontconfig, pixman, harfbuzz (C++, built
#                              with ../toolchain's g++), utf8proc and the
#                              DejaVu fonts
#                              (scripts/build_render_stack.sh), then
#                              tllist, fcft and foot (scripts/build_foot.sh)
#   lib/lib*.so.N, lib/lib*.so shared builds of libffi, libwayland-server,
#                              libwayland-client and libxkbcommon (need
#                              ../musl/shared from `make -C musl shared`)
#   share/X11/xkb/             verified xkeyboard-config subset (staged into
#                              the ext2 image at /usr/share/X11/xkb)
#   share/wayland/wayland.xml

OUT_DIR ?= $(CURDIR)/out
MUSL_CC ?= $(abspath ../musl/sysroot/bin/musl-gcc)

.PHONY: all fetch build clean distclean info

all: build

fetch:
	./fetch-wayland.sh

wayland-src libffi-src wayland-protocols-src libxkbcommon-src libdrm-src pixman-src expat-src freetype-src fontconfig-src dejavu-src harfbuzz-src utf8proc-src tllist-src fcft-src foot-src:
	$(MAKE) fetch

../musl/shared/lib/libc.so:
	$(MAKE) -C ../musl shared

# harfbuzz is C++: it needs the PeetOS cross g++ and C++ runtime.
../toolchain/out/bin/x86_64-linux-musl-g++: ../musl/shared/lib/libc.so
	$(MAKE) -C ../toolchain

build: wayland-src libffi-src wayland-protocols-src libxkbcommon-src libdrm-src pixman-src expat-src freetype-src fontconfig-src dejavu-src harfbuzz-src utf8proc-src tllist-src fcft-src foot-src ../musl/shared/lib/libc.so ../toolchain/out/bin/x86_64-linux-musl-g++
	../scripts/build_wayland.sh \
		--wayland-src "$(CURDIR)/wayland-src" \
		--libffi-src "$(CURDIR)/libffi-src" \
		--protocols-src "$(CURDIR)/wayland-protocols-src" \
		--out "$(OUT_DIR)" \
		--musl-cc "$(MUSL_CC)"
	../scripts/build_xkbcommon.sh \
		--xkbcommon-src "$(CURDIR)/libxkbcommon-src" \
		--out "$(OUT_DIR)" \
		--musl-cc "$(MUSL_CC)"
	../scripts/build_libdrm.sh \
		--libdrm-src "$(CURDIR)/libdrm-src" \
		--out "$(OUT_DIR)" \
		--musl-cc "$(MUSL_CC)"
	../scripts/build_render_stack.sh
	../scripts/build_foot.sh

clean:
	rm -rf "$(OUT_DIR)" build

distclean: clean
	rm -rf wayland-src libffi-src wayland-protocols-src libxkbcommon-src libdrm-src pixman-src expat-src freetype-src fontconfig-src dejavu-src harfbuzz-src utf8proc-src tllist-src fcft-src foot-src wayland-*.tar.xz libffi-*.tar.gz libxkbcommon-*.tar.xz libdrm-*.tar.xz pixman-*.tar.gz expat-*.tar.xz freetype-*.tar.xz fontconfig-*.tar.xz dejavu-*.tar.bz2 harfbuzz-*.tar.xz utf8proc-*.tar.gz tllist-*.tar.gz fcft-*.tar.gz foot-*.tar.gz

info:
	@echo "OUT_DIR=$(OUT_DIR)"
	@echo "MUSL_CC=$(MUSL_CC)"
	@ls -1 "$(OUT_DIR)/lib" 2>/dev/null || echo "(not built)"
