# EVerest image

`Dockerfile` builds upstream EVerest at a pinned SHA for arm64 (Pi 3) with the
everestrs path enabled, adds `module/RsGreenUpEvse` and the crates it uses to
the tree, and packages the result on Debian 12 slim.

```sh
# from the repo root; Apple silicon builds arm64 natively
docker build --platform linux/arm64 -f docker/everest/Dockerfile -t everest-greenup .

# bump the pin or try a canary against upstream main
docker build --platform linux/arm64 -f docker/everest/Dockerfile \
  --build-arg EVEREST_REF=main -t everest-greenup:canary .
```

CI: `.github/workflows/everest-image.yml` builds on `ubuntu-24.04-arm`, pushes
to GHCR on `main`, and runs a weekly canary against EVerest `main`.

## What the build checks

- every ELF file under `/opt/everest` resolves its shared libraries;
- `manager --check` accepts `config/config-spike-rust.yaml` (two Rust modules
  with a requirement between them and a C++ store), so the everestrs code
  generation, the cxx bridge and the module install layout all work;
- `manager --check` accepts `config/config-greenup-sil.yaml` and the
  `RsGreenUpEvse` binary is installed.

## How the module gets in

The module directory is copied to `modules/RsGreenUpEvse` in the EVerest tree
and `ev_add_module(RsGreenUpEvse)` is appended to `modules/CMakeLists.txt`;
EVerest's CMake then symlinks it into the Cargo workspace it generates. The
crates it depends on are copied to `greenup/` in the tree together with the
root `Cargo.toml`, so their `workspace = true` keys resolve, and the module's
relative crate paths are rewritten to that location with `sed`.

The generated workspace has no lockfile, so cargo resolves every dependency
fresh at build time. `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`
makes it skip releases that need a newer rustc than `RUST_VERSION` (the first
build without it picked `uuid` 1.27, which wants 1.89).

## Pins (build args)

| Arg | Value | Why |
| --- | --- | --- |
| `EVEREST_REF` | upstream `main` at 4afd42e0 (2026-10-02) | 2026.02.x predates the `evse_manager` changes we must honour; move to `2026.10.0` once tagged |
| `BUILD_KIT_TAG` | `v1.6.0` | upstream build-kit (Debian 12, gcc 12); the arm64 variant works but prints an "unsupported architecture" banner |
| `RUST_VERSION` | `1.86.0` | upstream's pin in `MODULE.bazel`; everestrs needs >= 1.86 |
| `CXXBRIDGE_VERSION` | `1.0.194` | must equal the `cxx` crate version in `lib/everest/framework/everestrs/everestrs/Cargo.toml` |
| `CMAKE_VERSION` | `3.31.10` | see below |
| `CXX_WARNING_FLAGS` | `-Wno-error=array-bounds ...` | see below |
| `EVEREST_MODULES` | allowlist | only what our configs use: the SIL set plus `RsExample*` for the spike config |
| `EVEREST_EXCLUDE_LIBS` | ocpp, iso15118, slac, ... | see below |

## Upstream quirks worked around

Upstream only CI-tests the Rust path with Bazel, and the CMake path shows
it. Four things needed fixing before this built:

- `cmake/everest-generate.cmake` builds the Rust link-dependency file with
  `$<LIST:JOIN,...>`, which needs CMake 3.27 or newer. The build-kit ships
  3.25.1, so a newer 3.x is pip-installed in front of it.
- The same file writes that list with `echo -e`. Ninja runs it through
  `/bin/sh`, which is dash on Debian, so the file starts with `-e ` and
  everestrs' `build.rs` fails with "Cannot find library path
  '-e /…/libeverestrs_sys.a'". `patches/0001-everestrs-link-deps-posix-sh.patch`
  switches it to `printf "%s\n"`. Worth sending upstream.
- Without an OCPP module in `EVEREST_INCLUDE_MODULES`,
  `lib/everest/ocpp_module_common` still configures and fails on
  `everest::ocpp_conversions`. `EVEREST_EXCLUDE_LIBS` leaves it out, together
  with the libraries no module of ours needs (`ocpp`, `iso15118`, `cbv2g`,
  `slac`, `ieee2030_1_1`, `can_dpm1000`). Drop `ocpp` and
  `ocpp_module_common` from that list when `OCPP201` joins the module list.
- `lib/everest/io` builds with `-Werror`, and gcc 12 on arm64 at `-O3` flags
  `std::vector` code in `mdns.cpp` with `-Warray-bounds`, a false positive.
  Upstream CI sets no build type, so it never sees this. `CXX_WARNING_FLAGS`
  turns that family back into plain warnings (`-Wno-error=array-bounds` and
  friends) through `CMAKE_CXX_FLAGS`.

Python and JavaScript module support are off too (`EVEREST_ENABLE_PY_SUPPORT`,
`EVEREST_ENABLE_JS_SUPPORT`). Nothing we run needs them, and they pull in
pybind11 and node.

## Runtime image

Debian 12 slim, same glibc as the build-kit. `manager` is the entrypoint;
configs live in `/opt/everest/etc/everest/`. The MQTT broker is not in this
image (Balena runs mosquitto as its own service).
