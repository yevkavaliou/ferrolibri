# FerroLibri

A fast desktop app for browsing your [Calibre](https://calibre-ebook.com/) library and getting books onto your e-reader. It opens the library Calibre already made, in place: nothing to import, nothing to migrate, and Calibre keeps working with the same folder.

> **Status: 0.x, work in progress.** FerroLibri already browses a library, converts and sends books to a reader, and searches extra sources through plugins, but it is not a complete Calibre replacement. See [Status and limits](#status-and-limits) for what has and has not been tried on real devices.

![FerroLibri screenshot](assets/screenshot.png)

## What it does

- **Browse** the whole library as a list or a grid of covers, sorted by author and title.
- **Search** as you type across author, title and format.
- **Details** for the selected book from its `metadata.opf`: publisher, date, language, series, tags, description, identifiers.
- **Convert** books to another format with Calibre's `ebook-convert` or [kepubify](https://pgaskin.net/kepubify/). Only formats you do not already have are offered.
- **Send to a reader.** Kobo and Kindle are recognized when connected; the right format is picked or prepared, and you see progress, a queue, and can cancel. FerroLibri never overwrites a file on the device.
- **Find more books with plugins** (optional): extra search sources on the Search screen, with a download button that adds the book to your Calibre library.

FerroLibri does not edit your library. It never changes `metadata.db`, covers, existing book files or metadata; the only way a book gets in is through Calibre's own `calibredb` when you download one with a plugin.

## Install

There are no ready-made downloads yet, so build it from source. You need [Rust](https://rustup.rs/) (1.93.1):

```
cargo install --path crates/ferrolibri-ui
ferrolibri
```

On Linux you also need the Wayland/X11, xkbcommon and Vulkan development packages; the exact list is in [`.github/workflows/ui.yml`](.github/workflows/ui.yml). To remove it: `cargo uninstall ferrolibri-ui`.

On first start FerroLibri asks for the folder of your Calibre library and remembers it.

## Using it

- **Keyboard:** `/` search, arrows move, Shift extends the selection, Ctrl+A selects all, `T` sends to the reader, `C` opens Convert, Ctrl+B / Ctrl+I show or hide the side panes, Esc closes or clears.
- **Readers:** connect the reader; it appears in the reader bar. With several readers you choose one, and for a reader that mounts as a drive you can also pick its folder by hand. Kobo prefers KEPUB, then EPUB; Kindle prefers AZW3, then MOBI, and FerroLibri can prepare AZW3 with Calibre.
- **Conversion tools:** none are needed to browse, search, or send a book the reader can already open. Converting needs Calibre's `ebook-convert` and/or kepubify; the Convert dialog tells you what is missing. Converted files are kept in a temporary workspace of the app, outside your library.

### Plugins

A plugin adds a search source: a website or catalog FerroLibri can ask. Install one in **Settings → Plugins → Install plugin…**, from a folder or from the address of a GitHub repository. A plugin is two small files, runs in a sandbox, and can reach only the sites it declares; the install dialog lists them. Downloading a book into your library needs Calibre to be installed. Without it you can still search and open a result's page.

The repository has two [examples](examples/plugins/README.md): Open Library (information only) and an OPDS catalog (Project Gutenberg, public-domain books, with downloads). Writing your own is described in the [plugin author's guide](docs/PLUGINS.md).

#### Disclaimer

FerroLibri's plugin interface is a neutral tool: it lets a program you install ask a website or catalog you choose and show the answer. The project does not provide, review, endorse, host or support third-party plugins, keeps no registry of them, and has no control over the sites they ask or the files those sites offer. The plugins and instructions in this repository are for sources that publish their catalogs for programs to use and offer books lawfully (Open Library and Project Gutenberg's public-domain collection); the project does not accept plugins or instructions for sites that distribute books without the right to do so.

You install and run a plugin at your own risk and are responsible for what you search for, download and add to your library: copyright and a site's terms of use differ from country to country and from site to site, and it is for you to make sure you may download or use a book. The software is provided "as is", without warranty of any kind (see the MIT License); the authors are not liable for what a third-party plugin does, for any damage it causes, or for the use of any plugin or source.

## Status and limits

- **Readers:** a real device has been tried only with a **Kindle Paperwhite Signature Edition over MTP on Windows** (detection, send, byte-for-byte read-back, cancellation). **Kobo readers have not been tried on a real device**, and neither has a reader that mounts as a USB drive (older Kindles included): those paths are covered by tests with fixture folders only.
- **Reader connections:** USB drives on Windows, Linux and macOS; MTP on Windows only. Network readers are not supported.
- **Systems:** run by hand only on Windows 11. Windows, Ubuntu 24.04 and macOS are built and tested in CI, but nobody has used the window on Linux or macOS. Window blur is Windows-only; other systems get an opaque window.
- **Plugins:** plugin API version 1 is the first published one; see [how it is versioned](docs/PLUGINS.md#api-versions-and-compatibility).
- **Unsigned binaries:** future release executables will not be code-signed, so Windows SmartScreen may warn.

See the [changelog](CHANGELOG.md) for what each version brings.

## For developers

FerroLibri is a Rust workspace: a core library (scanning, metadata, conversion, device detection and transfer, background jobs), a plugin runtime and the iced desktop client. Build and test instructions, the layout of the crates and how the core behaves are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md); plugins are covered by the [plugin author's guide](docs/PLUGINS.md).

## Credits and license

FerroLibri started from the core of [Rulibre](https://github.com/Glydric/Rulibre) by Leonardo Migliorelli (MIT). It is an independent project with its own name, iced UI and core changes; it is not affiliated with or endorsed by the original author.

Released under the [MIT License](LICENSE), which keeps the original copyright notice. The vendored iced crates in `vendor/` keep their own MIT license ([`vendor/LICENSE-iced`](vendor/LICENSE-iced)). Other dependencies are used unmodified under their own licenses (mostly MIT/Apache-2.0; a few crates are MPL-2.0).
