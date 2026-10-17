// Valid JSON, wrong contract: the runtime passes it on, validation rejects it.
export function search(query, ctx) {
  switch (query.text) {
    case "hits-not-array": return { hits: "nope" };
    case "no-hits": return { results: [] };
    case "bad-cursor": return { hits: [], next_cursor: 42 };
    case "array": return [1, 2, 3];
    case "mixed": return { hits: [{ id: "ok", title: "Fine" }, { id: "bad" }, { title: "no id" }] };
    default: return "just a string";
  }
}
