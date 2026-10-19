//! The name the app gives a downloaded file. It is built from the title of the result and the
//! format of the offer; a name from the plugin, from the URL or from `Content-Disposition` never
//! reaches the file system.

use ferrolibri_core::book::Format;

use crate::Archive;

/// Longest cleaned title, in characters. Calibre's own path limit on Windows is far tighter than
/// any file system's, and the file lives in a temporary folder only for the length of the job.
pub const MAX_NAME_CHARS: usize = 80;

const FALLBACK: &str = "book";

/// `<cleaned title>.<extension>`; `.fb2.zip` for an FB2 offer wrapped in a zip.
pub fn file_name(title: &str, format: Format, archive: Option<Archive>) -> String {
    format!("{}.{}", clean_title(title), extension(format, archive))
}

pub fn extension(format: Format, archive: Option<Archive>) -> &'static str {
    match (format, archive) {
        (Format::Fb2, Some(Archive::Zip)) => "fb2.zip",
        (Format::Fb2, None) => "fb2",
        (Format::Epub, _) => "epub",
        (Format::Mobi, _) => "mobi",
        (Format::Azw3, _) => "azw3",
        (Format::Pdf, _) => "pdf",
        (Format::Txt, _) => "txt",
        (Format::Docx, _) => "docx",
        (Format::Kepub, _) => "kepub.epub",
    }
}

/// A title that is safe as a file name on every system the app runs on: no control or
/// direction-changing characters, no path separators or characters Windows forbids, no leading
/// or trailing dots and spaces, not a Windows device name, not longer than [`MAX_NAME_CHARS`].
fn clean_title(title: &str) -> String {
    let mut cleaned = String::new();
    let mut gap = false;
    for c in title.chars() {
        if c.is_whitespace() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            gap = !cleaned.is_empty();
            continue;
        }
        if c.is_control() || is_invisible(c) {
            continue;
        }
        if gap {
            cleaned.push(' ');
            gap = false;
        }
        cleaned.push(c);
    }
    let cut: String = cleaned.chars().take(MAX_NAME_CHARS).collect();
    let cut = cut.trim_matches(|c| c == '.' || c == ' ');
    if cut.is_empty() {
        return FALLBACK.into();
    }
    if is_device_name(cut) {
        return format!("_{cut}");
    }
    cut.to_owned()
}

/// Zero-width and direction-control characters (they can disguise an extension).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}' | '\u{FEFF}'
    )
}

/// `CON`, `NUL`, `COM1`, `LPT1`, ... — Windows treats them as devices whatever follows the
/// first dot, so the part before the first dot is what counts.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let stem = stem.to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(number.chars().collect::<Vec<_>>().as_slice(), [d] if d.is_ascii_digit() || matches!(d, '¹' | '²' | '³'))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(title: &str) -> String {
        file_name(title, Format::Epub, None)
    }

    #[test]
    fn an_ordinary_title_is_kept_with_the_extension_of_the_format() {
        assert_eq!(name("Война и мир"), "Война и мир.epub");
        assert_eq!(file_name("A", Format::Pdf, None), "A.pdf");
        assert_eq!(file_name("A", Format::Fb2, None), "A.fb2");
        assert_eq!(file_name("A", Format::Fb2, Some(Archive::Zip)), "A.fb2.zip");
    }

    #[test]
    fn path_separators_and_forbidden_characters_cannot_make_a_path() {
        for title in [
            "../../evil",
            "..\\..\\evil",
            "C:\\Windows\\x",
            "/etc/passwd",
            "a|b<c>d?e*f\"g",
        ] {
            let cleaned = name(title);
            assert!(
                !cleaned.contains(['/', '\\', ':', '|', '<', '>', '?', '*', '"']),
                "{title:?} -> {cleaned:?}"
            );
            assert!(!cleaned.starts_with('.'), "{cleaned:?}");
        }
        assert_eq!(name("../../evil"), "evil.epub");
    }

    #[test]
    fn control_and_direction_characters_are_dropped() {
        assert_eq!(name("a\0b\r\nc\u{7}d"), "ab cd.epub");
        // A right-to-left override can make `x.exe` look like `x.txt`.
        assert_eq!(name("book\u{202E}fdp"), "bookfdp.epub");
        assert_eq!(name("zero\u{200B}width"), "zerowidth.epub");
    }

    #[test]
    fn windows_device_names_are_not_used_as_they_are() {
        for title in [
            "CON", "nul", "Aux", "COM1", "lpt9", "con.txt", "NUL ..", "COM²",
        ] {
            assert!(name(title).starts_with('_'), "{title} -> {}", name(title));
        }
        // Only the exact device names; neighbours are fine.
        assert_eq!(name("CONSOLE"), "CONSOLE.epub");
        assert_eq!(name("COM10"), "COM10.epub");
    }

    #[test]
    fn trailing_dots_and_spaces_go_because_windows_would_drop_them_silently() {
        assert_eq!(name("Title. . "), "Title.epub");
        assert_eq!(name("   ...   "), "book.epub");
        assert_eq!(name(""), "book.epub");
        assert_eq!(name("\u{0}\u{1}"), "book.epub");
    }

    #[test]
    fn a_long_title_is_cut_by_characters_not_bytes() {
        let long = "ж".repeat(500);
        let cleaned = name(&long);
        assert_eq!(cleaned.chars().count(), MAX_NAME_CHARS + ".epub".len());
        assert!(cleaned.ends_with(".epub"));
    }

    #[test]
    fn spaces_are_collapsed() {
        assert_eq!(
            name("  Many   spaces\tand\ttabs "),
            "Many spaces and tabs.epub"
        );
    }
}
