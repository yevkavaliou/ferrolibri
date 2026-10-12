# Vendored iced crates

Unmodified copies of the crates.io sources, with the small changes marked
`RULIBRE PATCH`. Both crates are MIT licensed by the iced authors
(<https://github.com/iced-rs/iced>). Wired in through `[patch.crates-io]` in the
workspace `Cargo.toml`; the directory is excluded from the workspace so
formatting and lints do not touch it.

| Crate | Version | Change |
|---|---|---|
| `iced_wgpu` | 0.14.0 | Build the wgpu instance with `BackendOptions::from_env_or_default()` so `WGPU_DX12_PRESENTATION_SYSTEM` is honoured, and choose premultiplied alpha when that variable is set (DirectComposition swap chains reject straight alpha). |
| `iced_winit` | 0.14.1 | On Windows, create a transparent window with `WS_EX_NOREDIRECTIONBITMAP` when `WGPU_DX12_PRESENTATION_SYSTEM` is set. Without it an opaque white redirection surface sits under the swap chain. |

Both changes are inactive unless `WGPU_DX12_PRESENTATION_SYSTEM` is set; the
application sets it (with `WGPU_BACKEND=dx12`) only on Windows, see
`crates/rulibre-ui/src/blur.rs`. Drop the patches when upstream exposes these
options (iced window/compositor settings).

To re-vendor after an iced upgrade: copy `src`, `Cargo.toml` and `README.md`
of the new release, re-apply the three marked hunks.
