# Bootloader build isolation

Source: crates.io `bootloader` 0.11.17, upstream commit
`ec8a8b4b59bd94f3c0280adc1bcdae530251b003`.
Published crate SHA-256:
`d861319d747a01da5d4d680500b7f808607f902a286b476bd51f991612e39d8e`.
The upstream MIT and Apache-2.0 licenses are retained.

The only code change is in the UEFI installer command in `build.rs`: give the
nested Cargo invocation its own target and intermediate build directories under
`OUT_DIR`. Otherwise the inherited project-local `CARGO_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR` can make the installer wait indefinitely for an artifact
lock held by the parent build. Both child directories remain project-local.
The bootloader version, image-generation code, and runtime code are unchanged.
The BIOS build is not enabled by WovenHat and is unchanged.

Package build inputs and upstream tests are retained; upstream repository-only
CI/editor configuration and documentation folders are omitted. Cargo excludes
this dependency from the workspace. The root crates.io patch selects it for
the host image builder; this is not a replacement for the kernel's bootloader
API dependency.
