# Contributing

Thanks for looking. FerroLibri is a small project; an issue or a pull request is welcome.

- **Before a big change,** open an issue to agree on the idea. The app aims to stay a small, fast client for a Calibre library: browse, search, convert, send to a reader. Changes that grow it into something else may be declined.
- **Build and test** as described in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). Before you send a pull request run `scripts/check-readiness.ps1` and `scripts/check-ui.ps1` (or at least `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`). CI runs the same on Windows, Linux and macOS.
- **Keep a change focused,** with tests where there is logic, and describe in the pull request what changed and what you tried. Say honestly what you could not test (for example a real reader).
- **The library is read-only.** Code must not write to the Calibre library except through `calibredb`.
- **Plugins:** the plugin API is described in [docs/PLUGINS.md](docs/PLUGINS.md) and is versioned; a change to it needs an API version or an addition that old plugins can ignore. Example plugins and instructions in this repository are only for sources that publish their catalogs for programs and offer books lawfully. Pull requests with plugins or instructions for sites that distribute books without the right to do so will be declined.
- **Changelog:** add a line under the unreleased version in [CHANGELOG.md](CHANGELOG.md) for anything a user would notice.

By contributing you agree that your work is released under the [MIT License](LICENSE).
