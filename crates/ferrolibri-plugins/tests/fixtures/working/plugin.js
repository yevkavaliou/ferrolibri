// Answers from memory; touches every host function except fetch.
export function search(query, ctx) {
  host.log("info", "searching for " + query.text + " on " + ctx.settings.shelf);
  const page = host.html.select(
    '<ul><li class="b" data-id="1"><a href="/b/1">One &amp; Two</a></li>' +
    '<li class="b" data-id="2"><a href="b/2">Three</a></li></ul>',
    "li.b > a");
  const feed = host.xml.parse(
    '<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Atom title</title></entry></feed>');
  const base = "https://books.example.org/catalog/index.html";
  const hits = page.map((a, i) => ({
    id: String(i + 1),
    title: a.text + (ctx.settings.loud ? "!" : ""),
    authors: [feed.children[0].children[0].text],
    page_url: host.url.resolve(base, a.attrs.href),
    cover_url: host.url.resolve(base, "//books.example.org/cover/" + (i + 1) + ".jpg"),
    offers: [{ id: "o" + (i + 1), format: "EPUB", size: 1000 * (i + 1) }],
  }));
  return { hits, next_cursor: query.page < 2 ? "page-" + (query.page + 1) : null };
}

export function details(hit, ctx) {
  return { description: "Details of " + hit.title, series: "Series " + ctx.settings.shelf };
}

export function resolveDownload(offer, ctx) {
  return { url: "https://books.example.org/files/" + offer.id + ".epub" };
}
