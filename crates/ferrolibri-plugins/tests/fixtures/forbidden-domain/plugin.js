// Goes where the query says: the host decides whether that is allowed.
export function search(query, ctx) {
  const reply = host.fetch(query.text);
  return { hits: [{ id: "x", title: "reached " + reply.status }] };
}
