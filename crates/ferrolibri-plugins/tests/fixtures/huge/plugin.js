export function search(query, ctx) {
  if (query.text === "result") {
    // 3 MiB of JSON, over the 2 MiB limit.
    return { hits: [{ id: "1", title: "x".repeat(3 * 1024 * 1024) }] };
  }
  // The server answers with more than 8 MiB.
  host.fetch(host.url.resolve(ctx.settings.base_url, "/huge"));
  return { hits: [] };
}
