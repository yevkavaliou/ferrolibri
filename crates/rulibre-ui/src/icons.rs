//! Embedded Lucide SVGs (ISC; assets/icons/LICENSE). No network at runtime.
use iced::{Color, widget::svg};
macro_rules! icon {
    ($body:literal) => { concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, $body, "</svg>").as_bytes() };
}
pub const HOME: &[u8] =
    icon!(r#"<path d="m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1Z"/>"#);
pub const SEARCH: &[u8] = icon!(r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#);
pub const BOOKS: &[u8] = icon!(
    r#"<rect x="3" y="3" width="5" height="18" rx="1"/><rect x="8" y="3" width="5" height="18" rx="1"/><path d="m16 3 5 17-5 1-5-17Z"/>"#
);
pub const LIST_CHECK: &[u8] =
    icon!(r#"<path d="m3 5 2 2 4-4M12 5h9m-9 7h9m-9 7h9M3 12h3m-3 7h3"/>"#);
pub const SETTINGS: &[u8] = icon!(
    r#"<path d="m9 3-1 3-3 1-2 4 2 2v4l4 2 3-1 3 1 4-2v-4l2-2-2-4-3-1-1-3Z"/><circle cx="12" cy="12" r="3"/>"#
);
pub const CHEVRON_RIGHT: &[u8] = icon!(r#"<path d="m9 6 6 6-6 6"/>"#);
pub const CHEVRON_DOWN: &[u8] = icon!(r#"<path d="m6 9 6 6 6-6"/>"#);
pub const FOLDER: &[u8] = icon!(
    r#"<path d="M3 7V5a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>"#
);
pub const CHECK: &[u8] = icon!(r#"<path d="m5 12 4 4L19 6"/>"#);
pub const SIDEBAR: &[u8] =
    icon!(r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>"#);
pub const SIDEBAR_RIGHT: &[u8] =
    icon!(r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M15 3v18"/>"#);
pub const LIST: &[u8] = icon!(r#"<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>"#);
pub const GRID: &[u8] = icon!(
    r#"<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>"#
);
pub const MINIMIZE: &[u8] = icon!(r#"<path d="M5 12h14"/>"#);
pub const MAXIMIZE: &[u8] = icon!(r#"<rect x="5" y="5" width="14" height="14" rx="1"/>"#);
pub const X: &[u8] = icon!(r#"<path d="m18 6-12 12M6 6l12 12"/>"#);
pub const REFRESH: &[u8] =
    icon!(r#"<path d="M3 12a9 9 0 0 1 15-6l3 3M21 3v6h-6m6 3a9 9 0 0 1-15 6l-3-3m6 0H3v6"/>"#);
pub fn tinted(data: &'static [u8], color: Color) -> svg::Svg<'static> {
    svg(svg::Handle::from_memory(data))
        .width(16)
        .height(16)
        .style(move |_, _| svg::Style { color: Some(color) })
}
pub fn view(data: &'static [u8]) -> svg::Svg<'static> {
    tinted(data, crate::theme::DARK.text_secondary)
}
