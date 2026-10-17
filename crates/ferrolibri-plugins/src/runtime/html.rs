//! `host.html.select`: HTML text + CSS selector → matching elements.

use kuchikiki::{iter::NodeEdge, traits::TendrilSink};
use serde_json::{Map, Value, json};

use crate::limits::{
    MAX_HTML_DEPTH, MAX_HTML_UNCLOSED_TAGS, MAX_PARSE_INPUT_BYTES, MAX_SELECT_MATCHES,
    MAX_SELECT_OUTPUT_BYTES,
};

/// Parses `html` leniently, as a browser does, and returns, for every element that matches
/// `selector` in document order, `{ text, html, attrs }`: its text, its markup (the element
/// itself included) and its attributes by name.
pub fn select(html: &str, selector: &str) -> Result<Vec<Value>, String> {
    if html.len() > MAX_PARSE_INPUT_BYTES {
        return Err(format!(
            "the HTML is larger than {MAX_PARSE_INPUT_BYTES} bytes"
        ));
    }
    if unclosed_tags(html) > MAX_HTML_UNCLOSED_TAGS {
        return Err(format!(
            "the HTML has more than {MAX_HTML_UNCLOSED_TAGS} tags that are opened and never closed"
        ));
    }
    let document = kuchikiki::parse_html().one(html);
    // Serialising an element recurses once per level of nesting, and a stack overflow ends the
    // whole process: refuse documents deeper than any real page before touching them.
    let mut depth = 0usize;
    for edge in document.document_node.traverse() {
        match edge {
            NodeEdge::Start(_) => {
                depth += 1;
                if depth > MAX_HTML_DEPTH {
                    return Err(format!(
                        "the HTML is nested deeper than {MAX_HTML_DEPTH} levels"
                    ));
                }
            }
            NodeEdge::End(_) => depth -= 1,
        }
    }
    let matches = document
        .document_node
        .select(selector)
        .map_err(|()| format!("{selector:?} is not a supported CSS selector"))?;
    let mut found = Vec::new();
    let mut output_bytes = 0usize;
    for element in matches {
        if found.len() == MAX_SELECT_MATCHES {
            return Err(format!(
                "the selector matches more than {MAX_SELECT_MATCHES} elements"
            ));
        }
        let text = element.text_contents();
        let markup = element.as_node().to_string();
        output_bytes += text.len() + markup.len();
        if output_bytes > MAX_SELECT_OUTPUT_BYTES {
            return Err(format!(
                "the matches hold more than {MAX_SELECT_OUTPUT_BYTES} bytes of text and markup"
            ));
        }
        let mut attrs = Map::new();
        for (name, attribute) in &element.attributes.borrow().map {
            attrs.insert(
                name.local.to_string(),
                Value::String(attribute.value.clone()),
            );
        }
        found.push(json!({ "text": text, "html": markup, "attrs": attrs }));
    }
    Ok(found)
}

/// The largest number of tags that are open at once, counted from the text alone. Elements that
/// close themselves (`<p>`, `<li>`, void elements, …) and the text of `<script>` and `<style>`
/// are left out. It is cheap and approximate; its job is to turn away input that makes the
/// HTML parser take time quadratic in the nesting (tens of thousands of unclosed `<div>`s take
/// minutes, and a parse cannot be interrupted), without refusing ordinary sloppy pages.
fn unclosed_tags(html: &str) -> usize {
    const SELF_CLOSING: &[&str] = &[
        "p", "li", "dt", "dd", "td", "th", "tr", "tbody", "thead", "tfoot", "option", "optgroup",
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];
    let bytes = html.as_bytes();
    let (mut open, mut deepest, mut at) = (0usize, 0usize, 0usize);
    while let Some(found) = bytes[at..].iter().position(|&b| b == b'<') {
        at += found + 1;
        match bytes.get(at) {
            Some(b'/') => open = open.saturating_sub(1),
            Some(b) if b.is_ascii_alphabetic() => {
                let end = bytes[at..]
                    .iter()
                    .position(|&b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
                    .map_or(bytes.len(), |n| at + n);
                let name = html[at..end].to_ascii_lowercase();
                if matches!(name.as_str(), "script" | "style") {
                    // Skip to the closing tag: the text inside is not markup.
                    let closing = format!("</{name}");
                    let lower = html[end..].to_ascii_lowercase();
                    at = lower.find(&closing).map_or(bytes.len(), |n| end + n);
                } else if !SELF_CLOSING.contains(&name.as_str()) {
                    open += 1;
                    deepest = deepest.max(open);
                }
            }
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "<html><body><ul id=books>\
        <li class=book data-id=1><a href=\"/b/1\">War &amp; Peace</a></li>\
        <li class=book data-id=2><a href=\"/b/2\">Emma</a><li class=other>x</ul></body></html>";

    #[test]
    fn selects_in_document_order_with_text_markup_and_attributes() {
        let found = select(PAGE, "li.book > a").unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["text"], "War & Peace");
        assert_eq!(found[0]["attrs"]["href"], "/b/1");
        assert_eq!(found[0]["html"], "<a href=\"/b/1\">War &amp; Peace</a>");
        assert_eq!(found[1]["text"], "Emma");
        let items = select(PAGE, "li[data-id]").unwrap();
        assert_eq!(items[1]["attrs"]["data-id"], "2");
        assert_eq!(items[1]["attrs"]["class"], "book");
    }

    #[test]
    fn broken_markup_is_repaired_and_no_match_is_an_empty_list() {
        assert_eq!(select(PAGE, "li").unwrap().len(), 3);
        assert!(select(PAGE, "table").unwrap().is_empty());
        assert!(select("", "a").unwrap().is_empty());
    }

    #[test]
    fn an_invalid_selector_is_an_error() {
        assert!(select(PAGE, "li[").is_err());
        assert!(select(PAGE, "").is_err());
    }

    #[test]
    fn too_many_matches_are_refused() {
        let html = "<i></i>".repeat(MAX_SELECT_MATCHES + 1);
        assert!(select(&html, "i").is_err());
        assert_eq!(select(&html, "i:first-child").unwrap().len(), 1);
    }

    #[test]
    fn nested_matches_cannot_multiply_the_output_without_bound() {
        // Every `div` repeats everything inside it: 400 copies of 100 KB.
        let depth = 400;
        let text = "x".repeat(100_000);
        let html = format!("{}{text}{}", "<div>".repeat(depth), "</div>".repeat(depth));
        assert!(select(&html, "div").is_err());
    }

    #[test]
    fn counting_unclosed_tags_ignores_what_closes_itself_and_script_text() {
        assert_eq!(unclosed_tags("<div><div></div><p>a<p>b<br><li>c</div>"), 2);
        assert_eq!(
            unclosed_tags("<script>if (a<b && c<d) { x = '<div><div>' }</script>"),
            0
        );
        assert_eq!(unclosed_tags("<SCRIPT>1</SCRIPT><Div>"), 1);
        assert_eq!(unclosed_tags("a < b <3 <!-- <div> --> </x>"), 1);
        assert_eq!(unclosed_tags("<ul>".repeat(5).as_str()), 5);
        assert_eq!(unclosed_tags("<"), 0);
    }

    #[test]
    fn very_deep_documents_are_refused_before_they_can_overflow_the_stack() {
        // Serialising such a tree would recurse a million times.
        let html = "<span>".repeat(5_000);
        assert!(select(&html, "span").unwrap_err().contains("nested deeper"));
        let unclosed = "<div>".repeat(1_000_000);
        assert!(
            select(&unclosed, "div")
                .unwrap_err()
                .contains("never closed")
        );
        let ok = format!("{}x{}", "<div>".repeat(300), "</div>".repeat(300));
        assert_eq!(select(&ok, "div").unwrap().len(), 300);
    }
}
