// A search plugin for an OPDS catalog, written against the shape of a real mirror
// (plans/plugins/research.md): searching is two steps (a menu, then the results), feeds use
// relative links, and a download is offered as a zip around an FB2 file.

// GET with a few tries: this kind of server answers 502/503 now and then.
function get(url) {
  for (let attempt = 1; ; attempt++) {
    let reply;
    try {
      reply = host.fetch(url, { headers: { Accept: "application/atom+xml" } });
    } catch (e) {
      if (["connect", "dns", "timeout", "tls"].includes(e.kind)) {
        throw new Error("Cannot connect to the catalog. Check its address in the plugin settings.");
      }
      throw e;
    }
    if ((reply.status === 502 || reply.status === 503) && attempt < 3) {
      host.log("warn", "status " + reply.status + " from " + url + ", trying again");
      continue;
    }
    if (reply.status !== 200) {
      throw new Error("The catalog answered with status " + reply.status);
    }
    return reply;
  }
}

const named = (node, name) => node.children.filter((child) => child.name === name);
const textOf = (node, name) => (named(node, name)[0] || { text: "" }).text.trim();

const FORMATS = {
  "application/fb2+zip": { format: "FB2", archive: "zip", label: "FB2 (zip)" },
  "application/epub+zip": { format: "EPUB", label: "EPUB" },
};

function toHit(entry, pageUrl) {
  const offers = named(entry, "link")
    .filter((link) => FORMATS[link.attrs.type])
    .map((link) => Object.assign(
      { id: host.url.resolve(pageUrl, link.attrs.href) }, FORMATS[link.attrs.type]));
  return {
    id: textOf(entry, "id"),
    title: textOf(entry, "title"),
    authors: named(entry, "author").map((author) => textOf(author, "name")),
    language: textOf(entry, "language") || null,
    page_url: pageUrl,
    offers,
  };
}

export function search(query, ctx) {
  const base = ctx.settings.base_url;
  let url = query.cursor;
  if (!url) {
    // Step 1: the term alone answers with a menu (books, authors).
    const menu = get(host.url.resolve(base, "/opds/search?searchTerm=" + encodeURIComponent(query.text)));
    const books = named(host.xml.parse(menu.body), "entry")
      .find((entry) => textOf(entry, "title") === "Books");
    if (!books) {
      return { hits: [] };
    }
    url = host.url.resolve(menu.url, named(books, "link")[0].attrs.href);
  }
  // Step 2: the results; the next page is a link, which is the cursor.
  const page = get(url);
  const feed = host.xml.parse(page.body);
  const next = named(feed, "link").find((link) => link.attrs.rel === "next");
  return {
    hits: named(feed, "entry").map((entry) => toHit(entry, page.url)),
    next_cursor: next ? host.url.resolve(page.url, next.attrs.href) : null,
  };
}

export function details(hit, ctx) {
  // The feed has no annotation; fetch the page the result came from and say where it is from.
  const feed = host.xml.parse(get(hit.page_url).body);
  const entry = named(feed, "entry").find((e) => textOf(e, "id") === hit.id);
  if (!entry) {
    return null;
  }
  return { description: "From the feed “" + textOf(feed, "title") + "”." };
}

export function resolveDownload(offer, ctx) {
  // The offer's id is the link; the catalog redirects from it to the file.
  return { url: offer.id, archive: "zip" };
}
