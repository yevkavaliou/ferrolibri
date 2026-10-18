// FerroLibri plugin: search in Google Books (https://developers.google.com/books/docs/v1/using).
// One GET answers with JSON. Google may refuse requests without an API key once the shared
// quota is used up, so the key is an optional setting and the refusal says what to do.

const PAGE_SIZE = 20;

function get(url) {
  const reply = host.fetch(url, { headers: { Accept: "application/json" } });
  if (reply.status === 200) {
    return JSON.parse(reply.body);
  }
  if (reply.status === 429 || reply.status === 403) {
    throw new Error(
      "Google Books refused the request (status " + reply.status + "): the quota is used up. " +
        "Enter your own API key in the plugin settings, or try again later.",
    );
  }
  throw new Error("Google Books answered with status " + reply.status);
}

// Descriptions come as HTML fragments (<p>, <br>, <b>); the app shows plain text.
function plain(html) {
  return html
    .replace(/<\/p>|<br\s*\/?>/gi, "\n")
    .replace(/<[^>]*>/g, "")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .trim();
}

function isbnOf(info) {
  const all = info.industryIdentifiers || [];
  const found = all.find((id) => id.type === "ISBN_13") || all.find((id) => id.type === "ISBN_10");
  return found ? found.identifier : null;
}

function toHit(item) {
  const info = item.volumeInfo || {};
  const year = parseInt((info.publishedDate || "").slice(0, 4), 10);
  const thumbnail = (info.imageLinks || {}).thumbnail;
  return {
    id: item.id,
    title: info.subtitle ? info.title + ": " + info.subtitle : info.title,
    authors: info.authors || [],
    publisher: info.publisher || null,
    year: year > 0 ? year : null,
    language: info.language || null,
    isbn: isbnOf(info),
    tags: info.categories || [],
    description: info.description ? plain(info.description) : null,
    // The API hands out http:// links and asks for a page curl in the picture.
    cover_url: thumbnail ? thumbnail.replace(/^http:/, "https:").replace("&edge=curl", "") : null,
    page_url: "https://books.google.com/books?id=" + encodeURIComponent(item.id),
  };
}

export function search(query, ctx) {
  // The cursor is the index of the first volume of the next page.
  const start = query.cursor ? Number(query.cursor) : (query.page - 1) * PAGE_SIZE;
  let url = host.url.resolve(
    ctx.settings.base_url,
    "/books/v1/volumes?q=" + encodeURIComponent(query.text) +
      "&printType=books&maxResults=" + PAGE_SIZE + "&startIndex=" + start,
  );
  if (ctx.settings.api_key) {
    url += "&key=" + encodeURIComponent(ctx.settings.api_key);
  }
  const items = get(url).items || [];
  return {
    hits: items.filter((item) => item.id && item.volumeInfo && item.volumeInfo.title).map(toHit),
    // `totalItems` is only a guess, so a full page is the sign that there may be more.
    next_cursor: items.length === PAGE_SIZE ? String(start + PAGE_SIZE) : null,
  };
}
