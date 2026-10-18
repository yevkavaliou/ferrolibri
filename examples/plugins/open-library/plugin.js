// FerroLibri plugin: search in Open Library (https://openlibrary.org/dev/docs/api/search).
// A search is one GET that answers with JSON; there is nothing to download, so the plugin
// returns the data of a book and a link to its page.

const PAGE_SIZE = 20;
const FIELDS = "key,title,author_name,first_publish_year,cover_i,subject";

function get(url) {
  const reply = host.fetch(url, { headers: { Accept: "application/json" } });
  if (reply.status === 429) {
    throw new Error("Open Library asks to slow down (status 429). Try again in a minute.");
  }
  if (reply.status !== 200) {
    throw new Error("Open Library answered with status " + reply.status);
  }
  return JSON.parse(reply.body);
}

// A work collects the author names of all its editions: the real author first, then translators,
// editors, misspellings and repeats (up to 57 of them). The host drops a result with more than
// 20 authors, and a long list is not useful anyway, so keep the first distinct names.
function authorsOf(doc) {
  return [...new Set(doc.author_name || [])].slice(0, 5).map((name) => name.slice(0, 120));
}

function toHit(doc, base) {
  return {
    id: doc.key,
    title: doc.title,
    authors: authorsOf(doc),
    year: doc.first_publish_year || null,
    // The subjects of a popular book run to hundreds; the host keeps at most 30 of 60 characters.
    tags: (doc.subject || []).slice(0, 10).map((subject) => subject.slice(0, 60)),
    cover_url: doc.cover_i
      ? "https://covers.openlibrary.org/b/id/" + doc.cover_i + "-M.jpg"
      : null,
    page_url: host.url.resolve(base, doc.key),
  };
}

export function search(query, ctx) {
  // The cursor is the number of the next page; the first page comes in `query.page`.
  const page = query.cursor ? Number(query.cursor) : query.page;
  const url = host.url.resolve(
    ctx.settings.base_url,
    "/search.json?q=" + encodeURIComponent(query.text) +
      "&fields=" + FIELDS + "&limit=" + PAGE_SIZE + "&page=" + page,
  );
  const data = get(url);
  const docs = data.docs || [];
  // A work without a key or a title cannot be shown; the host would drop it with a warning anyway.
  const hits = docs.filter((doc) => doc.key && doc.title).map((doc) => toHit(doc, ctx.settings.base_url));
  const more = (data.start || 0) + docs.length < (data.numFound || 0);
  return { hits, next_cursor: more ? String(page + 1) : null };
}
