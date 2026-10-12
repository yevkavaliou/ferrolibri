use crate::types::BookId;

pub fn accepts(
    request: u64,
    current: u64,
    book: &BookId,
    selected: Option<&BookId>,
    visible: bool,
) -> bool {
    visible && request == current && selected == Some(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_request_generation_reject_stale_metadata() {
        let a = BookId::Calibre(1);
        let b = BookId::Calibre(2);
        assert!(accepts(2, 2, &a, Some(&a), true));
        assert!(!accepts(2, 2, &a, Some(&b), true));
        assert!(!accepts(1, 3, &a, Some(&a), true)); // A -> B -> A
        assert!(!accepts(2, 2, &a, Some(&a), false));
        assert!(!accepts(2, 2, &a, None, true));
    }
}
