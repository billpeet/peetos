# C++ Toolchain and Runtime

PeetOS userspace can run C++ programs. A GCC 15.2 cross toolchain targets `x86_64-linux-musl` against the repo's musl. It ships libstdc++, libsupc++, libgcc_s (the unwinder) and libatomic as shared libraries in the ext2 image. C++ programs are dynamically linked PIEs that load through musl's loader, like the C programs in `docs/elf.md`.

**Where It Lives**
- `toolchain/fetch-toolchain.sh` pins and fetches the sources (SHA-256): GCC 15.2.0, GMP 6.3.0, MPFR 4.2.2 and MPC 1.3.1. GMP, MPFR and MPC are symlinked into the GCC tree and built in-tree.
- `toolchain/build-toolchain.sh` builds the toolchain. `make -C toolchain` runs both scripts.
- `toolchain/out/bin/x86_64-linux-musl-{gcc,g++}` are the cross compilers.
- `toolchain/out/x86_64-linux-musl/lib64/` holds the target runtime: `libstdc++.so.6`, `libgcc_s.so.1`, `libatomic.so.1`, plus static archives.
- `toolchain/sysroot/usr/{include,lib}` is the compiler's sysroot: musl's headers, startup objects, `libc.a`, and `libc.so` from `make -C musl shared`.
- `user/cxx_smoke.cpp`, `user/cxx/` and `scripts/build_cxx_smoke.sh` are the runtime smoke test, and `scripts/run_toybox_cxx_smoke.sh` and `tests/musl_ext2_cxx_smoke.sh` run it in QEMU (`make run-cxx-smoke`).

**Building**
```bash
make musl && make -C musl shared   # musl static sysroot and shared libc
make -C toolchain                  # fetch + build GCC (about 4 minutes on 32 cores)
make run-cxx-smoke
```
The build machine needs a host C/C++ compiler, `make`, `m4` (for GMP) and binutils. `build-essential` on Ubuntu covers this; `m4` is a separate package on some distributions.

**How It Is Built**
- **No binutils build.** PeetOS user programs are ordinary x86_64 ELF, so the build machine's GNU `as` and `ld` work for them. The script symlinks them (and `ar`, `nm`, `ranlib`, `strip`, `objcopy`, `objdump`, `readelf`) under target-prefixed names in `toolchain/out/bin`, which is where GCC's configure and driver look for them.
- **GCC configure.** `--target=x86_64-linux-musl --with-sysroot --enable-languages=c,c++ --enable-shared --enable-threads=posix --enable-tls --enable-__cxa_atexit --enable-default-pie --disable-multilib`. libsanitizer, libgomp, libquadmath, libitm, libssp, libvtv, gcov and NLS are disabled. Only `all-gcc`, libgcc, libstdc++-v3 and libatomic are built and installed (stripped).
- **Dynamic loader.** GCC's musl target already defaults to `/lib/ld-musl-x86_64.so.1` as `PT_INTERP`. The image builder installs musl's `libc.so` at that path.
- **Image.** `scripts/build_ext2_musl_execve.sh` copies `libstdc++.so.6`, `libgcc_s.so.1` and `libatomic.so.1` into `/usr/lib` whenever the toolchain has been built. The rendering stack also seeds `libstdc++`/`libgcc_s` into its staged sysroot (`wayland/out/stage`), so C programs that link a C++ library can resolve its runtime dependencies with `-rpath-link`.

**Using It**
- Compile with `toolchain/out/bin/x86_64-linux-musl-g++`. Add `-Wl,-z,max-page-size=0x1000` like the other user builds. The result is a PIE that needs `libstdc++.so.6`, `libgcc_s.so.1` and `libc.so`.
- Meson projects get the compiler through the `cpp` entry of `wayland/out/peetos-cross.ini` (`scripts/build_render_stack.sh`). C sources in those projects still use `musl-gcc`.
- `scripts/build_cxx_smoke.sh` is a minimal example of an executable, a C++ shared library it links, and a `dlopen()`ed plugin.

**First user: harfbuzz**
harfbuzz 10.1.0 is now built with this compiler (see `docs/rendering.md`), so fcft has text shaping and foot has grapheme clustering. harfbuzz is compiled without exceptions or RTTI and doesn't use the C++ standard library, so `libharfbuzz.so.0` doesn't need `libstdc++` at run time; what it needed was a C++ compiler. Later ports (Mesa, Qt, many applications) do need the full runtime.

**Testing**
`make run-cxx-smoke` (part of `make test`) boots the `musl-ext2-cxx-smoke` profile with 2 CPUs and runs `bin/cxx_smoke`, which links `libpeetcxx.so` and `dlopen()`s `libpeetcxxplugin.so`. It checks:
- **Static construction:** constructors in the executable and in a shared library run before `main` (once each); a static destructor runs through `__cxa_atexit` after `main` returns.
- **Exceptions:** throw/catch by base class; destructors run in every unwound frame; catch-all rethrow; `std::exception_ptr`; exceptions thrown in a shared library and in a `dlopen()`ed plugin caught by their exact type in the executable (type_info matches across objects); `std::out_of_range` from libstdc++; `std::bad_alloc` when a 1 TiB allocation exceeds the per-process mapping budget.
- **RTTI and dispatch:** `dynamic_cast`, `typeid` comparison, and virtual calls into objects created by a shared library and by the plugin.
- **Library:** containers and algorithms, `std::string` across a library boundary, `ostringstream` formatting, `std::format`, `shared_ptr`/`weak_ptr`/`std::function`, and `std::filesystem` directory iteration over `/usr/lib`.
- **Threads:** four `std::thread`s with atomics and `std::mutex`; per-thread `thread_local` values; `thread_local` destructors at thread exit; a function-local static initialized once under contention (`__cxa_guard_*`); `condition_variable` handoff; `std::async` results and exception propagation; `steady_clock` with `sleep_for`.
- **Plugins:** the plugin's static constructor runs at `dlopen`, and `dlclose` succeeds.

`tests/cxx_toolchain.sh` (also in `make test`) is the host-side check. It syntax-checks the toolchain scripts and confirms `build-toolchain.sh` stops cleanly without its sources. When the toolchain is built, it compiles a static C++ program (exceptions, threads, `std::format`) and runs it on the Linux build machine, then checks that a dynamic build is a PIE with the musl loader and the shared C++ runtime as dependencies.

The smoke needed no kernel changes: the syscalls libstdc++ uses (futexes, `clone(CLONE_THREAD)`, TLS, `clock_gettime`, `getdents64`, `mmap`) were already in place for musl pthreads and the Wayland stack.

Limitations / Simplifications
- The C++ standard library is GNU libstdc++, not LLVM libc++. There is no clang toolchain.
- The toolchain uses the build machine's binutils rather than a pinned binutils build, so the assembler/linker version depends on the host. It must be a GNU binutils that targets x86_64 ELF.
- C code in the meson builds is still compiled with `musl-gcc` (the host GCC with musl's specs file); only C++ goes through the cross g++. Both use the same musl headers and libc, but they are different compiler versions.
- Target libraries install to `lib64` (GCC's default OS directory for x86_64 without multilib), not `lib`.
- libsanitizer, libgomp (OpenMP), libquadmath, libitm and gcov are not built. Programs that need OpenMP or `__float128` maths won't link.
- No `libstdc++` debug-mode or pretty-printer support is staged, and the runtime libraries are stripped.
- The runtime is not versioned with the image: rebuilding the toolchain replaces `libstdc++.so.6` for every program on the next image build.
- `std::locale` beyond the "C" locale is untested; musl has limited locale support.
- The toolchain is rebuilt from scratch by `build-toolchain.sh` (no incremental rebuilds); `make -C toolchain` only skips it when `x86_64-linux-musl-g++` already exists.
