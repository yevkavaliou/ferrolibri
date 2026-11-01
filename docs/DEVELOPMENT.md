# Developing FerroLibri

This page is for people who build, test or change FerroLibri. Users should start with the [README](../README.md); plugin authors with the [plugin guide](PLUGINS.md).

## Workspace

| Crate | What it is |
| --- | --- |
| `crates/ferrolibri-core` | The library: scanning the Calibre library, metadata, conversion, reader detection and transfer, the background job executor, `calibredb` integration. No UI. |
| `crates/ferrolibri-plugins` | The plugin contract, the HTTP layer all network access goes through, the QuickJS sandbox, the store of installed plugins, downloads. Also the `ferrolibri-plugin-check` tool. |
| `crates/ferrolibri-ui` | The [iced](https://iced.rs) desktop client. The executable is `ferrolibri`. |
| `examples/plugins` | Two example plugins (`open-library`, `opds-catalog`). |
| `vendor/` | Two patched iced crates, see [vendor/README.md](../vendor/README.md). Excluded from the workspace. |
| `scripts/` | Verification scripts and two helpers (`make-ui-fixture.py` builds a synthetic library for manual checks, `make-ui-icon.py` generates the app icon). |

## Build and run

Rust 1.93.1. On Linux install the Wayland/X11, xkbcommon and Vulkan development packages listed in [`.github/workflows/ui.yml`](../.github/workflows/ui.yml).

```
cargo run -p ferrolibri-ui --release       # run the app
cargo build --release                       # build the whole workspace
cargo install --path crates/ferrolibri-ui  # install the executable `ferrolibri`
```

The UI is not published on crates.io because it depends on the patched iced crates in `vendor/`. A `justfile` has `gui-run` and `gui-build` shortcuts.

## Verification

```powershell
pwsh -File scripts/check-readiness.ps1
pwsh -File scripts/check-ui.ps1
```

`check-readiness.ps1` runs the core and plugin tests, strict Clippy and the native volume enumeration smoke; `-RealConverters` additionally tests an installed Calibre with a synthetic TXT → EPUB → KEPUB conversion, and `-Ui` also runs `check-ui.ps1`. `check-ui.ps1` checks formatting, builds the UI, runs Clippy and the tests (a headless UI simulator and real-core send/cancel/conflict flows), and tests the plugin crate. CI (`readiness.yml`, `ui.yml`) runs these on Windows, Ubuntu 24.04 and macOS and does a release build of the default workspace.

CI uses fixture folders as readers. Tests marked `#[ignore]` need something CI does not have (an attached MTP Kindle, an installed Calibre or `ebook-convert`, the internet) and are run by hand, for example:

```
cargo test -p ferrolibri-core native_mtp_roundtrip_and_cancel -- --ignored --nocapture
```

## What gets scanned

The scanner walks `{library}/{author}/{title (id)}/` directories and recognizes AZW3, DOCX, EPUB, KEPUB, MOBI, PDF and TXT. Extensions are case-insensitive; `.kepub.epub` is distinct from EPUB. Unknown formats (including CBZ), covers, OPF and temporary files are not book inputs. Each book has a stable ID and an ordered list of exact file paths.

Excluded from scanning: `.caltrash/`, `.calnotes/`, `.DS_Store` and `downloaded/` (Calibre internal), and `metadata.opf` and `cover.jpg` inside book folders. A missing OPF falls back to directory metadata; a corrupt OPF is an error.

## The library is read-only

The core never writes to the Calibre library, never changes `metadata.db`, OPF, covers or source books. The one exception is adding a downloaded book through `calibredb` (`calibre.rs`). Conversion results live in a separate temporary workspace owned by the application; explicit output directories must be outside the library, and an existing output file is a conflict error.

## Readers and transfers

Detection enumerates mounted local filesystems on Windows, Linux and macOS and checks reader signatures; a volume label or a `documents` directory alone does not identify a reader. On Windows, MTP readers are found through SetupAPI and written through the Windows portable-device API (WPD); on Linux and macOS MTP is unavailable and FUSE-mounted MTP is deliberately ignored. Network storage is unsupported.

The core supports explicit selection by device ID and `device::manual_device(path, kind)` for a validated mount root. Kobo prefers KEPUB, then EPUB; Kindle prefers AZW3, then MOBI and can prepare AZW3 using Calibre.

Transfers to a drive copy into a temporary file on the destination and publish without overwriting. Cancellation and write failures are reported; if a disconnection prevents cleanup, the error includes the partial file path. Root mount points are never recreated. MTP uploads are verified by object size, reverted on cancellation, and a failed upload removes only the object it created; MTP has no atomic publish, so a file with the same name written by another program at the same moment is outside the app's protection.

## Errors and jobs

Core operations return a typed `Error` with an `ErrorKind`; scans separate root errors from per-book warnings. `JobExecutor` runs scans, metadata reads, conversions, transfers and external jobs (such as plugin downloads) on bounded worker threads. It exposes job IDs, states and events, reserves the paths and devices a job uses so conflicting jobs never run together, and supports cancellation. Copy progress reports bytes, conversion progress reports stages. Shutdown cancels work and joins the workers, but may wait for an uninterruptible OS call.

## Releasing

A release is made by pushing a tag. `.github/workflows/release.yml` runs on tags `v*` and builds the Windows x64 program with the locked dependencies, then publishes a GitHub release with `ferrolibri-<tag>-windows-x64.zip` (the executable, README, LICENSE, CHANGELOG and `THIRD_PARTY_LICENSES.txt`) and `SHA256SUMS.txt`. The release notes are the section of `CHANGELOG.md` for that version; a version starting with `0.` is marked as a pre-release.

1. Put the changes under a `## <version>` heading in `CHANGELOG.md` and set `workspace.package.version` in `Cargo.toml` (and the lock file) to the same version.
2. Make sure CI is green on the commit, merge it to `main`.
3. `git tag v<version>` on that commit and `git push origin v<version>`. The workflow stops if the tag and the workspace version differ or the changelog has no section for the version.

`scripts/third-party-licenses.py` writes the licenses of the libraries linked into the executable (it can be run by hand: `python scripts/third-party-licenses.py out.txt`). Only Windows x64 is built; the program is not code-signed.

## Plugins

The contract, the sandbox and the limits are described for authors in [PLUGINS.md](PLUGINS.md); the code is in `crates/ferrolibri-plugins`. `ferrolibri-plugin-check` checks a plugin folder, and `crates/ferrolibri-plugins/tests/examples.rs` shows how to test a plugin without the internet using a local test server (`ferrolibri_plugins::testing`).

```
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/open-library --query dracula
```
