//! Undo/Redo snapshots for the open stamp.

use crate::stamp::Stamp;

const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub stamp: Stamp,
    pub selected_building: Option<usize>,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    limit: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: DEFAULT_LIMIT,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn push(&mut self, snap: Snapshot) {
        self.undo.push(snap);
        if self.undo.len() > self.limit {
            let drop_n = self.undo.len() - self.limit;
            self.undo.drain(0..drop_n);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }

    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filedb::{DocumentOrdered, Version};

    fn empty_stamp() -> Stamp {
        Stamp {
            document: DocumentOrdered {
                version: Version::V3,
                roots: vec![],
                tags: Default::default(),
                attribs: Default::default(),
            },
            buildings: vec![],
            polygon_modules: vec![],
            polygon_cells: vec![],
            polygon_area: Default::default(),
            stamp_version: 1,
        }
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut h = History::new();
        let a = Snapshot {
            stamp: empty_stamp(),
            selected_building: None,
        };
        let mut b_stamp = empty_stamp();
        b_stamp.stamp_version = 2;
        let b = Snapshot {
            stamp: b_stamp,
            selected_building: Some(0),
        };
        h.push(a.clone());
        let restored = h.undo(b.clone()).unwrap();
        assert_eq!(restored.stamp.stamp_version, 1);
        let again = h.redo(restored).unwrap();
        assert_eq!(again.stamp.stamp_version, 2);
        assert_eq!(again.selected_building, Some(0));
    }
}

