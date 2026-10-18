# Mock Library

A **test source** for checking FerroLibri itself, not a source of books. It makes no network requests: every search answers at once with the same made-up books, whatever the text. Use it to try the Search screen, paging, the details panel, the download icon, the plugin's settings form, dropped results and error messages without depending on a real site.

The one domain in the manifest, `mock.invalid`, never resolves (RFC 2606). The download addresses the plugin returns are on it, so a download starts, fails at once with a DNS error and adds nothing. There are no covers.

## Try it

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/mock-library --query anything
```

In the app: **Settings → Plugins → Install plugin…**, choose **Folder**, pick this folder. Then choose *Mock Library* on the Search screen.

The tests are in `crates/ferrolibri-plugins/tests/mock_library.rs`; the plan is `plans/plugins/mock-plugin.md` in the docs repository.

## The data

A search returns 45 books, `book-01` … `book-45`, in pages of 10, 20 (default) or 50 (setting **Results per page**: 5, 3 or 1 pages). The text of the query is not used, only echoed in the description (`Mock answer to "…", page N.`).

| Book | What it shows |
| --- | --- |
| `book-01` | An ordinary book: all fields, a multi-line description, files in EPUB, FB2 (zip), MOBI and PDF |
| `book-02` | Cyrillic text; FB2 (zip) and EPUB |
| `book-03` | A series with an index; AZW3 |
| `book-04` | No authors, year or files: only *Open page* |
| `book-05` | 20 authors (the limit) |
| `book-06` | A title of exactly 300 characters |
| `book-07` | 10 files (the limit), all seven formats; `book-07-fail` makes `resolveDownload` throw, `book-07-blocked` returns an address that is not allowed |
| `book-08`, `book-09` | Japanese with an emoji; Arabic (right to left) |
| `book-10` | 30 tags and a description of exactly 8000 characters with line breaks and tabs |
| `book-11` | `pt-BR`, an ISBN-10 with hyphens, year 2100, a fractional series index |
| `book-12` | Only `id` and `title` |
| `book-13` … `book-45` | Alike; every third has no files, the rest have one or two |

## Service queries

A query that starts with `!` (case and the spaces around do not matter) gives an edge case. Any other text gives the regular set.

| Query | Answer | What to look for in the app |
| --- | --- | --- |
| `!empty` | no results | the empty state |
| `!single` | one result, no next page | no paging control |
| `!max` | 100 results | the most a page may hold |
| `!invalid` | 3 good results and 8 that break a rule | only the good ones are shown; the log has 8 warnings |
| `!toomany` | 101 results | the whole page is rejected |
| `!reject` | `hits` is not an array | the whole page is rejected |
| `!huge` | an answer over 2 MiB | an error from the plugin |
| `!error` | an exception | `Mock Library: Mock Library failed on purpose…` in the app |
| `!log` | one result | log lines at all four levels |

## Details and downloads

* `details` adds a publisher and a longer description; a book without files gets two (EPUB and FB2 zip) — **Offer files for download** off means none. `book-04` gets `{}`, `book-12` gets `null` (nothing to add), `book-05` throws, `book-06` returns year 0, which breaks the contract after the merge: the result stays as it was and an error is shown.
* `resolveDownload` returns `https://mock.invalid/files/<offer id><extension>` (`archive: "zip"` for FB2 zip). A download therefore ends with a network error; `book-07-fail` and `book-07-blocked` end earlier, with the plugin's error and with "address not allowed".

## Settings

| Setting | Meaning |
| --- | --- |
| **Results per page** | `10`, `20` or `50` |
| **Offer files for download** | off: results come without files, so no download icon |
| **Tag added to every result** | text added as the last tag (a book that already has 30 tags keeps them) |
