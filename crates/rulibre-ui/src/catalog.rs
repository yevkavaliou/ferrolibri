//! Sorted catalog snapshots, indexed by stable library-scoped IDs.
use crate::app::Filter;
use rulibre_core::book::{Book, BookId, Format};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};
#[derive(Default, Debug)]
pub struct SearchIndex(Vec<(BookId, String, String)>);
impl SearchIndex {
    pub fn search(&self, query: &str) -> Vec<BookId> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.0
            .iter()
            .filter(|(_, title, author)| title.contains(&query) || author.contains(&query))
            .map(|(id, _, _)| id.clone())
            .collect()
    }
}
#[derive(Debug)]
pub struct Entry {
    pub book: Book,
    pub formats: String,
}
#[derive(Default, Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub order: Vec<BookId>,
    index: BTreeMap<BookId, usize>,
    pub formats: BTreeMap<Format, usize>,
    /// Normalized author -> (first spelling in catalog order, book count).
    pub authors: BTreeMap<String, (String, usize)>,
    pub search_index: Arc<SearchIndex>,
}
impl Catalog {
    pub fn new(books: Vec<Book>) -> Self {
        let mut keyed: Vec<_> = books
            .into_iter()
            .map(|b| {
                (
                    (
                        b.author.to_lowercase(),
                        b.title.to_lowercase(),
                        b.id.clone(),
                    ),
                    b,
                )
            })
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        let entries: Vec<_> = keyed
            .into_iter()
            .map(|(_, book)| {
                let mut formats: Vec<_> = book.files.iter().map(|f| f.format.label()).collect();
                formats.sort_unstable();
                formats.dedup();
                Entry {
                    book,
                    formats: formats.join(" · "),
                }
            })
            .collect();
        let order: Vec<_> = entries.iter().map(|e| e.book.id.clone()).collect();
        let index = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let mut formats = BTreeMap::new();
        let mut authors = BTreeMap::new();
        for entry in &entries {
            for format in entry
                .book
                .files
                .iter()
                .map(|f| f.format)
                .collect::<BTreeSet<_>>()
            {
                *formats.entry(format).or_insert(0) += 1;
            }
            let author = authors
                .entry(entry.book.author.to_lowercase())
                .or_insert_with(|| (entry.book.author.clone(), 0));
            author.1 += 1;
        }
        let search_index = Arc::new(SearchIndex(
            entries
                .iter()
                .map(|e| {
                    (
                        e.book.id.clone(),
                        e.book.title.to_lowercase(),
                        e.book.author.to_lowercase(),
                    )
                })
                .collect(),
        ));
        Self {
            entries,
            order,
            index,
            formats,
            authors,
            search_index,
        }
    }
    pub fn get(&self, id: &BookId) -> Option<&Entry> {
        self.index.get(id).map(|&i| &self.entries[i])
    }
    pub fn filter(&self, filter: &Filter) -> Vec<BookId> {
        let author = match filter {
            Filter::Author(a) => a.to_lowercase(),
            _ => String::new(),
        };
        self.entries
            .iter()
            .filter(|e| match filter {
                Filter::All => true,
                Filter::Format(f) => e.book.files.iter().any(|file| file.format == *f),
                Filter::Author(_) => e.book.author.to_lowercase() == author,
            })
            .map(|e| e.book.id.clone())
            .collect()
    }
}
pub const ROW_HEIGHT: f32 = 36.0;
pub const OVERSCAN: usize = 3;
pub fn visible_slice(count: usize, offset: f32, height: f32) -> Range<usize> {
    visible_slice_with_height(count, offset, height, ROW_HEIGHT)
}
pub fn visible_slice_with_height(
    count: usize,
    offset: f32,
    height: f32,
    row_height: f32,
) -> Range<usize> {
    let height = height.max(0.0);
    let offset = offset
        .max(0.0)
        .min((count as f32 * row_height - height).max(0.0));
    let start = (offset / row_height).floor() as usize;
    let end = ((offset + height) / row_height).ceil() as usize;
    start.saturating_sub(OVERSCAN).min(count)..end.saturating_add(OVERSCAN).min(count)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facets_count_books_once_and_normalize_authors() {
        use rulibre_core::book::BookFile;
        let mut a = book(1, "ŁUKASZ", "Математика");
        a.files = [Format::Epub, Format::Epub, Format::Pdf]
            .map(|format| BookFile {
                format,
                path: "file".into(),
            })
            .to_vec();
        let mut b = book(2, "łukasz", "Żółw");
        b.files = vec![BookFile {
            format: Format::Pdf,
            path: "file".into(),
        }];
        let c = Catalog::new(vec![a, b, book(3, "Другой", "Other")]);
        assert_eq!(c.formats, [(Format::Epub, 1), (Format::Pdf, 2)].into());
        assert_eq!(c.authors["łukasz"].1, 2);
        assert_eq!(c.filter(&Filter::Author("ŁuKaSz".into())).len(), 2);
        assert_eq!(
            c.filter(&Filter::Format(Format::Epub)),
            vec![BookId::Calibre(1)]
        );
        assert!(c.filter(&Filter::Format(Format::Mobi)).is_empty());
        assert_eq!(c.search_index.search("МАТЕМ"), vec![BookId::Calibre(1)]);
        assert_eq!(c.search_index.search("ŻÓŁ"), vec![BookId::Calibre(2)]);
        assert_eq!(c.search_index.search("ŁUKASZ").len(), 2);
        assert!(c.search_index.search(" ").is_empty());
        assert!(c.search_index.search("zolw").is_empty());
    }
    #[test]
    fn twenty_thousand_search_budget() {
        let c = Catalog::new(
            (0..20_000)
                .map(|id| book(id, "Автор Łódź", &format!("Книга {id:05}")))
                .collect(),
        );
        let start = std::time::Instant::now();
        for query in ["К", "Кн", "Кни", "Книга", "Книга 1", "ŁÓDŹ", "missing"] {
            let results = c.search_index.search(query);
            assert!(results.len() <= 20_000);
        }
        eprintln!("20,000 catalog, seven queries: {:?}", start.elapsed());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    pub fn book(id: u64, author: &str, title: &str) -> Book {
        Book {
            id: BookId::Calibre(id),
            author: author.into(),
            title: title.into(),
            files: vec![],
            path: format!("book{id}").into(),
        }
    }
    #[test]
    fn unicode_sort_and_index() {
        let c = Catalog::new(vec![
            book(1, "МОРИС", "Я"),
            book(2, "морис", "А"),
            book(3, "Łukasz", "Ż"),
            book(4, "ALICE", "z"),
            book(5, "alice", "A"),
        ]);
        assert_eq!(c.order, [5, 4, 3, 2, 1].map(BookId::Calibre));
        assert_eq!(c.get(&BookId::Calibre(2)).unwrap().book.title, "А");
        assert!(c.get(&BookId::Calibre(99)).is_none());
    }
    #[test]
    fn viewport_boundaries() {
        assert_eq!(visible_slice(0, 0.0, 400.0), 0..0);
        assert_eq!(visible_slice(100, 0.0, 360.0), 0..13);
        assert_eq!(visible_slice(100, 36.5, 360.0), 0..15);
        assert_eq!(visible_slice(100, 1e9, 360.0), 87..100);
        assert_eq!(visible_slice(2, -30.0, 360.0), 0..2);
    }
    #[test]
    fn twenty_thousand_catalog_budget() {
        let books = (0..20_000)
            .rev()
            .map(|i| book(i, "Автор Ł", "Title"))
            .collect();
        let start = std::time::Instant::now();
        let c = Catalog::new(books);
        eprintln!("20,000 catalog: {:?}", start.elapsed());
        assert_eq!(c.entries.len(), 20_000);
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }
}
