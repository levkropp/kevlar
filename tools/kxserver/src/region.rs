// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Region data structure used by the XFIXES extension (Phase 11) and
// the RENDER clip-region path.
//
// A `Region` is a set of screen-local rectangles.  The rectangles do
// NOT need to be non-overlapping — `contains(x, y)` is an OR of per-
// rect contains, so overlap is harmless for the clip-check fast path.
// Subtract/invert go through a classical 4-way rectangle split that
// DOES produce a non-overlapping result, because real region
// semantics require it for the Fetch/Union round-trip to be stable.
//
// For Phase 11 the algorithms are deliberately naïve (O(n*m)) — the
// typical region from Xft or xterm is less than a dozen rectangles,
// and Cairo's biggest regions tend to be bounded damage lists that
// rarely grow past ~50 rects.  We'll optimize if a profiler ever
// points here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn new(x: i16, y: i16, w: u16, h: u16) -> Self { Rect { x, y, w, h } }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32 && px < (self.x as i32 + self.w as i32)
            && py >= self.y as i32 && py < (self.y as i32 + self.h as i32)
    }

    pub fn is_empty(&self) -> bool { self.w == 0 || self.h == 0 }

    /// Compute the intersection of two rectangles.  Returns `None`
    /// when they don't overlap.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = ((self.x as i32) + (self.w as i32)).min((other.x as i32) + (other.w as i32));
        let y1 = ((self.y as i32) + (self.h as i32)).min((other.y as i32) + (other.h as i32));
        if x1 <= x0 as i32 || y1 <= y0 as i32 { return None; }
        Some(Rect {
            x: x0,
            y: y0,
            w: (x1 - x0 as i32) as u16,
            h: (y1 - y0 as i32) as u16,
        })
    }

    /// Subtract `hole` from `self`, returning up to 4 fragment
    /// rectangles that together cover `self \ hole`.  Fragments with
    /// width or height 0 are dropped.  The split order produces
    /// non-overlapping output.
    pub fn subtract(&self, hole: &Rect) -> Vec<Rect> {
        let inter = match self.intersect(hole) {
            Some(r) => r,
            None => return vec![*self],
        };
        let mut out = Vec::with_capacity(4);
        // Top strip: full width of self, above `inter`.
        if inter.y > self.y {
            out.push(Rect {
                x: self.x,
                y: self.y,
                w: self.w,
                h: (inter.y - self.y) as u16,
            });
        }
        // Bottom strip: full width, below `inter`.
        let self_bot = self.y as i32 + self.h as i32;
        let inter_bot = inter.y as i32 + inter.h as i32;
        if inter_bot < self_bot {
            out.push(Rect {
                x: self.x,
                y: inter_bot as i16,
                w: self.w,
                h: (self_bot - inter_bot) as u16,
            });
        }
        // Left strip: height of `inter`, left of it.
        if inter.x > self.x {
            out.push(Rect {
                x: self.x,
                y: inter.y,
                w: (inter.x - self.x) as u16,
                h: inter.h,
            });
        }
        // Right strip: height of `inter`, right of it.
        let self_right = self.x as i32 + self.w as i32;
        let inter_right = inter.x as i32 + inter.w as i32;
        if inter_right < self_right {
            out.push(Rect {
                x: inter_right as i16,
                y: inter.y,
                w: (self_right - inter_right) as u16,
                h: inter.h,
            });
        }
        out
    }
}

// ═════════════════════════════════════════════════════════════════════
// Region
// ═════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
pub struct Region {
    pub rects: Vec<Rect>,
}

impl Region {
    pub fn empty() -> Self { Region { rects: Vec::new() } }

    pub fn from_rect(r: Rect) -> Self {
        if r.is_empty() { Region::empty() } else { Region { rects: vec![r] } }
    }

    pub fn from_rects(rects: Vec<Rect>) -> Self {
        Region { rects: rects.into_iter().filter(|r| !r.is_empty()).collect() }
    }

    pub fn is_empty(&self) -> bool { self.rects.iter().all(|r| r.is_empty()) }

    /// Bounding box of all member rects.  Returns an empty rect for
    /// empty regions.
    pub fn extents(&self) -> Rect {
        if self.rects.is_empty() {
            return Rect::new(0, 0, 0, 0);
        }
        let mut x0 = i32::MAX;
        let mut y0 = i32::MAX;
        let mut x1 = i32::MIN;
        let mut y1 = i32::MIN;
        for r in &self.rects {
            if r.is_empty() { continue; }
            x0 = x0.min(r.x as i32);
            y0 = y0.min(r.y as i32);
            x1 = x1.max(r.x as i32 + r.w as i32);
            y1 = y1.max(r.y as i32 + r.h as i32);
        }
        if x1 <= x0 || y1 <= y0 {
            return Rect::new(0, 0, 0, 0);
        }
        Rect {
            x: x0 as i16,
            y: y0 as i16,
            w: (x1 - x0) as u16,
            h: (y1 - y0) as u16,
        }
    }

    /// True iff some member rectangle contains the point.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        self.rects.iter().any(|r| r.contains(px, py))
    }

    /// Shift every rectangle by (dx, dy).
    pub fn translate(&mut self, dx: i16, dy: i16) {
        for r in &mut self.rects {
            r.x = r.x.saturating_add(dx);
            r.y = r.y.saturating_add(dy);
        }
    }

    /// Union: concat the rect lists.  Does not enforce
    /// non-overlap — the `contains` path is OR-based so overlap is a
    /// no-op for clip checks.
    pub fn union(&self, other: &Region) -> Region {
        let mut r = self.rects.clone();
        r.extend_from_slice(&other.rects);
        Region { rects: r }
    }

    /// Intersection: pairwise intersect every rect in `self` with
    /// every rect in `other`, keep non-empty results.
    pub fn intersect(&self, other: &Region) -> Region {
        let mut out = Vec::new();
        for a in &self.rects {
            for b in &other.rects {
                if let Some(i) = a.intersect(b) {
                    out.push(i);
                }
            }
        }
        Region { rects: out }
    }

    /// Subtraction: for each rect `a` in `self`, subtract every
    /// rect in `other` via repeated 4-way splits.  Result is
    /// non-overlapping.
    pub fn subtract(&self, other: &Region) -> Region {
        let mut work: Vec<Rect> = self.rects.clone();
        for hole in &other.rects {
            let mut next = Vec::with_capacity(work.len());
            for a in work {
                next.extend(a.subtract(hole));
            }
            work = next;
        }
        Region { rects: work.into_iter().filter(|r| !r.is_empty()).collect() }
    }

    /// Invert against a bounding rectangle: `bounds \ self`.
    pub fn invert(&self, bounds: Rect) -> Region {
        Region::from_rect(bounds).subtract(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i16, y: i16, w: u16, h: u16) -> Rect { Rect::new(x, y, w, h) }

    #[test]
    fn rect_contains() {
        let a = r(0, 0, 10, 10);
        assert!(a.contains(0, 0));
        assert!(a.contains(9, 9));
        assert!(!a.contains(10, 5));
        assert!(!a.contains(5, 10));
        assert!(!a.contains(-1, 5));
    }

    #[test]
    fn rect_intersect_overlap() {
        let a = r(0, 0, 10, 10);
        let b = r(5, 5, 10, 10);
        let i = a.intersect(&b).unwrap();
        assert_eq!(i, r(5, 5, 5, 5));
    }

    #[test]
    fn rect_intersect_disjoint() {
        let a = r(0, 0, 10, 10);
        let b = r(20, 20, 10, 10);
        assert!(a.intersect(&b).is_none());
    }

    #[test]
    fn rect_subtract_interior_hole() {
        // A big rect with a hole in the middle → 4 fragments.
        let a = r(0, 0, 10, 10);
        let hole = r(3, 3, 4, 4);
        let frags = a.subtract(&hole);
        assert_eq!(frags.len(), 4);
        // Top strip: (0,0,10,3)
        assert!(frags.contains(&r(0, 0, 10, 3)));
        // Bottom strip: (0,7,10,3)
        assert!(frags.contains(&r(0, 7, 10, 3)));
        // Left strip: (0,3,3,4)
        assert!(frags.contains(&r(0, 3, 3, 4)));
        // Right strip: (7,3,3,4)
        assert!(frags.contains(&r(7, 3, 3, 4)));
    }

    #[test]
    fn rect_subtract_nonoverlapping() {
        let a = r(0, 0, 10, 10);
        let hole = r(20, 20, 5, 5);
        assert_eq!(a.subtract(&hole), vec![a]);
    }

    #[test]
    fn region_union_extents() {
        let a = Region::from_rect(r(0, 0, 10, 10));
        let b = Region::from_rect(r(20, 20, 10, 10));
        let u = a.union(&b);
        assert_eq!(u.rects.len(), 2);
        assert_eq!(u.extents(), r(0, 0, 30, 30));
    }

    #[test]
    fn region_intersect() {
        let a = Region::from_rects(vec![r(0, 0, 10, 10), r(20, 0, 10, 10)]);
        let b = Region::from_rect(r(5, 5, 30, 2));
        let i = a.intersect(&b);
        assert_eq!(i.rects.len(), 2);
        assert!(i.contains(7, 6));
        assert!(i.contains(25, 6));
        assert!(!i.contains(15, 6));
    }

    #[test]
    fn region_subtract_produces_nonoverlapping() {
        let a = Region::from_rect(r(0, 0, 10, 10));
        let b = Region::from_rect(r(3, 3, 4, 4));
        let d = a.subtract(&b);
        // Every point in d should NOT be in the hole, and every
        // point in original a but outside the hole SHOULD be in d.
        for y in 0..10 {
            for x in 0..10 {
                let in_hole = x >= 3 && x < 7 && y >= 3 && y < 7;
                assert_eq!(d.contains(x, y), !in_hole, "x={x} y={y}");
            }
        }
    }

    #[test]
    fn region_translate() {
        let mut a = Region::from_rect(r(10, 20, 5, 5));
        a.translate(3, 4);
        assert_eq!(a.rects[0], r(13, 24, 5, 5));
    }

    #[test]
    fn region_invert() {
        let a = Region::from_rect(r(3, 3, 4, 4));
        let bounds = r(0, 0, 10, 10);
        let inv = a.invert(bounds);
        assert!(inv.contains(0, 0));
        assert!(!inv.contains(5, 5));
    }
}
