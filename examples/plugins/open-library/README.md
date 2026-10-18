# Open Library

Searches [Open Library](https://openlibrary.org) by title, author or ISBN through its [search API](https://openlibrary.org/dev/docs/api/search). A result has the title, up to five authors, the year of the first publication, subjects and a cover, and a link to the book's page. The plugin offers **no downloads**.

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/open-library --query "war and peace"
```

* One request per page of 20 results; the next page is the number of the page.
* Open Library asks programs to identify themselves and to be moderate; the app sends a `User-Agent` with the plugin's id and spaces requests to one host.
* The **Open Library address** setting is for a mirror; leave it as it is.
* The covers come from `covers.openlibrary.org`, which redirects to the Internet Archive (`archive.org`, `*.archive.org`); that is why those domains are in `plugin.toml`.
* Open Library lists the author names of *all* editions of a work, with repeats and misspellings; the plugin keeps the first five distinct names.
