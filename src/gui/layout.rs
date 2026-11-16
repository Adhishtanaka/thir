//! Binary split tree of panes. Pane ids are chosen by the caller.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dir { Left, Right, Up, Down }

enum Node { Leaf(usize), Split { side_by_side: bool, a: Box<Node>, b: Box<Node> } }

pub struct Layout { root: Node, pub focus: usize, pub full: bool }

fn walk(n: &Node, r: Rect, gap: i32, out: &mut Vec<(usize, Rect)>) {
    match n {
        Node::Leaf(id) => out.push((*id, r)),
        Node::Split { side_by_side, a, b } => if *side_by_side {
            let w = (r.w - gap) / 2;
            walk(a, Rect { w, ..r }, gap, out);
            walk(b, Rect { x: r.x + w + gap, w: r.w - w - gap, ..r }, gap, out);
        } else {
            let h = (r.h - gap) / 2;
            walk(a, Rect { h, ..r }, gap, out);
            walk(b, Rect { y: r.y + h + gap, h: r.h - h - gap, ..r }, gap, out);
        },
    }
}
fn first(n: &Node) -> usize { match n { Node::Leaf(i) => *i, Node::Split { a, .. } => first(a) } }

fn split_leaf(n: &mut Node, at: usize, side: bool, new: usize) -> bool {
    match n {
        Node::Leaf(i) if *i == at => { *n = Node::Split { side_by_side: side, a: Box::new(Node::Leaf(at)), b: Box::new(Node::Leaf(new)) }; true }
        Node::Leaf(_) => false,
        Node::Split { a, b, .. } => split_leaf(a, at, side, new) || split_leaf(b, at, side, new),
    }
}
/// Removes leaf `id`; returns the pane that should get the focus (first leaf of the sibling).
fn remove(n: &mut Node, id: usize) -> Option<usize> {
    let Node::Split { a, b, .. } = n else { return None };
    let (ia, ib) = (matches!(**a, Node::Leaf(i) if i == id), matches!(**b, Node::Leaf(i) if i == id));
    if ia || ib {
        let Node::Split { a, b, .. } = std::mem::replace(n, Node::Leaf(0)) else { unreachable!() };
        let keep = if ia { *b } else { *a };
        let f = first(&keep); *n = keep; return Some(f);
    }
    remove(a, id).or_else(|| remove(b, id))
}

impl Layout {
    pub fn new(id: usize) -> Layout { Layout { root: Node::Leaf(id), focus: id, full: false } }

    pub fn rects(&self, area: Rect, gap: i32) -> Vec<(usize, Rect)> {
        let mut v = Vec::new();
        if self.full { v.push((self.focus, area)) } else { walk(&self.root, area, gap, &mut v) }
        v
    }
    /// All pane ids in tree order, ignoring fullscreen.
    pub fn ids(&self, area: Rect, gap: i32) -> Vec<usize> { let mut v = Vec::new(); walk(&self.root, area, gap, &mut v); v.into_iter().map(|p| p.0).collect() }

    /// Splits the focused pane along its longer side and focuses `new`.
    pub fn split(&mut self, area: Rect, gap: i32, new: usize) {
        let cur = self.ids_rects(area, gap).into_iter().find(|p| p.0 == self.focus).map_or(area, |p| p.1);
        if split_leaf(&mut self.root, self.focus, cur.w >= cur.h, new) { self.focus = new; self.full = false; }
    }
    fn ids_rects(&self, area: Rect, gap: i32) -> Vec<(usize, Rect)> { let mut v = Vec::new(); walk(&self.root, area, gap, &mut v); v }

    /// False when `id` is the only pane (nothing changes then).
    pub fn close(&mut self, id: usize) -> bool {
        match remove(&mut self.root, id) {
            Some(f) => { if self.focus == id || self.full { self.focus = f; } self.full = false; true }
            None => false,
        }
    }
    pub fn toggle_full(&mut self) { self.full = !self.full; }

    pub fn move_focus(&mut self, area: Rect, gap: i32, d: Dir) {
        let all = self.ids_rects(area, gap);
        let Some(cur) = all.iter().find(|p| p.0 == self.focus).map(|p| p.1) else { return };
        let c = |r: &Rect| (r.x * 2 + r.w, r.y * 2 + r.h);       // doubled center
        let best = all.iter().filter(|p| p.0 != self.focus).filter_map(|(id, r)| {
            let (axis, overlap, perp) = match d {
                Dir::Right => (r.x - (cur.x + cur.w), r.y < cur.y + cur.h && cur.y < r.y + r.h, (c(r).1 - c(&cur).1).abs()),
                Dir::Left => (cur.x - (r.x + r.w), r.y < cur.y + cur.h && cur.y < r.y + r.h, (c(r).1 - c(&cur).1).abs()),
                Dir::Down => (r.y - (cur.y + cur.h), r.x < cur.x + cur.w && cur.x < r.x + r.w, (c(r).0 - c(&cur).0).abs()),
                Dir::Up => (cur.y - (r.y + r.h), r.x < cur.x + cur.w && cur.x < r.x + r.w, (c(r).0 - c(&cur).0).abs()),
            };
            (axis >= 0 && overlap).then_some((axis, perp, *id))
        }).min();
        if let Some((_, _, id)) = best { self.focus = id; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: Rect = Rect { x: 0, y: 0, w: 1000, h: 400 };
    fn ids(l: &Layout) -> Vec<usize> { l.rects(A, 10).iter().map(|p| p.0).collect() }
    #[test] fn single_pane_fills_area() { assert_eq!(Layout::new(1).rects(A, 10), vec![(1, A)]); }
    #[test] fn splits_along_longer_side() {
        let mut l = Layout::new(1); l.split(A, 10, 2);
        let r = l.rects(A, 10);
        assert_eq!(r[0], (1, Rect { x: 0, y: 0, w: 495, h: 400 })); assert_eq!(r[1], (2, Rect { x: 505, y: 0, w: 495, h: 400 }));
        assert_eq!(l.focus, 2);
        l.split(A, 10, 3);   // pane 2 is 495x400: still wider than tall => side by side again
        assert_eq!(l.rects(A, 10)[2].1, Rect { x: 757, y: 0, w: 243, h: 400 });
        l.split(A, 10, 4);   // pane 3 is 243x400: taller => stacked
        assert_eq!(l.rects(A, 10)[3].1, Rect { x: 757, y: 205, w: 243, h: 195 });
        assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    }
    #[test] fn close_gives_space_to_sibling_and_focus() {
        let mut l = Layout::new(1); l.split(A, 10, 2); l.split(A, 10, 3);
        assert!(l.close(3)); assert_eq!(l.focus, 2); assert_eq!(l.rects(A, 10)[1].1, Rect { x: 505, y: 0, w: 495, h: 400 });
        assert!(l.close(1)); assert_eq!(l.rects(A, 10), vec![(2, A)]); assert_eq!(l.focus, 2);
        assert!(!l.close(2));
    }
    #[test] fn close_unfocused_keeps_focus() { let mut l = Layout::new(1); l.split(A, 10, 2); l.close(1); assert_eq!(l.focus, 2); let mut l = Layout::new(1); l.split(A, 10, 2); l.focus = 1; l.close(2); assert_eq!(l.focus, 1); }
    #[test] fn focus_by_direction() {
        let mut l = Layout::new(1); l.split(A, 10, 2); l.split(A, 10, 3); l.split(A, 10, 4);   // 1 | 2 | (3 over 4)
        l.focus = 1; l.move_focus(A, 10, Dir::Right); assert_eq!(l.focus, 2);
        l.move_focus(A, 10, Dir::Right); assert_eq!(l.focus, 3);
        l.move_focus(A, 10, Dir::Down); assert_eq!(l.focus, 4);
        l.move_focus(A, 10, Dir::Down); assert_eq!(l.focus, 4);
        l.move_focus(A, 10, Dir::Up); assert_eq!(l.focus, 3);
        l.move_focus(A, 10, Dir::Left); assert_eq!(l.focus, 2);
        l.move_focus(A, 10, Dir::Left); l.move_focus(A, 10, Dir::Left); assert_eq!(l.focus, 1);
    }
    #[test] fn fullscreen_toggle() {
        let mut l = Layout::new(1); l.split(A, 10, 2); l.toggle_full();
        assert_eq!(l.rects(A, 10), vec![(2, A)]); l.toggle_full(); assert_eq!(l.rects(A, 10).len(), 2);
    }
}
