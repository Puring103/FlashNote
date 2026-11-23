//! Adapted from ProseMirror/prosemirror-transform src/map.ts (_map).
//! Copyright (C) 2015-2017 by Marijn Haverbeke and others. MIT, see reference/LICENSE.
//! This subset maps one replacement; it does not implement collaborative rebasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepMap {
    pub start: usize,
    pub old_size: usize,
    pub new_size: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapResult {
    pub position: usize,
    pub deleted: bool,
}
impl StepMap {
    pub fn map(&self, pos: usize, assoc: i8) -> MapResult {
        let end = self.start + self.old_size;
        if pos < self.start {
            return MapResult {
                position: pos,
                deleted: false,
            };
        }
        if pos <= end {
            let side = if self.old_size == 0 {
                assoc
            } else if pos == self.start {
                -1
            } else if pos == end {
                1
            } else {
                assoc
            };
            MapResult {
                position: self.start + if side < 0 { 0 } else { self.new_size },
                deleted: if assoc < 0 {
                    pos != self.start
                } else {
                    pos != end
                },
            }
        } else {
            MapResult {
                position: pos - self.old_size + self.new_size,
                deleted: false,
            }
        }
    }
    pub fn invert(&self) -> Self {
        Self {
            start: self.start,
            old_size: self.new_size,
            new_size: self.old_size,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insertion_affinity_and_replacement_boundaries() {
        let m = StepMap {
            start: 2,
            old_size: 0,
            new_size: 3,
        };
        assert_eq!(m.map(2, -1).position, 2);
        assert_eq!(m.map(2, 1).position, 5);
        assert_eq!(m.map(4, 1).position, 7);
        let m = StepMap {
            start: 2,
            old_size: 4,
            new_size: 1,
        };
        assert_eq!(m.map(2, 1).position, 2);
        assert_eq!(m.map(6, -1).position, 3);
        assert!(m.map(3, 1).deleted);
        assert!(!m.map(6, 1).deleted);
        assert_eq!(m.invert().map(m.map(9, 1).position, 1).position, 9);
    }
}
