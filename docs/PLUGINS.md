# Writing a FerroLibri plugin

This guide is for people who write plugins. It covers what a plugin is, how to write, check and publish one, every field and limit of plugin API version 1, and how the API is versioned. The types for your editor are in [`ferrolibri-plugin.d.ts`](ferrolibri-plugin.d.ts). If this guide and the app disagree, the app is right: please report it.

* [What a plugin is](#what-a-plugin-is)
* [A first plugin in ten minutes](#a-first-plugin-in-ten-minutes)
* [The manifest, `plugin.toml`](#the-manifest-plugintoml)
* [The script, `plugin.js`](#the-script-pluginjs)
* [The `host` object](#the-host-object)
* [What a search returns](#what-a-search-returns)
* [What happens when the user downloads](#what-happens-when-the-user-downloads)
* [Limits](#limits)
* [Habits that keep a plugin working](#habits-that-keep-a-plugin-working)
* [Checking a plugin](#checking-a-plugin)
* [Debugging](#debugging)
* [Publishing a release on GitHub](#publishing-a-release-on-github)
* [API versions and compatibility](#api-versions-and-compatibility)
* [Safety rules](#safety-rules)
* [Which sources the project writes plugins for](#which-sources-the-project-writes-plugins-for)

## What a plugin is

A plugin adds a **source** to FerroLibri's Search screen: a website or catalog the user can search. When at least one plugin is installed, the Search screen gets a source switch. The user types a query; FerroLibri calls your plugin's `search` function; your plugin asks the site and returns results in FerroLibri's own format; FerroLibri draws them. When a result offers a file, a download icon appears next to it, and a click downloads the book and adds it to the Calibre library (through Calibre's `calibredb`, which must be installed).

A plugin is **two files** and nothing else: `plugin.toml` (what the plugin is and what it may reach) and `plugin.js` (one JavaScript module). There are no archives, no extra files and no build step.

The plugin runs in a sandbox (QuickJS). It cannot read files, open sockets, or see the user's library. Its only link with the world is the global `host` object, and everything it returns is treated as untrusted and checked. Version 1 of the API has one kind of plugin, `search`.

## A first plugin in ten minutes

1. Copy the example closest to your source from [`examples/plugins/`](../examples/plugins/README.md) to a new folder. `open-library` is the smallest; `opds-catalog` shows downloads.
2. In `plugin.toml` change `id`, `name`, `description` and `network.domains` (the sites you will ask).
3. In `plugin.js` change the address and the code that turns the site's answer into results.
4. Run the checker after every change:

   ```text
   cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- path/to/your-plugin --query "war and peace"
   ```

5. In the app: **Settings → Plugins → Install plugin…**, choose **Folder**, pick the folder and leave **Turn on after installing** ticked. Then choose the new source on the Search screen.

The example folders also have a README with the details of each source.

## The manifest, `plugin.toml`

```toml
id = "my-library"                 # 2-40 characters: a-z, 0-9, "-"
name = "My Library"
version = "0.1.0"                 # semver of your plugin
api_version = 1                   # the plugin API version this plugin is written for
provides = ["search"]
min_app_version = "0.8.0"         # optional: the oldest FerroLibri that can run it
author = "Your Name"
homepage = "https://github.com/you/my-library-plugin"
license = "MIT"
description = "Searches My Library by title, author or ISBN."
search_hint = "Search by title, author or ISBN"   # shown under the search field
content = ["metadata"]            # optional, a claim of yours; the app does not check it
entry = "plugin.js"               # optional; the script's name, ending in .js
sha256 = "..."                    # optional: SHA-256 of the script; the app refuses a script that differs

[network]
domains = ["catalog.example.org", "*.cdn.example.org"]

[limits]
call_timeout_s = 30               # optional, 5-60

[[settings]]
key = "base_url"
type = "url"                      # string | url | bool | choice
label = "Mirror address"
network = true                    # url only: the user's address is then allowed
default = "https://catalog.example.org"

[[settings]]
key = "order"
type = "choice"
label = "Order"
options = ["relevance", "newest"] # choice only
default = "newest"
```

`id`, `name`, `version`, `api_version` and `provides` are required. Unknown fields, and unknown values in `provides` and `content`, are ignored, so a newer manifest still reads on an older app. Two things are not ignored: an `api_version` the app does not know (the rest of the manifest is not read at all and the plugin is reported as incompatible, see [API versions and compatibility](#api-versions-and-compatibility)), and an unknown setting `type` (a parse error).

Lengths are counted in Unicode characters. Single-line fields cannot be empty or contain control characters.

| Field | Limit |
| --- | --- |
| `plugin.toml` | 64 KiB |
| `id` | 2–40 characters `a-z`, `0-9`, `-`; the first is not `-`. Names reserved by Windows (`con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`) are refused on install |
| `name` | 60 |
| `version`, `min_app_version` | semver |
| `author` | 100 |
| `license` | 60 |
| `description` | 500 |
| `homepage` | `http`/`https`, 2000, no user name or password |
| `entry` | a file name without a path, ending in `.js` |
| `sha256` | 64 hexadecimal digits |
| `provides`, `content` | 10 items of up to 40 characters each |
| `search_hint` | 120 |
| `network.domains` | 20 entries |
| `limits.call_timeout_s` | 5–60, default 30 |
| `settings` | 20 |
| `settings[].key` | `[a-z][a-z0-9_]{0,39}`, unique |
| `settings[].label` | 80 |
| `settings[].default` | by type: `string` up to 500 characters; `url` is `http`/`https`; `bool` is `true`/`false`; `choice` is one of the `options` |
| `settings[].options` | `choice` only, required: 1–20 unique strings of up to 80 characters |
| `settings[].network` | `url` only |

### Network domains

`network.domains` is the **whole list** of sites the plugin may talk to: the sites it asks, **and the sites its covers and files redirect to**. A request to anything else fails with `blocked_domain`, and a result whose `cover_url` is on an unlisted site is dropped.

* `catalog.example.org` is exactly that host. `*.example.org` is any subdomain (`a.example.org`, `a.b.example.org`) but **not** `example.org` itself; list both when you need both. Case does not matter.
* Only names: ASCII host names (use punycode for international names), no IP addresses, single-label names such as `localhost`, ports, paths or schemes. `*.` is allowed only as a prefix and needs at least two more labels (`*.org` is refused).
* Requests to a listed domain are `https` on port 443 and only to public addresses.
* Follow the redirects yourself before you decide on the list. Open Library's covers, for example, are on `covers.openlibrary.org`, which redirects to `archive.org`, which redirects to `ia600505.us.archive.org`; the list has to name all of them (`covers.openlibrary.org`, `archive.org`, `*.archive.org`). The checker drops such a result with a warning, as the app does.

### Settings

The app draws the settings form for you. The values arrive in the `ctx.settings` argument: what the user saved, or else the `default`. A setting with neither is absent.

* `string`, `bool`, `choice` hold what the user types or picks.
* A `url` setting with `network = true` lets the user point the plugin at **their own server or a mirror**: the host of the address they **save** (and its subdomains) is allowed, over any scheme and port, even a private address. This is how a plugin for a self-hosted catalog works without a fixed domain. A `default` is *not* an address the user entered: it reaches `ctx.settings`, but it does not open the network, so name the default site in `network.domains` too.
* Do not ask for passwords. Version 1 has no login and no secure storage; anything entered in a setting is kept as plain text in the app's settings file. An API key that identifies *the user* to a service is the most you should ask for, and your plugin's README should say where it is stored and where it is sent.

## The script, `plugin.js`

An ES module with up to three exported **synchronous** functions. (An `async` function returns a `Promise`, and the call fails.)

```js
export function search(query, ctx) {
  // query: { text, page, cursor }    ctx: { settings: { ... } }
  const reply = host.fetch("https://catalog.example.org/api?q=" + encodeURIComponent(query.text));
  const data = JSON.parse(reply.body);
  return {
    hits: data.items.map((item) => ({ id: item.id, title: item.title, authors: item.authors })),
    next_cursor: data.hasMore ? String(query.page + 1) : null,
  };
}

export function details(hit, ctx) {       // optional
  // Called when the user selects a result. Returns the fields the result lacks.
  return { description: "...", cover_url: "https://..." };
}

export function resolveDownload(offer, ctx) {   // optional
  // Called only when the user clicks the download icon of an offer.
  return { url: "https://catalog.example.org/files/" + offer.id + ".epub" };
}
```

* **`search`** receives `query.text`, `query.page` (from 1) and `query.cursor` (absent on the first page). Return `{ hits, next_cursor }`. `next_cursor` is any string up to 500 characters that *you* will understand: the app hands it back in `query.cursor` when the user asks for the next page. Use the page number, the index of the next item, or the link of the next page. `null`, a missing value or an empty string means there is no next page.
* **`details`** receives the result as `search` returned it and returns the missing fields. Every field that is not `null` *replaces* the result's field (`offers` replaces the whole list); `id` is never changed. The merged result is checked again against the same limits; if it fails, the result stays as it was and the user sees the error. Return `null` or `{}` for "nothing to add". Use `details` for what costs another request, such as the annotation or the list of files (the `opds-catalog` example does this).
* **`resolveDownload`** receives one offer and returns `{ url }`, and for an FB2 file packed in a zip also `archive: "zip"` (it overrides the offer's own `archive`). `url` must be `http`/`https`, on an allowed domain, without a user name or password. The app downloads that address itself. A plugin cannot fetch binary data, and it cannot name the file: the app names it from the book's title and the format.

Every call runs in a **fresh runtime**: global variables do not survive between calls, so do not cache. What a function returns must be serialisable as JSON (`undefined` and functions read as `null`). The language is ECMAScript (QuickJS): `JSON`, `RegExp`, `Date`, `Map`, `Set`, typed arrays, `Promise`, `BigInt`, `encodeURIComponent`, classes, generators and so on are there. There is no `console`, `fetch`, `setTimeout`, `require` or `process`, no `import` (static or dynamic), and **`eval`, `new Function` and the `AsyncFunction`/`GeneratorFunction` constructors are disabled**. Everything fits into the one file.

## The `host` object

| Call | What it does |
| --- | --- |
| `host.fetch(url, { headers })` | `GET` the address and return `{ status, url, headers, body }`. Any HTTP status comes back as a normal reply, including 404 and 500: check `status` yourself. `url` is the final address after redirects: resolve relative links against it. `body` is UTF-8 text. |
| `host.url.resolve(base, ref)` | Resolve a relative or `//host/path` link against an absolute `base`, as a browser does (WHATWG URL rules). An empty `ref` gives `base`; spaces and non-ASCII characters are encoded. A `base` that is not absolute throws a `TypeError`. |
| `host.html.select(html, selector)` | Parse HTML (as forgivingly as a browser) and return the elements that match a CSS selector, in document order, as `{ text, html, attrs }`: the text of the element, its markup including itself, its attributes by name. A bad selector or a broken limit throws a `TypeError`. |
| `host.xml.parse(text)` | Parse XML into the root element `{ name, ns, attrs, text, children }`. `name` is the local name without a prefix, `ns` the namespace URI or `null`, `attrs` the attributes by local name, `text` all the text of the element including nested elements. DTDs and entities are refused, external entities are never loaded; a parse error throws a `TypeError`. |
| `host.log(level, message)` | Write to the app's log (`debug`, `info`, `warn`, `error`; another level throws a `TypeError`). A message is cut to 2000 characters; a call may write 200. |

About `host.fetch`:

* **GET only**, no request body, no cookies, no proxy, no automatic retries. A `method` other than `"GET"` throws a `TypeError`. Only the headers `Accept`, `Accept-Language`, `Content-Type`, `Referer` and `Range` are sent. Anything else is silently left out and noted in the plugin's log. Response header names are lower-case, repeated headers are joined with `", "`, `set-cookie` is not included. Bytes that are not UTF-8 become U+FFFD; binary replies are not supported.
* A network failure is **thrown** as an `Error` with a `kind`: `timeout`, `connect` (no connection, bad address, more than 5 redirects), `dns`, `tls`, `blocked_domain` (the address is not on your list, or it is a private address, or the scheme or a user name in the address is not allowed), `too_large` (a body over 8 MiB), `too_many_requests` (more than 20 requests in one call) or `cancelled`. If you do not catch it, the user sees `network error (kind): ...`. Catch it when you can say something more useful. Whether to retry is up to the plugin.
* The app identifies requests with `User-Agent: FerroLibri/<version> plugin/<id>` and puts a pause (250 ms) between requests to one host. Be a good citizen: use few requests.
* Limits that apply: 20 requests per call, 8 MiB per reply. HTML parsing: input up to 8 MiB, nesting up to 512, up to 5000 matches and 8 MiB of text and markup in the answer. XML parsing: input up to 8 MiB, up to 200 000 elements.

## What a search returns

Each result is an object; extra fields are ignored. Only `id` and `title` are required: give what the source knows, nothing else. `null`, an empty or blank optional string and a missing field all mean "not given".

| Field | Type | Limit |
| --- | --- | --- |
| `id` | string, required | up to 200 characters; unique within the page (a repeat is dropped). You get it back in `details` |
| `title` | string, required | up to 300 |
| `authors` | array of strings | up to 20 names of up to 120 characters |
| `description` | string | up to 8000; line breaks and tabs are allowed |
| `language` | string | a BCP 47 tag (`en`, `pt-BR`), up to 35 |
| `year` | integer | 1–2100 |
| `isbn` | string | ISBN-10 or ISBN-13; hyphens and spaces are fine, the check digit is not verified |
| `publisher`, `series` | string | up to 200 |
| `series_index` | number | 0–100000 |
| `tags` | array of strings | up to 30 of up to 60 characters |
| `cover_url` | string | `http`/`https`, up to 2000; the host must be allowed |
| `page_url` | string | `http`/`https`, up to 2000; the "Open page" button |
| `offers` | array | up to 10, see below |

Single-line text fields cannot contain control characters. `description`, `language`, `year`, `isbn`, `publisher`, `series`, `series_index` and `tags` are shown in the details panel; title, authors, ISBN, tags, series and language are also handed to Calibre when a file is downloaded.

An **offer** is a file the user can download:

| Field | Type | Limit |
| --- | --- | --- |
| `id` | string, required | up to 200; unique within the result. Whatever you need in `resolveDownload`, often the file's address |
| `format` | string, required | exactly `EPUB`, `FB2`, `MOBI`, `AZW3`, `PDF`, `TXT` or `DOCX` |
| `archive` | string | only `"zip"`, and only with `format = "FB2"` (a `.fb2.zip` file) |
| `size` | integer | bytes, 1 to 2 GiB |
| `label` | string | up to 100 |

The download icon appears for a result that **has offers**; there is no separate flag. A result without offers shows an "Open page" button only. A plugin that finds metadata but has no download addresses (the `open-library` example) is perfectly good.

What the app checks in your answer: every field against the limits above, the cover address against your domains, `id` uniqueness. A result that breaks one rule is **dropped, with a warning in the log**; the other results stay. The whole page is rejected if the answer is not an object with a `hits` array, has more than 100 results, is bigger than 2 MiB, or has a `next_cursor` that is not a string or is longer than 500 characters.

## What happens when the user downloads

A click on the download icon of an offer queues a job. Knowing what it does helps you decide what to return:

1. The app calls your `resolveDownload` and checks the address against your domains.
2. It downloads the file itself, as a stream, with the same network rules as your plugin (every redirect is checked; the limit is 200 MiB; a status other than 2xx is an error). The file name comes from the book's title (cleaned, at most 80 characters) and the extension of the format; a name from the plugin, the address or `Content-Disposition` is never used.
3. It checks the **bytes**, not the `Content-Type`: EPUB and DOCX must be a zip with the right marker file; PDF starts with `%PDF`; MOBI and AZW3 have `BOOKMOBI` at offset 60; FB2 is not HTML and contains `<FictionBook`; an FB2 zip holds exactly one `.fb2` file within the size limit; TXT is not HTML. A site that answers with an HTML page where a book should be is reported as "the site did not return a book", and nothing is saved.
4. It adds the book with `calibredb add`, passing the title, authors, ISBN, tags, series and language of your result (not the publisher, year, description or cover). A book that is already in the library is reported as such. Without `calibredb` the download is not accepted: "Calibre is needed".
5. The temporary files are removed whatever the outcome.

The app does not unpack archives: a `.fb2.zip` goes to Calibre as it is.

## Limits

| What | Limit |
| --- | --- |
| Time of one call (including loading the module) | `call_timeout_s`, 30 s by default, 5 to 60. More than 30 is shown to the user when installing |
| Memory / stack | 64 MiB / 1 MiB |
| Requests per call | 20 |
| Body of one reply | 8 MiB |
| Answer of the plugin | 2 MiB of JSON (nesting up to 128 levels), 100 results, 10 offers per result |
| Script / manifest | 1 MiB / 64 KiB |
| Log | 200 messages per call, 2000 characters each |
| HTML parsing | input 8 MiB, depth 512, 5000 matches |
| XML parsing | input 8 MiB, 200 000 elements |
| A downloaded file | 200 MiB |

## Habits that keep a plugin working

* **Check the status.** `host.fetch` does not throw on 404, 429 or 500. Throw an `Error` with a sentence the user can act on: *"Open Library asks to slow down (status 429). Try again in a minute."* The app shows `Plugin name: message`; it never shows your stack.
* **Say what to do.** If a service needs a key or is full, say so in the message.
* **Clean what you return.** Descriptions often arrive as HTML: strip tags and entities. Cut lists to the limits. *Real data breaks limits:* Open Library returned works with 57 author names, and the 20-author limit would have dropped them; the example keeps the first five distinct names. Run the checker on several queries, not one.
* **Don't make many requests.** 20 per call, one host at a time, 250 ms apart: a call with 20 requests takes five seconds before the sites even answer. Put what costs a request in `details` (it runs for the one result the user opens), not in `search` for all the results.
* **Make cursors self-contained.** The cursor is all that comes back with the next page (plus the query text). A link to the next page is a good cursor.
* **Don't parse a page when there is an API.** OPDS, JSON and XML feeds change less than HTML. If you must read HTML, use `host.html.select` with selectors that rely on structure, not on style classes.

## Checking a plugin

`ferrolibri-plugin-check` is the tool to run before every release. From a checkout of the repository:

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- <plugin folder> [options]
```

| Option | Meaning |
| --- | --- |
| `--query TEXT` | Run `search` with this text over the **real network**. Without it only the files are checked. |
| `--page N` / `--cursor TEXT` | The page number (default 1) and the cursor of an earlier page. |
| `--setting key=value` | Store a setting as the form would. An address stored for a `url` setting with `network = true` is allowed; a `default` is not (as in the app). Repeat for several settings. |
| `--raw` | Also print the plugin's answer as JSON, before the contract check. |

It does what the app does, in this order:

1. Reads `plugin.toml` and the script as the installer does and prints the name, the id and version, whether the plugin is compatible with this FerroLibri, the domains, the settings, the time per call and the **SHA-256 of the script** (paste it into `sha256` if you want the app to refuse a changed file).
2. With `--query`, runs `search` in the same sandbox, with the plugin's real allow-list, and validates the answer against the contract.
3. Prints the number of accepted and dropped results, the first ten results (title, authors, year, `[formats]`, `+cover`), the plugin's log and **the cost**: `cost: 2 request(s) of 20 allowed per call, 1.43 s of 30 s allowed`. The time includes the pause between requests to one host.

```text
Open Library (open-library 0.1.0), plugin API 1
  compatible with FerroLibri 0.8.0: yes
  network domains: openlibrary.org, covers.openlibrary.org, archive.org, *.archive.org
  ...
Search "dracula", page 1:
  20 result(s) accepted, 0 dropped, next page: yes
   1. Dracula - Bram Stoker (1897) +cover
  ...
  cost: 1 request(s) of 20 allowed per call, 0.69 s of 30 s allowed

Result: OK
```

**Problem** lines (`PROBLEM:`) make the exit code 1: an incompatible or unreadable manifest, a script that does not match its `sha256`, a plugin that throws or times out, an answer that the app would reject as a whole. **Warnings** do not: dropped results (each with the reason), `warn` and `error` lines of the log, no results at all, no network domains, a call that used more than three quarters of the request allowance or two thirds of the time. A good release has neither. Exit code 2 means the command line was wrong.

The checker runs only `search`. To try `details` or `resolveDownload` with a result as JSON:

```text
cargo run -p ferrolibri-plugins --example run_plugin -- <plugin folder> details '{"id":"...","title":"..."}'
cargo run -p ferrolibri-plugins --example run_plugin -- <plugin folder> resolveDownload '{"id":"...","format":"EPUB"}'
```

The checker needs the internet for a real search; it has no mode that fakes a site. To test a plugin without the internet (for example in CI), serve saved answers with a local server and use `--setting` to point a `url` setting at it; `crates/ferrolibri-plugins/tests/examples.rs` does exactly this for the two examples.

## Debugging

* `host.log("info", "got " + data.items.length + " items")` writes to the app's log under the name `plugin`. The checker prints the log of its run. In the app the lines go to `ferrolibri.log` next to `ui.toml` and, when there is a console, to stderr; run with `RUST_LOG=warn,plugin=debug` to see `host.log` lines too (at the default level only warnings and errors appear). A failed search or `details` call is always logged as a warning with the plugin's id, and the search panel names the HTTP status of a 5xx answer when the plugin's message contains it ("answered with status 503").
* An exception in your code is reported as `Name: message`; its JavaScript stack, with the file and line (`plugin.js:15:9`), goes to the log. Everything that goes wrong inside a call (an exception, a syntax error, running out of time or memory, a missing exported function, an answer that is not JSON or is over 2 MiB) is reported to the user as an error of your plugin.
* `--raw` shows exactly what the plugin returned. If a result is missing, run without `--raw` too: the dropped results are listed with the rule they broke.
* If a call fails with "did not finish within N s", look at the cost line: each request waits for the earlier request to the same host and for the 250 ms pause.
* There is no debugger and no `console`; `host.log` is the way.

## Publishing a release on GitHub

Users install from a repository address (`https://github.com/you/my-library-plugin`) or from a link to `plugin.toml`. For a GitHub repository the app lists the **releases** and installs the one the user picks.

1. Set `version` in `plugin.toml` (semver) and, if you use it, update `sha256` (the checker prints it).
2. Run the checker once more and fix problems and warnings.
3. Create a release on GitHub. The tag can be anything (`v0.1.0`); it is shown to the user. **Attach two files as release assets: `plugin.toml` and the script** (`plugin.js`, or the name in `entry`). Nothing else is needed or used; the source archive GitHub adds is ignored.
4. Mark a release as a *pre-release* to hide it from users who did not ask for pre-releases. Drafts are not seen.

Details:

* The app reads `plugin.toml` of each release first and shows whether it is compatible with the user's FerroLibri (`api_version`, `min_app_version`) **before** it downloads the script. Do not rely on the tag for compatibility; set `min_app_version`.
* If GitHub reports a digest for the script asset, the app compares it; and `sha256` in the manifest is compared too.
* The repository's release list is read through GitHub's API without a login: 60 requests an hour per user. The app tells the user when the limit is used up.
* Another way: host `plugin.toml` and the script side by side on any `https` site and give the link to `plugin.toml`. The script is looked for in the same directory.
* An update replaces the files and keeps the user's settings that the new manifest still accepts. A setting that became `network = true` does not inherit the saved address: the user enters it again, on purpose.

## API versions and compatibility

Three version numbers appear in a manifest; do not mix them up:

| Field | What it is | Who changes it |
| --- | --- | --- |
| `version` | The version of **your plugin** (semver). | You, on every release. |
| `api_version` | The version of **the plugin API**, the contract described in this guide. A whole number. | The project, when the contract changes in an incompatible way. You set it to the version you wrote the plugin for. |
| `min_app_version` | The oldest FerroLibri that can run your plugin (semver). Optional. | You, when you rely on something the API gained in a later app. |

### Which versions exist

| API version | First app version | Status |
| --- | --- | --- |
| 1 | 0.8.0 | current |

An app runs the API versions it knows (version 1 today). A plugin is compatible when all of these hold:

* its `api_version` is one the app knows. If it is not, the rest of the manifest is **not read**, because a newer API may lay it out differently; the plugin is shown as incompatible with that reason;
* `min_app_version`, if present, is not newer than the app;
* `provides` contains `"search"`, the only kind of plugin version 1 has;
* every field is within the limits above.

The installer and `ferrolibri-plugin-check` list **all** the reasons at once, each a sentence a user can read.

### How the API changes

* **The API version changes only for an incompatible change:** removing or renaming a field or function, changing what it means, tightening a limit, a new *required* field or behaviour, a new setting `type`. FerroLibri does not make such a change silently inside a version, also while it is a 0.x program: it ships as a new `api_version`.
* **Within a version the API only grows in ways an old plugin can ignore:** new optional manifest fields, new optional fields of results, new functions on `host`, new optional exported functions, new `format` values, larger limits. A plugin that does not use the addition keeps working unchanged. Because older apps ignore unknown manifest fields, a plugin may use an addition and still load on an older app, but without the addition's behaviour; if the plugin needs it, set `min_app_version` to the first app version that has it so that older apps refuse the plugin with a clear reason instead of misbehaving.
* **Every change is written down:** the changelog of the project names the app version that introduced it, and this guide lists it under *Additions within a version* below and, for a new version, in a *Changes from version N* section.
* **A new version does not remove the old one at once.** The project intends to keep running the previous API version for at least one release after a new one appears, and to announce in the changelog the release that stops running it. A plugin whose `api_version` the app no longer knows is shown as incompatible, never run in a guessed way.

When a new API version appears, update your plugin by reading *Changes from version N*, changing the code, raising `api_version` and `min_app_version` together, and publishing a release. Users on an older app can still pick the earlier release of your plugin: the app shows whether each release is compatible with their FerroLibri before it downloads anything.

### Additions within a version

None yet: version 1 is as described in this guide.

## Safety rules

* The user decides what a plugin may reach by installing it: the install dialog lists the domains, any "your own address" settings and a call time over 30 s. Ask for as little as you need; a plugin with a wildcard for a whole service looks worse than one with three exact hosts.
* Everything a plugin returns is untrusted: lengths, address schemes, formats, even the downloaded bytes are checked. Do not rely on the app to repair your data; fix it in the plugin.
* A plugin cannot read the user's files or library, send anything but a `GET`, store anything between calls or run code it builds from strings.
* Do not collect or forward what the user searches for to any site other than the source the plugin is for, and do not put identifiers of the user or the machine into requests.
* **Do not hide behaviour.** The `description` is what the user reads before installing; it must say which site the plugin asks and what it does with the result.

## Which sources the project writes plugins for

FerroLibri's plugin API is **neutral**: it does not know or care what site a plugin talks to, and anyone can write and share a plugin. The project itself, though, takes a position on what it publishes:

* The examples in this repository, and the instructions in this guide, are for sources that **publish their catalogs for programs to use and distribute books lawfully**: Open Library (metadata) and Project Gutenberg (public domain) are the two.
* The project does **not** include plugins, instructions or examples for sites that distribute books without the right to do so, and it does **not** keep a registry or catalog of third-party plugins. A pull request that adds such a plugin or instructions to this repository will be declined.
* Plugins written by others are theirs: the project does not review, endorse, host or support them. See the disclaimer in the README.

When you write a plugin for a source, make sure you are allowed to use it the way the plugin does: read its terms and API rules, follow its rate limits and say in your README what the plugin does and what the user is responsible for.
