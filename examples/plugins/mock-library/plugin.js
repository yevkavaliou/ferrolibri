// FerroLibri plugin: a mock source for testing the app. It never calls host.fetch: every search
// answers at once with the same made-up books. See README.md for the data and the queries.

const TOTAL = 45; // books in the regular set
const SITE = "https://mock.invalid";

// --- Building blocks -----------------------------------------------------------------------

function pad(n) {
  return "book-" + String(n).padStart(2, "0");
}

function names(count) {
  return Array.from({ length: count }, (_, i) => "Author " + String(i + 1).padStart(2, "0"));
}

// One downloadable file. The id is `<hit id>-<key>`; resolveDownload reads the key back.
const KINDS = {
  epub: { format: "EPUB", ext: ".epub" },
  fb2: { format: "FB2", ext: ".fb2" },
  fb2zip: { format: "FB2", archive: "zip", ext: ".fb2.zip" },
  mobi: { format: "MOBI", ext: ".mobi" },
  azw3: { format: "AZW3", ext: ".azw3" },
  pdf: { format: "PDF", ext: ".pdf" },
  txt: { format: "TXT", ext: ".txt" },
  docx: { format: "DOCX", ext: ".docx" },
};

function offer(hitId, key, size, label) {
  const kind = KINDS[key] || KINDS.epub;
  const result = { id: hitId + "-" + key, format: kind.format };
  if (kind.archive) result.archive = kind.archive;
  if (size) result.size = size;
  if (label) result.label = label;
  return result;
}

function longText(length, unit) {
  let text = "";
  for (let i = 1; text.length < length; i++) text += unit(i);
  return text.slice(0, length - 1) + ".";
}

// --- The books -----------------------------------------------------------------------------

// The first twelve cover the edges of the contract; the rest are alike.
const SHOWCASE = [
  (id) => ({
    id,
    title: "War and Peace",
    authors: ["Leo Tolstoy"],
    description: "A novel of five families in the Napoleonic wars.\nSecond line.\n\tIndented line.",
    language: "en",
    year: 1869,
    isbn: "978-0-14-044793-4",
    publisher: "Mock Press",
    tags: ["classic", "history", "novel"],
    offers: [
      offer(id, "epub", 2_300_000, "Standard ebook"),
      offer(id, "fb2zip", 1_100_000, "FictionBook, zipped"),
      offer(id, "mobi", 3_000_000),
      offer(id, "pdf", 9_500_000, "Scanned edition"),
    ],
  }),
  (id) => ({
    id,
    title: "Война и мир",
    authors: ["Лев Толстой"],
    description: "Роман-эпопея о русском обществе в эпоху наполеоновских войн.",
    language: "ru",
    year: 1869,
    tags: ["классика"],
    offers: [offer(id, "fb2zip", 1_200_000), offer(id, "epub", 2_100_000)],
  }),
  (id) => ({
    id,
    title: "The Mock Trilogy: Part Two",
    authors: ["Mia Mockwell"],
    description: "The middle book of a series.",
    language: "en",
    year: 2012,
    series: "The Mock Trilogy",
    series_index: 2,
    offers: [offer(id, "azw3", 1_800_000)],
  }),
  (id) => ({ id, title: "A Book Without Authors, Year or Files", description: "Only the page link." }),
  (id) => ({
    id,
    title: "Twenty Authors",
    authors: names(20),
    description: "The most authors the contract allows.",
    offers: [offer(id, "epub")],
  }),
  (id) => ({
    id,
    title: longText(300, (i) => "Long title " + i + " "),
    authors: ["Verbose Writer"],
    description: "The title is exactly 300 characters.",
    offers: [offer(id, "pdf", 500_000)],
  }),
  (id) => ({
    id,
    title: "Ten Offers",
    authors: ["Mia Mockwell"],
    description: "The most offers the contract allows; the last two fail when resolved.",
    offers: [
      offer(id, "epub", 1_000_000),
      offer(id, "fb2", 900_000),
      offer(id, "fb2zip", 400_000),
      offer(id, "mobi", 1_500_000),
      offer(id, "azw3", 1_400_000),
      offer(id, "pdf", 5_000_000),
      offer(id, "txt", 700_000),
      offer(id, "docx", 800_000),
      { id: id + "-fail", format: "EPUB", label: "resolveDownload throws" },
      { id: id + "-blocked", format: "EPUB", label: "address not allowed" },
    ],
  }),
  (id) => ({
    id,
    title: "吾輩は猫である 🐈",
    authors: ["夏目漱石"],
    description: "日本語の説明と絵文字 📚 を表示するための項目。",
    language: "ja",
    year: 1905,
    offers: [offer(id, "epub", 600_000)],
  }),
  (id) => ({
    id,
    title: "ألف ليلة وليلة",
    authors: ["مجهول"],
    description: "نص من اليمين إلى اليسار لاختبار العرض.",
    language: "ar",
    offers: [offer(id, "epub", 900_000)],
  }),
  (id) => ({
    id,
    title: "Thirty Tags and a Long Description",
    authors: ["Mia Mockwell"],
    description: longText(8000, (i) => "Line " + i + "\tfills the description up to the limit.\n"),
    tags: Array.from({ length: 30 }, (_, i) => "tag-" + String(i + 1).padStart(2, "0")),
    offers: [offer(id, "txt", 8000)],
  }),
  (id) => ({
    id,
    title: "Livro em Português",
    authors: ["Autora Exemplo"],
    description: "A book with a regional language tag, the highest year and a fractional series index.",
    language: "pt-BR",
    year: 2100,
    isbn: "85-359-0277-5",
    series: "Série de Teste",
    series_index: 1.5,
    offers: [offer(id, "epub", 700_000)],
  }),
  (id) => ({ id, title: "Just a Title" }),
];

const AUTHORS = ["Ada Mockford", "Boris Fakeman", "Clara Dummy", "Dmitri Sample", "Eve Stubb"];

function filler(n) {
  const id = pad(n);
  const hit = {
    id,
    title: "Mock Book " + n,
    authors: [AUTHORS[n % AUTHORS.length]],
    description: "A generated book number " + n + ".",
    language: n % 4 === 0 ? "de" : "en",
    year: 1950 + ((n * 7) % 70),
    tags: ["generated", "group-" + (n % 5)],
  };
  // Every third book has no files, so `details` has something to add.
  if (n % 3 === 0) hit.offers = [offer(id, "epub", 100_000 * n)];
  else if (n % 3 === 1) hit.offers = [offer(id, "pdf", 200_000 * n), offer(id, "fb2zip", 80_000 * n)];
  return hit;
}

function book(n) {
  return n <= SHOWCASE.length ? SHOWCASE[n - 1](pad(n)) : filler(n);
}

// --- Settings and queries ------------------------------------------------------------------

function flag(value, fallback) {
  if (value === undefined || value === null) return fallback;
  return !(value === false || value === "false");
}

function readSettings(ctx) {
  const s = (ctx && ctx.settings) || {};
  const perPage = Number(s.per_page);
  return {
    perPage: perPage > 0 ? perPage : 20,
    withOffers: flag(s.with_offers, true),
    extraTag: typeof s.extra_tag === "string" ? s.extra_tag.trim() : "",
  };
}

function dress(hit, settings, echo) {
  const out = Object.assign({}, hit);
  out.page_url = SITE + "/books/" + hit.id;
  if (echo && hit.description && hit.id !== "book-10") out.description += "\n\n" + echo;
  if (!settings.withOffers) delete out.offers;
  if (settings.extraTag) out.tags = (hit.tags || []).concat(settings.extraTag.slice(0, 60)).slice(0, 30);
  return out;
}

function range(from, to) {
  const items = [];
  for (let n = from; n <= to; n++) items.push(book(n));
  return items;
}

function special(text, settings) {
  switch (text) {
    case "!empty":
      return { hits: [] };
    case "!single":
      return { hits: [dress(book(1), settings)] };
    case "!max":
      return { hits: range(1, 100).map((hit) => dress(hit, settings)), next_cursor: null };
    case "!toomany":
      return { hits: range(1, 101).map((hit) => dress(hit, settings)) };
    case "!reject":
      return { hits: "this is not an array" };
    case "!huge":
      // More than the 2 MiB a plugin may return.
      return { hits: [dress(book(1), settings)], padding: "x".repeat(3 * 1024 * 1024) };
    case "!error":
      throw new Error("Mock Library failed on purpose. Try another query.");
    case "!log":
      for (const level of ["debug", "info", "warn", "error"]) host.log(level, "log line at level " + level);
      return { hits: [dress(book(12), settings)] };
    case "!invalid": {
      const good = range(1, 3).map((hit) => dress(hit, settings));
      const bad = [
        { id: "bad-no-title" },
        { id: "book-01", title: "Repeats an id of this page" },
        { id: "bad-year", title: "Year zero", year: 0 },
        { id: "bad-authors", title: "Twenty-one authors", authors: names(21) },
        { id: "bad-format", title: "Lower-case format", offers: [{ id: "o", format: "epub" }] },
        { id: "bad-cover", title: "Cover on a foreign site", cover_url: "https://example.org/cover.jpg" },
        { id: "bad-isbn", title: "Not an ISBN", isbn: "not-an-isbn" },
        { id: "bad-archive", title: "Zip on an EPUB", offers: [{ id: "o", format: "EPUB", archive: "zip" }] },
      ];
      return { hits: good.concat(bad), next_cursor: null };
    }
    default:
      return null;
  }
}

// --- Entry points --------------------------------------------------------------------------

export function search(query, ctx) {
  const text = String((query && query.text) || "").trim();
  const settings = readSettings(ctx);
  const answer = special(text.toLowerCase(), settings);
  if (answer) return answer;

  // The cursor is the offset of the next page; the first page comes from `query.page`.
  const offset = query.cursor ? Number(query.cursor) : (Math.max(query.page || 1, 1) - 1) * settings.perPage;
  const page = Math.floor(offset / settings.perPage) + 1;
  const last = Math.min(offset + settings.perPage, TOTAL);
  const echo = 'Mock answer to "' + text.slice(0, 100) + '", page ' + page + ".";
  host.log("debug", "search " + JSON.stringify(text) + " offset " + offset);
  return {
    hits: range(offset + 1, last).map((hit) => dress(hit, settings, echo)),
    next_cursor: last < TOTAL ? String(last) : null,
  };
}

export function details(hit, ctx) {
  const settings = readSettings(ctx);
  switch (hit.id) {
    case "book-04":
      return {};
    case "book-12":
      return null;
    case "book-05":
      throw new Error("Mock Library: details failed on purpose.");
    case "book-06":
      return { year: 0 }; // breaks the contract after the merge: the old result stays
  }
  const extra = {
    publisher: "Mock Press",
    description: (hit.description || hit.title) + "\n\nExtended description from details().",
  };
  if (settings.withOffers && !(hit.offers && hit.offers.length)) {
    extra.offers = [offer(hit.id, "epub", 123_456), offer(hit.id, "fb2zip", 65_432, "Added by details")];
  }
  return extra;
}

export function resolveDownload(offer, ctx) {
  if (offer.id.endsWith("-fail")) throw new Error("Mock Library: resolveDownload failed on purpose.");
  if (offer.id.endsWith("-blocked")) return { url: "https://example.org/files/" + offer.id + ".epub" };
  const key = Object.keys(KINDS).find((k) => offer.id.endsWith("-" + k)) || "epub";
  const result = { url: SITE + "/files/" + offer.id + KINDS[key].ext };
  if (offer.format === "FB2" && offer.archive === "zip") result.archive = "zip";
  return result;
}
