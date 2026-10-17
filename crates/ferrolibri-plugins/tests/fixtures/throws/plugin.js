function inner() { throw new TypeError("the catalog format changed"); }
export function search(query, ctx) {
  switch (query.text) {
    case "string": throw "just a string";
    case "object": throw { code: 7 };
    case "undefined-call": return undefinedFunction();
    default: return inner();
  }
}
