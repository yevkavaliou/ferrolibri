// FerroLibri plugin: search in an OPDS catalog (https://specs.opds.io), Project Gutenberg by default.
//
// Two shapes of catalog are understood:
//  * a result entry lists its files (rel "http://opds-spec.org/acquisition"): they are the offers;
//  * a result entry only links to a feed of its own book, as Project Gutenberg does
//    (/ebooks/345.opds): `search` returns the book without files, `details` reads the book's feed
//    for the files, the annotation, the subjects and the cover.

const FORMATS = {
  "application/epub+zip": { format: "EPUB" },
  "application/x-mobipocket-ebook": { format: "MOBI" },
  "application/pdf": { format: "PDF" },
  "application/fb2+zip": { format: "FB2", archive: "zip" },
};
const ACQUISITION = "http://opds-spec.org/acquisition";
const BOOK_FEED = /\/(\d+)\.opds$/;

function get(url) {
  const reply = host.fetch(url, { headers: { Accept: "application/atom+xml" } });
  if (reply.status !== 200) {
    throw new Error("The catalog answered with status " + reply.status);
  }
  return reply;
}

const named = (node, name) => node.children.filter((child) => child.name === name);
const textOf = (node, name) => (named(node, name)[0] || { text: "" }).text.trim();
const links = (node, rel) =>
  named(node, "link").filter((link) => link.attrs.rel === rel || (rel === ACQUISITION && (link.attrs.rel || "").startsWith(ACQUISITION)));

function offersOf(entry, pageUrl) {
  const offers = [];
  for (const link of links(entry, ACQUISITION)) {
    const kind = FORMATS[(link.attrs.type || "").split(";")[0].trim()];
    if (!kind || !link.attrs.href) {
      continue;
    }
    const offer = Object.assign({ id: host.url.resolve(pageUrl, link.attrs.href) }, kind);
    if (link.attrs.title) {
      offer.label = link.attrs.title.slice(0, 100);
    }
    const size = Number(link.attrs.length);
    if (size > 0) {
      offer.size = size;
    }
    offers.push(offer);
  }
  return offers.slice(0, 10);
}

// Project Gutenberg puts the author into a plain-text `content`; a book without an author
// (an anthology) has the count of its downloads there instead.
function authorLine(content) {
  const line = content && content.attrs.type === "text" ? content.text.trim() : "";
  return line && !/^[\d,.]+ downloads$/.test(line) ? [line] : [];
}

function toHit(entry, pageUrl) {
  const title = textOf(entry, "title");
  const offers = offersOf(entry, pageUrl);
  const authors = named(entry, "author").map((author) => textOf(author, "name")).filter(Boolean);
  if (offers.length > 0) {
    return { id: textOf(entry, "id"), title, authors, offers, page_url: pageUrl };
  }
  // The entries of a search also include "Authors", "Subjects" and the like: those do not link
  // to a book's feed.
  const feed = named(entry, "link").find((link) => BOOK_FEED.test(link.attrs.href || ""));
  if (!feed) {
    return null;
  }
  const id = host.url.resolve(pageUrl, feed.attrs.href);
  const number = BOOK_FEED.exec(id)[1];
  const content = named(entry, "content")[0];
  return {
    id,
    title,
    authors: authors.length > 0 ? authors : authorLine(content),
    page_url: id.replace(/\.opds$/, ""),
    cover_url: host.url.resolve(id, "/cache/epub/" + number + "/pg" + number + ".cover.small.jpg"),
  };
}

export function search(query, ctx) {
  const template = ctx.settings.search_path;
  if (!template || !template.includes("{searchTerms}")) {
    throw new Error("The search path in the plugin settings must contain {searchTerms}.");
  }
  const url = query.cursor ||
    host.url.resolve(ctx.settings.base_url, template.replace("{searchTerms}", encodeURIComponent(query.text)));
  const page = get(url);
  const feed = host.xml.parse(page.body);
  const next = links(feed, "next")[0];
  return {
    hits: named(feed, "entry").map((entry) => toHit(entry, page.url)).filter(Boolean),
    // The next page is a link, which is the cursor.
    next_cursor: next ? host.url.resolve(page.url, next.attrs.href) : null,
  };
}

export function details(hit, ctx) {
  if (hit.offers && hit.offers.length > 0) {
    return null; // the search already gave everything the catalog has
  }
  const page = get(hit.id);
  // A book's feed has an entry for each edition (with and without images): the first one
  // describes the book, and every one lists files.
  const entries = named(host.xml.parse(page.body), "entry");
  const entry = entries[0];
  if (!entry) {
    return null;
  }
  // The annotation is a paragraph "Summary: …" inside the XHTML `content`.
  const paragraphs = named(named(entry, "content")[0] || { children: [] }, "div")
    .flatMap((div) => named(div, "p"))
    .map((p) => p.text.trim());
  const summary = paragraphs.find((text) => text.startsWith("Summary:"));
  const image = links(entry, "http://opds-spec.org/image")[0] || links(entry, "http://opds-spec.org/image/thumbnail")[0];
  const offers = entries.flatMap((e) => offersOf(e, page.url)).slice(0, 10);
  return {
    description: summary ? summary.slice("Summary:".length).replace(/\s+/g, " ").trim().slice(0, 8000) : null,
    language: textOf(entry, "language") || null,
    tags: named(entry, "category").filter((c) => c.attrs.scheme === "http://purl.org/dc/terms/LCSH").map((c) => c.attrs.term.slice(0, 60)).slice(0, 30),
    cover_url: image ? host.url.resolve(page.url, image.attrs.href) : null,
    offers: offers.length > 0 ? offers : null,
  };
}

export function resolveDownload(offer, ctx) {
  // The offer's id is the address of the file; the catalog redirects from it to the real one.
  return { url: offer.id, archive: offer.archive };
}
