//! Read-only source discovery and bounded thumbnail decoding, off the UI thread.
use iced::widget::image::Handle;
use rulibre_core::{book::BookId, cover::CoverFile};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub const WIDTH: u32 = 192;
pub const HEIGHT: u32 = 264;
pub const CAPACITY: usize = 300;
pub const CONCURRENCY: usize = 4;

pub fn cache_key(id: &BookId, cover: &CoverFile) -> String {
    let modified = cover
        .modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Include the absolute source path: Calibre IDs are library-scoped.
    format!(
        "{:x}",
        Sha256::digest(format!(
            "v1-{WIDTH}-{HEIGHT}|{id:?}|{:?}|{}|{}|{}",
            cover.path,
            modified.as_secs(),
            modified.subsec_nanos(),
            cover.len
        ))
    )
}

pub fn load(id: &BookId, book: &Path, cache: &Path) -> Result<Option<Handle>, String> {
    let Some(cover) = rulibre_core::cover::find_cover(book).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = cache.join(format!("{}.png", cache_key(id, &cover)));
    if let Ok(pixels) = decode(&path) {
        return Ok(Some(handle(pixels)));
    }
    let pixels = decode(&cover.path)?;
    // Cache failure does not prevent showing a valid cover. Atomic replacement
    // avoids partial PNGs when multiple application instances share the cache.
    let mut bytes = std::io::Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgba8(pixels.clone())
        .write_to(&mut bytes, image::ImageFormat::Png)
        .is_ok()
        && let Err(error) = rulibre_core::config::write_atomic(&path, bytes.get_ref())
    {
        log::warn!("Could not cache cover {}: {error}", path.display());
    }
    Ok(Some(handle(pixels)))
}
fn read(path: &Path) -> Result<image::DynamicImage, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn decode(path: &Path) -> Result<image::RgbaImage, String> {
    read(path).map(|image| image.thumbnail(WIDTH, HEIGHT).into_rgba8())
}
fn handle(pixels: image::RgbaImage) -> Handle {
    Handle::from_rgba(pixels.width(), pixels.height(), pixels.into_raw())
}
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rulibre/covers")
}

#[derive(Default)]
pub struct CoverCache {
    entries: BTreeMap<BookId, (Option<Handle>, u64)>,
    clock: u64,
    pub generation: u64,
    pub wanted: Vec<BookId>,
    pub running: BTreeMap<BookId, u64>,
}
impl CoverCache {
    pub fn get(&self, id: &BookId) -> Option<&Handle> {
        self.entries.get(id).and_then(|(h, _)| h.as_ref())
    }
    pub fn contains(&self, id: &BookId) -> bool {
        self.entries.contains_key(id)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn insert(&mut self, id: BookId, handle: Option<Handle>) {
        self.clock += 1;
        self.entries.insert(id, (handle, self.clock));
        while self.entries.len() > CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, age))| age)
                .unwrap()
                .0
                .clone();
            self.entries.remove(&oldest);
        }
    }
    pub fn request(&mut self, wanted: Vec<BookId>) {
        if self.wanted != wanted {
            self.generation += 1;
            self.wanted = wanted;
        }
        for id in &self.wanted {
            if let Some((_, age)) = self.entries.get_mut(id) {
                self.clock += 1;
                *age = self.clock;
            }
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.wanted.clear();
        self.generation += 1;
        // Retain outstanding slots until completion: rescans cannot start
        // another four decoders while the previous workers are still alive.
    }
    pub fn next_requests(&mut self) -> Vec<(BookId, u64)> {
        let ids: Vec<_> = self
            .wanted
            .iter()
            .filter(|id| !self.contains(id) && !self.running.contains_key(id))
            .take(CONCURRENCY.saturating_sub(self.running.len()))
            .cloned()
            .collect();
        ids.into_iter()
            .map(|id| {
                self.running.insert(id.clone(), self.generation);
                (id, self.generation)
            })
            .collect()
    }
    pub fn finish(&mut self, id: BookId, generation: u64, handle: Option<Handle>) -> bool {
        if self.running.get(&id) != Some(&generation) {
            return false;
        }
        self.running.remove(&id);
        if generation != self.generation || !self.wanted.contains(&id) {
            return false;
        }
        self.insert(id, handle);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_invalidation_and_library_namespace() {
        let id = BookId::Calibre(1);
        let mut cover = CoverFile {
            path: "library/cover.jpg".into(),
            modified: UNIX_EPOCH,
            len: 1,
        };
        let key = cache_key(&id, &cover);
        cover.len = 2;
        assert_ne!(key, cache_key(&id, &cover));
        cover.len = 1;
        cover.modified += std::time::Duration::from_nanos(100);
        assert_ne!(key, cache_key(&id, &cover));
        cover.modified = UNIX_EPOCH;
        cover.path = "other-library/cover.jpg".into();
        assert_ne!(key, cache_key(&id, &cover));
    }
    #[test]
    fn lru_and_fast_scroll_keep_memory_and_workers_bounded() {
        let mut cache = CoverCache::default();
        for id in 0..CAPACITY as u64 {
            cache.insert(BookId::Calibre(id), None);
        }
        cache.request(vec![BookId::Calibre(0)]);
        cache.insert(BookId::Calibre(999), None);
        assert!(cache.contains(&BookId::Calibre(0)));
        assert!(!cache.contains(&BookId::Calibre(1)));
        assert_eq!(cache.len(), CAPACITY);
        cache.request((1000..1100).map(BookId::Calibre).collect());
        let running = cache.next_requests();
        assert_eq!(running.len(), CONCURRENCY);
        cache.request((2000..2100).map(BookId::Calibre).collect());
        assert!(cache.next_requests().is_empty());
        for (id, generation) in running {
            assert!(!cache.finish(id, generation, None));
        }
        assert_eq!(cache.next_requests().len(), CONCURRENCY);
        cache.clear();
        assert_eq!(cache.running.len(), CONCURRENCY);
    }
    #[test]
    fn oversized_images_are_rejected_before_large_decode() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wide.png");
        image::RgbImage::new(12_001, 1).save(&path).unwrap();
        assert!(decode(&path).is_err());
    }
    #[test]
    fn disk_cache_reused_and_corruption_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let book = temp.path().join("book");
        let cache = temp.path().join("cache");
        std::fs::create_dir(&book).unwrap();
        let source = book.join("cover.jpg");
        image::RgbImage::from_pixel(400, 600, image::Rgb([200, 30, 60]))
            .save(&source)
            .unwrap();
        let id = BookId::Calibre(1);
        assert!(load(&id, &book, &cache).unwrap().is_some());
        let original_source = std::fs::read(&source).unwrap();
        let cached = std::fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = std::fs::metadata(&cached).unwrap().modified().unwrap();
        assert!(load(&id, &book, &cache).unwrap().is_some());
        assert_eq!(
            before,
            std::fs::metadata(&cached).unwrap().modified().unwrap()
        );
        std::fs::write(&cached, b"broken").unwrap();
        assert!(load(&id, &book, &cache).unwrap().is_some());
        assert!(decode(&cached).is_ok());
        assert_eq!(std::fs::read(&source).unwrap(), original_source);
        // A replaced source must create a new key instead of reusing stale pixels.
        image::RgbImage::from_pixel(600, 400, image::Rgb([30, 200, 60]))
            .save(&source)
            .unwrap();
        assert!(load(&id, &book, &cache).unwrap().is_some());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 2);
        let blocked_cache = temp.path().join("blocked-cache");
        std::fs::write(&blocked_cache, b"not a directory").unwrap();
        assert!(load(&id, &book, &blocked_cache).unwrap().is_some());
        assert_eq!(std::fs::read(&blocked_cache).unwrap(), b"not a directory");
        std::fs::write(&source, b"broken").unwrap();
        assert!(load(&id, &book, &cache).is_err());
        assert!(load(&id, temp.path(), &cache).unwrap().is_none());
    }
}
