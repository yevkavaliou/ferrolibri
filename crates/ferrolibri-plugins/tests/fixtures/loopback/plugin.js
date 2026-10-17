// Tries the machine it runs on, by address and by name.
export function search(query, ctx) {
  const seen = [];
  for (const url of query.text.split(" ")) {
    try {
      const reply = host.fetch(url);
      seen.push(url + " -> " + reply.status);
    } catch (e) {
      seen.push(url + " -> " + e.kind);
    }
  }
  return { hits: seen.map((s, i) => ({ id: String(i), title: s })) };
}
