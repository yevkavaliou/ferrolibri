# Changelog

## 0.8.0 (unreleased)

The first release with a stable shape: a native desktop client for a Calibre library. It is a 0.x release, not a complete Calibre replacement.

### Added

- Browse a Calibre library as a list or a cover grid; live search across author, title and format; details from `metadata.opf`.
- Convert books with Calibre's `ebook-convert` and/or kepubify.
- Send books to Kobo and Kindle readers mounted as drives, and, on Windows, to readers connected over MTP, with a progress bar, a queue and cancellation.
- Plugins (plugin API version 1): extra search sources on the Search screen, with downloads added to the library through `calibredb`. Examples for Open Library and a Project Gutenberg OPDS catalog; the author's guide is [docs/PLUGINS.md](docs/PLUGINS.md).
- Settings for appearance (text and cover size, window opacity, blur) and plugins; the window size is remembered.

### Changed

- The executable is now `ferrolibri` (it was `ferrolibri-gui`).

### Removed

- The terminal (TUI) client and the `scan_metadata` diagnostic tool.
