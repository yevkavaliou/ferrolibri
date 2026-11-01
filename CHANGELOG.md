# Changelog

## 0.8.0 (unreleased)

The first public release: a native desktop client for a Calibre library. It is a 0.x release, not a complete Calibre replacement.

### Added

- Browse a Calibre library as a list or a cover grid; live search across author, title and format; details from `metadata.opf`.
- Convert books with Calibre's `ebook-convert` and/or kepubify.
- Send books to Kobo and Kindle readers mounted as drives, and, on Windows, to readers connected over MTP, with a progress bar, a queue and cancellation.
- Plugins (plugin API version 1): extra search sources on the Search screen, with downloads added to the library through `calibredb`. Examples for Open Library and a Project Gutenberg OPDS catalog; the author's guide is [docs/PLUGINS.md](docs/PLUGINS.md).
- Settings for appearance (text and cover size, window opacity, blur) and plugins; the window size is remembered.

### Known issues

- The disk cache of cover thumbnails is not size-limited yet; on a very large library it can grow to hundreds of megabytes. It lives in the user's cache directory and can be deleted at any time.
- Keyboard shortcuts on macOS are written with Ctrl; whether Cmd should be used instead is not decided.
- Only a Kindle over MTP on Windows has been tried on a real device (see the README).

### Changed

- The executable is now `ferrolibri` (it was `ferrolibri-gui`).
- Versions before 0.8.0 (0.1.x) were releases of the terminal client this project started from; there is no earlier release of the desktop client.

### Removed

- The terminal (TUI) client and the `scan_metadata` diagnostic tool.
