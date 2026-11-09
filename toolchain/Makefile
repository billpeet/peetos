# ==========================================================================
# PeetOS C/C++ cross toolchain (GCC targeting x86_64-linux-musl)
# ==========================================================================
#
# Produces out/bin/x86_64-linux-musl-g++ and the C++ runtime (libstdc++,
# libgcc_s, libatomic) in out/x86_64-linux-musl/lib64. See docs/cxx.md.

.PHONY: all fetch build clean distclean info

TOOLCHAIN_STAMP := out/bin/x86_64-linux-musl-g++

all: build

fetch:
	./fetch-toolchain.sh

gcc-src:
	$(MAKE) fetch

../musl/sysroot/lib/libc.a:
	$(MAKE) -C ../musl all

../musl/shared/lib/libc.so:
	$(MAKE) -C ../musl shared

build: $(TOOLCHAIN_STAMP)

$(TOOLCHAIN_STAMP): gcc-src ../musl/sysroot/lib/libc.a ../musl/shared/lib/libc.so
	./build-toolchain.sh

clean:
	rm -rf out sysroot build

distclean: clean
	rm -rf gcc-src gmp-src mpfr-src mpc-src gcc-*.tar.xz gmp-*.tar.xz mpfr-*.tar.xz mpc-*.tar.gz

info:
	@if [ -x "$(TOOLCHAIN_STAMP)" ]; then $(TOOLCHAIN_STAMP) --version | head -n 1; \
	  ls -1 out/x86_64-linux-musl/lib64; else echo "(not built)"; fi
