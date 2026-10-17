//! `host.xml.parse`: text → a tree of plain objects.

use serde_json::{Map, Value, json};

use crate::limits::{MAX_PARSE_INPUT_BYTES, MAX_XML_NODES};

/// Parses `text` (an XML document) into
/// `{ name, ns, attrs: { local name: value }, text, children: [ … ] }`.
///
/// `name` is the local name; `ns` is the namespace URI or `null`; `text` is the text of the
/// element and of everything inside it. DTDs are refused, so no entity can expand beyond the five
/// predefined ones and character references.
pub fn parse(text: &str) -> Result<Value, String> {
    if text.len() > MAX_PARSE_INPUT_BYTES {
        return Err(format!(
            "the text is larger than {MAX_PARSE_INPUT_BYTES} bytes"
        ));
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
        // External entities are never fetched.
        entity_resolver: None,
    };
    let document = roxmltree::Document::parse_with_options(text, options)
        .map_err(|e| format!("not well-formed XML: {e}"))?;
    Ok(element(document.root_element()))
}

fn element(node: roxmltree::Node) -> Value {
    let mut attrs = Map::new();
    for attribute in node.attributes() {
        attrs
            .entry(attribute.name())
            .or_insert_with(|| Value::String(attribute.value().to_owned()));
    }
    let mut own_text = String::new();
    let mut children = Vec::new();
    for child in node.children() {
        if child.is_element() {
            let child = element(child);
            if let Some(Value::String(inner)) = child.get("text") {
                own_text.push_str(inner);
            }
            children.push(child);
        } else if child.is_text() {
            own_text.push_str(child.text().unwrap_or_default());
        }
    }
    json!({
        "name": node.tag_name().name(),
        "ns": node.tag_name().namespace(),
        "attrs": attrs,
        "text": own_text,
        "children": children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_tree_with_local_names_and_namespaces() {
        let tree = parse(
            "<feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:dc=\"urn:dc\">\
             <title>Books &amp; more</title><dc:language>ru</dc:language>\
             <link rel=\"next\" href=\"/x?a=1&amp;b=2\"/></feed>",
        )
        .unwrap();
        assert_eq!(tree["name"], "feed");
        assert_eq!(tree["ns"], "http://www.w3.org/2005/Atom");
        let children = tree["children"].as_array().unwrap();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0]["text"], "Books & more");
        assert_eq!(children[1]["name"], "language");
        assert_eq!(children[1]["ns"], "urn:dc");
        assert_eq!(children[2]["attrs"]["href"], "/x?a=1&b=2");
        assert_eq!(tree["text"], "Books & moreru");
    }

    #[test]
    fn elements_without_a_namespace_have_a_null_ns() {
        assert_eq!(parse("<a/>").unwrap()["ns"], Value::Null);
    }

    #[test]
    fn malformed_text_and_dtds_are_refused() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("").is_err());
        let bomb = "<?xml version=\"1.0\"?><!DOCTYPE a [<!ENTITY x \"aaaa\">]><a>&x;</a>";
        assert!(parse(bomb).is_err());
    }

    #[test]
    fn too_many_nodes_are_refused() {
        let text = format!("<a>{}</a>", "<b/>".repeat(MAX_XML_NODES as usize));
        assert!(parse(&text).is_err());
    }
}
