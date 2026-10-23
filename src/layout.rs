//! Split layout for the panes of one tab: a binary tree of splits whose
//! leaves are panes, plus the math to turn it into screen rectangles.

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

use crate::pane::PaneId;

/// How a split divides its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// Children side by side, separated by a vertical line (`Ctrl+Shift+|`).
    Vertical,
    /// Children stacked, separated by a horizontal line (`Ctrl+Shift+_`).
    Horizontal,
}

/// Saved as-is in session files (see `session`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Node {
    Pane(PaneId),
    /// `first` is left/top. Nested splits in the same direction share their
    /// area evenly (see `compute`).
    Split {
        split: Split,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// A 1-cell line drawn between the two halves of a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divider {
    pub area: Rect,
    pub split: Split,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Computed {
    pub panes: Vec<(PaneId, Rect)>,
    pub dividers: Vec<Divider>,
}

impl Computed {
    pub fn rect_of(&self, id: PaneId) -> Option<Rect> {
        self.panes.iter().find(|(p, _)| *p == id).map(|(_, r)| *r)
    }

    pub fn pane_at(&self, x: u16, y: u16) -> Option<(PaneId, Rect)> {
        self.panes.iter().copied().find(|(_, r)| contains(*r, x, y))
    }
}

pub fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

impl Node {
    /// Pane ids in tree order (left-to-right / top-to-bottom).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Pane(id) => out.push(*id),
            Node::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// Replace the leaf `target` with a split of `target` and `new`
    /// (new goes right/below). Returns false if `target` isn't in the tree.
    pub fn split(&mut self, target: PaneId, split: Split, new: PaneId) -> bool {
        match self {
            Node::Pane(id) if *id == target => {
                *self = Node::Split {
                    split,
                    first: Box::new(Node::Pane(target)),
                    second: Box::new(Node::Pane(new)),
                };
                true
            }
            Node::Pane(_) => false,
            Node::Split { first, second, .. } => {
                first.split(target, split, new) || second.split(target, split, new)
            }
        }
    }

    /// Remove the leaf `target`; its sibling takes over the parent's space.
    /// Returns `None` if the tree is now empty.
    pub fn remove(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Pane(id) if id == target => None,
            Node::Pane(_) => Some(self),
            Node::Split {
                split,
                first,
                second,
            } => match (first.remove(target), second.remove(target)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    split,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// Compute each pane's rectangle within `area`.
    ///
    /// A chain of splits in the same direction counts as one row (or column)
    /// of equal slots, so splitting a pane of `[1 | 2]` sideways gives three
    /// equal columns instead of 50/25/25.
    pub fn compute(&self, area: Rect) -> Computed {
        let mut out = Computed::default();
        self.compute_into(area, &mut out);
        out
    }

    fn compute_into(&self, area: Rect, out: &mut Computed) {
        match self {
            Node::Pane(id) => out.panes.push((*id, area)),
            Node::Split { split, .. } => {
                let mut slots = Vec::new();
                self.slots(*split, &mut slots);
                let (rects, dividers) = split_rect(area, *split, slots.len());
                out.dividers.extend(dividers.into_iter().map(|d| Divider {
                    area: d,
                    split: *split,
                }));
                for (node, rect) in slots.into_iter().zip(rects) {
                    node.compute_into(rect, out);
                }
            }
        }
    }

    /// The children that share this node's area along `dir`: nested splits
    /// in the same direction are flattened, anything else is one slot.
    fn slots<'a>(&'a self, dir: Split, out: &mut Vec<&'a Node>) {
        match self {
            Node::Split {
                split,
                first,
                second,
            } if *split == dir => {
                first.slots(dir, out);
                second.slots(dir, out);
            }
            _ => out.push(self),
        }
    }
}

/// Split `area` into `n` equal parts with a 1-cell divider between each
/// pair. Leftover cells go to the last parts. When the area is too small for
/// dividers they are dropped; parts may be empty.
pub fn split_rect(area: Rect, split: Split, n: usize) -> (Vec<Rect>, Vec<Rect>) {
    let n = n.max(1) as u16;
    let len = match split {
        Split::Vertical => area.width,
        Split::Horizontal => area.height,
    };
    let divider_len = u16::from(len >= 2 * n - 1);
    let avail = len - divider_len * (n - 1);
    let (base, extra) = (avail / n, avail % n);

    let part = |offset: u16, size: u16| match split {
        Split::Vertical => Rect::new(area.x + offset, area.y, size, area.height),
        Split::Horizontal => Rect::new(area.x, area.y + offset, area.width, size),
    };
    let mut parts = Vec::new();
    let mut dividers = Vec::new();
    let mut offset = 0;
    for i in 0..n {
        if i > 0 && divider_len == 1 {
            dividers.push(part(offset, 1));
            offset += 1;
        }
        let size = base + u16::from(i >= n - extra);
        parts.push(part(offset, size));
        offset += size;
    }
    (parts, dividers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: PaneId) -> Box<Node> {
        Box::new(Node::Pane(id))
    }

    #[test]
    fn split_rect_vertical_reserves_divider() {
        let (p, d) = split_rect(Rect::new(10, 0, 81, 20), Split::Vertical, 2);
        assert_eq!(p, vec![Rect::new(10, 0, 40, 20), Rect::new(51, 0, 40, 20)]);
        assert_eq!(d, vec![Rect::new(50, 0, 1, 20)]);
    }

    #[test]
    fn split_rect_even_width_gives_extra_to_second() {
        let (p, _) = split_rect(Rect::new(0, 0, 80, 10), Split::Vertical, 2);
        assert_eq!((p[0].width, p[1].width), (39, 40));
    }

    #[test]
    fn split_rect_horizontal() {
        let (p, d) = split_rect(Rect::new(0, 2, 30, 11), Split::Horizontal, 2);
        assert_eq!(p, vec![Rect::new(0, 2, 30, 5), Rect::new(0, 8, 30, 5)]);
        assert_eq!(d, vec![Rect::new(0, 7, 30, 1)]);
    }

    #[test]
    fn split_rect_tiny_area_drops_divider() {
        let (p, d) = split_rect(Rect::new(0, 0, 2, 5), Split::Vertical, 2);
        assert!(d.is_empty());
        assert_eq!((p[0].width, p[1].width), (1, 1));
        let (p, d) = split_rect(Rect::new(0, 0, 0, 5), Split::Vertical, 3);
        assert!(d.is_empty());
        assert!(p.iter().all(|r| r.width == 0));
    }

    #[test]
    fn split_rect_three_equal_parts() {
        // 80 = 3 parts + 2 dividers: 78 / 3 = 26 each.
        let (p, d) = split_rect(Rect::new(0, 0, 80, 10), Split::Vertical, 3);
        assert_eq!(
            p.iter().map(|r| r.width).collect::<Vec<_>>(),
            vec![26, 26, 26]
        );
        assert_eq!(d.iter().map(|r| r.x).collect::<Vec<_>>(), vec![26, 53]);
        // 81: the leftover cell goes to the last part.
        let (p, _) = split_rect(Rect::new(0, 0, 81, 10), Split::Vertical, 3);
        assert_eq!(
            p.iter().map(|r| r.width).collect::<Vec<_>>(),
            vec![26, 26, 27]
        );
    }

    #[test]
    fn split_and_remove_roundtrip() {
        let mut n = Node::Pane(1);
        assert!(n.split(1, Split::Vertical, 2));
        assert!(n.split(2, Split::Horizontal, 3));
        assert!(!n.split(99, Split::Vertical, 4));
        assert_eq!(n.panes(), vec![1, 2, 3]);

        let n = n.remove(2).unwrap();
        assert_eq!(
            n,
            Node::Split {
                split: Split::Vertical,
                first: leaf(1),
                second: leaf(3)
            }
        );
        let n = n.remove(1).unwrap();
        assert_eq!(n, Node::Pane(3));
        assert_eq!(n.remove(3), None);
    }

    #[test]
    fn compute_nested_layout() {
        // [1 | [2 / 3]]
        let n = Node::Split {
            split: Split::Vertical,
            first: leaf(1),
            second: Box::new(Node::Split {
                split: Split::Horizontal,
                first: leaf(2),
                second: leaf(3),
            }),
        };
        let c = n.compute(Rect::new(0, 0, 21, 11));
        assert_eq!(c.rect_of(1), Some(Rect::new(0, 0, 10, 11)));
        assert_eq!(c.rect_of(2), Some(Rect::new(11, 0, 10, 5)));
        assert_eq!(c.rect_of(3), Some(Rect::new(11, 6, 10, 5)));
        assert_eq!(c.dividers.len(), 2);
        assert_eq!(c.pane_at(15, 7).map(|p| p.0), Some(3));
        assert_eq!(c.pane_at(10, 3), None); // on the divider
    }

    #[test]
    fn same_direction_splits_share_evenly() {
        // Split 1 sideways, then split 2 sideways: three equal columns.
        let mut n = Node::Pane(1);
        n.split(1, Split::Vertical, 2);
        n.split(2, Split::Vertical, 3);
        let c = n.compute(Rect::new(0, 0, 80, 10));
        let widths: Vec<u16> = [1, 2, 3].map(|id| c.rect_of(id).unwrap().width).into();
        assert_eq!(widths, vec![26, 26, 26]);
        assert_eq!(c.dividers.len(), 2);

        // A stacked pair inside a row is still one slot: [1 | 2/3] is 50/50.
        let mut n = Node::Pane(1);
        n.split(1, Split::Vertical, 2);
        n.split(2, Split::Horizontal, 3);
        let c = n.compute(Rect::new(0, 0, 81, 10));
        assert_eq!(c.rect_of(1).unwrap().width, 40);
        assert_eq!(c.rect_of(2).unwrap().width, 40);
    }
}
