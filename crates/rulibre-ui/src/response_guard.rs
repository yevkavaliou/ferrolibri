use rulibre_core::book::BookId;

/// Generation shared by query changes and catalog replacements.
#[derive(Default)]
pub struct GenerationGuard(u64);
impl GenerationGuard {
    pub fn invalidate(&mut self) -> u64 {
        self.0 = self.0.checked_add(1).expect("generation exhausted");
        self.0
    }
    pub fn current(&self) -> u64 {
        self.0
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.0 == generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestKey {
    pub generation: u64,
    pub book: BookId,
}

/// Each new request invalidates all earlier responses, including A -> B -> A.
#[derive(Default)]
pub struct ResponseGuard {
    generation: u64,
    current: Option<RequestKey>,
}
impl ResponseGuard {
    pub fn request(&mut self, book: BookId) -> RequestKey {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("request generation exhausted");
        let key = RequestKey {
            generation: self.generation,
            book,
        };
        self.current = Some(key.clone());
        key
    }
    pub fn clear(&mut self) {
        self.current = None;
    }
    pub fn accepts(&self, key: &RequestKey) -> bool {
        self.current.as_ref() == Some(key)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_a_b_a_and_hidden_responses() {
        let mut guard = ResponseGuard::default();
        let a = guard.request(BookId::Calibre(1));
        let b = guard.request(BookId::Calibre(2));
        let latest = guard.request(BookId::Calibre(1));
        assert!(!guard.accepts(&a));
        assert!(!guard.accepts(&b));
        assert!(guard.accepts(&latest));
        guard.clear();
        assert!(!guard.accepts(&latest));
    }
}
