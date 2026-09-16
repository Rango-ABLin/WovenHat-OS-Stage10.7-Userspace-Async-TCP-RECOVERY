# WovenHat OS 0.4.0 Stage 6 — clean-build fix

This patch addresses the diagnostics found after enabling `warnings = "deny"`.

## Genuine code fix

`network::endpoint_to_packed` previously used `let ... else` on `IpAddress`. In the current smoltcp configuration only IPv4 is compiled, so the pattern is irrefutable and the `else` branch is unreachable. The function now uses a direct irrefutable binding.

## Intentional compatibility/test APIs

The following items remain in the source tree for compatibility, diagnostics, or later stages but are not referenced by the current shell-first runtime. They are marked individually with `#[allow(dead_code)]` so `warnings = "deny"` remains enabled for all other warnings:

- `CachedDevice::misses`
- `fat32::create_root_file`
- `hal::pci::read_config_dword`
- `paging::clone_user_range_in`
- `pipe::buffer_size`
- `storage::fat32_writable`
- `vfs::OpenFile`
- selected `virtio_net::Stats` diagnostic fields
- `virtio_net::Transport::location`
- reserved `SocketError` variants

The unused VirtIO descriptor flag `DESC_F_NEXT` was removed.

## Build

```powershell
cargo clean
cargo build
```

A successful build is expected to have no WovenHat warnings because the crate-level lint policy still denies warnings.
