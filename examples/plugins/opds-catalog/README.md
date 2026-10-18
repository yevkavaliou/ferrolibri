# OPDS catalog (Project Gutenberg)

Searches an [OPDS](https://specs.opds.io) catalog; by default [Project Gutenberg](https://www.gutenberg.org), whose books are in the public domain in the USA. It is the example with **downloads**: select a result and its files (EPUB, Kindle) appear as download offers.

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/opds-catalog --query dracula
cargo run -p ferrolibri-plugins --example run_plugin -- examples/plugins/opds-catalog details "{\"id\":\"https://www.gutenberg.org/ebooks/345.opds\",\"title\":\"Dracula\"}"
```

How it works with Project Gutenberg:

1. `search` asks `/ebooks/search.opds/?query=…`. The feed lists books, but each only links to a feed of its own (`/ebooks/345.opds`); "Authors", "Subjects" and "Bookshelves" entries are skipped. The next page is the feed's `rel="next"` link, used as the cursor.
2. `details` reads the book's feed: the annotation, the language, the subjects, a cover and the files (each edition of the book is an entry; the plugin lists the files of all of them, at most 10).
3. `resolveDownload` returns the address of the file. The app downloads it (the address redirects to `…/cache/epub/…`), checks the bytes, and adds the book to the Calibre library.

Settings:

* **Catalog address** — another OPDS catalog. The host of the address you enter (and its subdomains) is then allowed; nothing else is.
* **Search path** — where the catalog searches, with `{searchTerms}` where the typed text goes. The default is Project Gutenberg's. A catalog that lists the files directly in the search feed (an `acquisition` link in every entry) works without `details`.

The plugin knows EPUB, Kindle (MOBI), PDF and FB2 in a zip; it does not offer anything else a catalog lists. It makes no attempt to check what a catalog is allowed to give away: that is the user's decision for the catalog they point it at.
