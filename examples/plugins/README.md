# Example plugins

Three plugins for FerroLibri's **Search** screen, each for a source that publishes its catalog for programs to use. They are examples of how a plugin is written ([the guide](../../docs/PLUGINS.md) explains every part); they are not a list of recommended sites, and the project keeps no registry of plugins.

| Folder | Source | What it shows |
| --- | --- | --- |
| [`open-library`](open-library/) | [Open Library](https://openlibrary.org) JSON search | The smallest plugin: one request, one JSON answer, paging by number, covers; no downloads |
| [`google-books`](google-books/) | [Google Books API](https://developers.google.com/books/docs/v1/using) | An optional API key as a setting, HTML cleaned from descriptions, paging by index, a clear message when the quota is used up |
| [`opds-catalog`](opds-catalog/) | [Project Gutenberg](https://www.gutenberg.org) over [OPDS](https://specs.opds.io) | XML feeds, paging by a link, downloads (`details` lists the files, `resolveDownload` names the address), settings that point the plugin at another catalog |

## Try one

From a checkout, with the folder of a plugin:

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/open-library --query dracula
```

`ferrolibri-plugin-check` reads the plugin the way the app does when it installs it, runs `search` over the real network and prints the results, the warnings, and how many requests and how much time the call took (see [the guide](../../docs/PLUGINS.md#checking-a-plugin)).

To use a plugin in the app: **Settings → Plugins → Install plugin…**, choose **Folder**, pick the plugin's folder and leave **Turn on after installing** ticked. The source switch appears on **Search**.

## Copy one

A plugin is a folder with `plugin.toml` and `plugin.js` and nothing else. Copy the example closest to your source, change the `id` and the `domains`, and run the checker after every change.
