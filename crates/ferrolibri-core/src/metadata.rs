use std::{fs, path::Path};

pub use crate::metadata_model::Metadata;

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const OPF_NS: &str = "http://www.idpf.org/2007/opf";

impl Metadata {
    /// Fill only missing fields; nonempty OPF values take precedence.
    pub fn with_fallback(mut self, title: &str, author: &str) -> Self {
        if self.title.trim().is_empty() {
            self.title = title.to_owned();
        }
        self.authors.retain(|a| !a.trim().is_empty());
        if self.authors.is_empty() && !author.is_empty() {
            self.authors.push(author.to_owned());
        }
        self
    }
}

/// Read display metadata, using directory labels for absent or empty OPF fields.
/// Corrupt or unreadable OPF remains an error, never a successful fallback.
pub fn read_metadata(book_path: &Path) -> Result<Metadata, crate::Error> {
    let title = crate::scanner::strip_calibre_id(
        &book_path.file_name().unwrap_or_default().to_string_lossy(),
    );
    let author = book_path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy();
    Ok(parse_opf(book_path)?
        .unwrap_or_default()
        .with_fallback(&title, &author))
}

pub fn parse_opf(book_path: &Path) -> Result<Option<Metadata>, crate::Error> {
    let opf_path = book_path.join("metadata.opf");
    let xml = match fs::read_to_string(&opf_path) {
        Ok(xml) => xml,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(crate::Error::io("read OPF", &opf_path, e)),
    };
    let doc = roxmltree::Document::parse(&xml)
        .map_err(|e| crate::Error::invalid_data("parse OPF", &opf_path, e))?;
    let metadata_node = doc
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "metadata"
                && matches!(n.tag_name().namespace(), Some(OPF_NS) | None)
        })
        .ok_or_else(|| {
            crate::Error::invalid_data("parse OPF", &opf_path, "missing metadata element")
        })?;

    let mut title = String::new();
    let mut authors = Vec::new();
    let mut publisher = String::new();
    let mut date = String::new();
    let mut language = String::new();
    let mut subjects = Vec::new();
    let mut description = String::new();
    let mut identifiers = Vec::new();
    let mut rating = String::new();
    let mut series = String::new();
    let mut series_index = String::new();
    let mut unrecognized = Vec::new();

    for child in metadata_node.children().filter(|n| n.is_element()) {
        let ns = child.tag_name().namespace().unwrap_or_default();
        let local = child.tag_name().name();
        let text = child
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect::<String>();
        let text = text.trim();

        match (ns, local) {
            (DC_NS, "title") => {
                if title.is_empty() && !text.is_empty() {
                    title = text.to_owned();
                }
            }
            (DC_NS, "creator") => {
                if !text.is_empty() {
                    authors.push(text.to_string());
                }
            }
            (DC_NS, "publisher") => {
                publisher = text.to_owned();
            }
            (DC_NS, "date") => {
                date = text.to_owned();
                if let Some(idx) = date.find('T') {
                    date.truncate(idx);
                }
            }
            (DC_NS, "language") => {
                language = text.to_owned();
            }
            (DC_NS, "subject") => {
                if !text.is_empty() {
                    subjects.push(text.to_string());
                }
            }
            (DC_NS, "description") => {
                // Escaped/CDATA HTML is text; inline XHTML must retain its tags.
                let mut html = String::new();
                for node in child.children() {
                    if node.is_text() {
                        html.push_str(node.text().unwrap_or_default());
                    } else if node.is_element() {
                        html.push_str(&xml[node.range()]);
                    }
                }
                description = strip_html(&html);
            }
            (DC_NS, "identifier") => {
                let scheme = child
                    .attribute((OPF_NS, "scheme"))
                    .unwrap_or("ID")
                    .to_string();
                if scheme != "calibre" && scheme != "uuid" && !text.is_empty() {
                    identifiers.push((scheme, text.to_string()));
                }
            }
            (OPF_NS | "", "meta") => {
                let name = child.attribute("name").unwrap_or_default();
                let content = child.attribute("content").unwrap_or_default();
                match name {
                    "calibre:rating" => rating = content.to_string(),
                    "calibre:series" => series = content.to_string(),
                    "calibre:series_index" => series_index = content.to_string(),
                    other if !other.is_empty() => {
                        unrecognized.push(format!("meta[{}] = {}", other, content));
                    }
                    _ => unrecognized.push(format!(
                        "meta[property={}] = {}",
                        child.attribute("property").unwrap_or_default(),
                        text
                    )),
                }
            }
            (ns, tag) => {
                let label = if ns.is_empty() {
                    tag.to_string()
                } else {
                    format!("{{{ns}}}{tag}")
                };
                unrecognized.push(label);
            }
        }
    }

    Ok(Some(Metadata {
        title,
        authors,
        publisher,
        date,
        language,
        subjects,
        description,
        identifiers,
        rating,
        series,
        series_index,
        unrecognized,
    }))
}

fn strip_html(input: &str) -> String {
    use kuchikiki::{NodeData, iter::NodeEdge, traits::TendrilSink};
    let doc = kuchikiki::parse_html().one(input);
    let mut result = String::new();
    let mut hidden = 0usize;
    for edge in doc.document_node.traverse() {
        let (node, start) = match edge {
            NodeEdge::Start(node) => (node, true),
            NodeEdge::End(node) => (node, false),
        };
        match node.data() {
            NodeData::Element(element) => {
                let tag = element.name.local.as_ref();
                if matches!(tag, "script" | "style" | "head" | "template") {
                    if start {
                        hidden += 1;
                    } else {
                        hidden -= 1;
                    }
                } else if hidden == 0 {
                    let breaks = match tag {
                        "p" | "div" | "section" | "article" | "blockquote" | "h1" | "h2" | "h3"
                        | "h4" | "h5" | "h6" => 2,
                        "br" | "li" | "tr" | "hr" => 1,
                        _ => 0,
                    };
                    if tag == "br" && start {
                        result.push('\n');
                    } else if breaks > 0 && tag != "br" {
                        let trailing = result.chars().rev().take_while(|c| *c == '\n').count();
                        for _ in trailing..breaks {
                            result.push('\n');
                        }
                    } else if matches!(tag, "td" | "th") {
                        result.push(' ');
                    }
                }
            }
            NodeData::Text(text) if start && hidden == 0 => {
                for c in text.borrow().chars() {
                    if c.is_whitespace() {
                        if !result.is_empty() && !result.ends_with([' ', '\n']) {
                            result.push(' ');
                        }
                    } else {
                        result.push(c);
                    }
                }
            }
            _ => {}
        }
    }
    result
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}
