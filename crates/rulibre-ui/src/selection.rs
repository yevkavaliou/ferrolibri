//! Keyboard/mouse selection in the currently visible order.
use iced::keyboard::Modifiers;
use rulibre_core::book::BookId;
use std::collections::BTreeSet;
#[derive(Default, Debug)]
pub struct Selection {
    pub selected: BTreeSet<BookId>,
    pub anchor: Option<BookId>,
    pub cursor: Option<BookId>,
    order: Vec<BookId>,
}
impl Selection {
    pub fn click(&mut self, id: BookId, modifiers: Modifiers, order: &[BookId]) {
        if !order.contains(&id) {
            return;
        }
        if modifiers.shift() {
            if let Some(a) = self
                .anchor
                .as_ref()
                .and_then(|a| order.iter().position(|i| i == a))
            {
                let b = order.iter().position(|i| i == &id).unwrap();
                self.selected = order[a.min(b)..=a.max(b)].iter().cloned().collect();
            } else {
                self.selected = [id.clone()].into();
                self.anchor = Some(id.clone());
            }
        } else if modifiers.control() || modifiers.logo() {
            if !self.selected.remove(&id) {
                self.selected.insert(id.clone());
            }
            self.anchor = Some(id.clone());
        } else {
            self.selected = [id.clone()].into();
            self.anchor = Some(id.clone());
        }
        self.cursor = Some(id);
        self.order.retain(|id| self.selected.contains(id));
        for id in order {
            if self.selected.contains(id) && !self.order.contains(id) {
                self.order.push(id.clone());
            }
        }
    }
    pub fn select_all(&mut self, order: &[BookId]) {
        self.selected = order.iter().cloned().collect();
        self.anchor = order.first().cloned();
        self.cursor = order.last().cloned();
        self.order = order.to_vec();
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn retain(&mut self, order: &[BookId]) {
        let ids: BTreeSet<_> = order.iter().collect();
        self.selected.retain(|id| ids.contains(id));
        self.order.retain(|id| self.selected.contains(id));
        if self.anchor.as_ref().is_some_and(|id| !ids.contains(id)) {
            self.anchor = None;
        }
        if self.cursor.as_ref().is_some_and(|id| !ids.contains(id)) {
            self.cursor = None;
        }
    }
    pub fn move_cursor(
        &mut self,
        delta: isize,
        modifiers: Modifiers,
        order: &[BookId],
    ) -> Option<usize> {
        if order.is_empty() {
            return None;
        }
        let i = self
            .cursor
            .as_ref()
            .and_then(|id| order.iter().position(|i| i == id))
            .map(|i| i.saturating_add_signed(delta).min(order.len() - 1))
            .unwrap_or(if delta < 0 { order.len() - 1 } else { 0 });
        self.click(order[i].clone(), modifiers, order);
        Some(i)
    }
    /// Selection chronology, with a stable fallback for programmatic selections.
    pub fn ordered(&self) -> Vec<BookId> {
        self.order
            .iter()
            .filter(|id| self.selected.contains(id))
            .cloned()
            .chain(
                self.selected
                    .iter()
                    .filter(|id| !self.order.contains(id))
                    .cloned(),
            )
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ids() -> Vec<BookId> {
        (0..5).map(BookId::Calibre).collect()
    }
    #[test]
    fn mouse_modifiers_and_visible_range() {
        let o = ids();
        let mut s = Selection::default();
        s.click(o[1].clone(), Modifiers::empty(), &o);
        s.click(o[3].clone(), Modifiers::CTRL, &o);
        assert_eq!(s.selected, [o[1].clone(), o[3].clone()].into());
        s.click(o[3].clone(), Modifiers::LOGO, &o);
        assert_eq!(s.selected, [o[1].clone()].into());
        s.click(o[0].clone(), Modifiers::SHIFT | Modifiers::CTRL, &o);
        assert_eq!(s.selected, o[..4].iter().cloned().collect());
        let f = vec![o[0].clone(), o[3].clone()];
        s.click(o[0].clone(), Modifiers::SHIFT, &f);
        assert_eq!(s.selected, f.into_iter().collect());
    }
    #[test]
    fn disappearing_anchor_and_hidden_selection() {
        let o = ids();
        let mut s = Selection::default();
        s.click(o[1].clone(), Modifiers::empty(), &o);
        s.retain(&o);
        assert_eq!(s.selected, [o[1].clone()].into());
        s.click(o[4].clone(), Modifiers::SHIFT, &o[2..]);
        assert_eq!(s.selected, [o[4].clone()].into());
        s.retain(&o[..4]);
        assert!(s.selected.is_empty());
        assert!(s.anchor.is_none());
        s.click(o[0].clone(), Modifiers::SHIFT, &o);
        assert_eq!(s.anchor, Some(o[0].clone()));
    }
    #[test]
    fn keyboard_ranges_clear_all_boundaries() {
        let o = ids();
        let mut s = Selection::default();
        assert_eq!(s.move_cursor(1, Modifiers::empty(), &o), Some(0));
        s.move_cursor(1, Modifiers::SHIFT, &o);
        s.move_cursor(1, Modifiers::SHIFT, &o);
        assert_eq!(s.selected, o[..3].iter().cloned().collect());
        s.move_cursor(-1, Modifiers::SHIFT, &o);
        assert_eq!(s.selected, o[..2].iter().cloned().collect());
        s.select_all(&o);
        assert_eq!(s.selected.len(), 5);
        s.clear();
        assert!(s.anchor.is_none());
        assert_eq!(s.move_cursor(1, Modifiers::empty(), &[]), None);
    }
}
