//! Xorg's per-window clip regions, computed from the window tree on demand:
//! the clip list (what of a window shows, its children out) and the
//! universe its children are cut from (`NotClippedByChildren`), as
//! `miComputeClips` keeps them (`mi/mivaltree.c:194-470`). Graphics
//! exposures (`miHandleExposures`) and the Expose events of a map or an
//! unmap (`miHandleValidateExposures`) are made of them.
//!
//! Regions are in a window's content space and in pixman's canonical form
//! ([`canonical`]), so the rects a client is sent match Xorg's one for one.

use yserver_protocol::x11::{ResourceId, xfixes::RegionRect};

use crate::{
    resources::{MapState, ROOT_WINDOW, WindowClass},
    server::{CompositeRedirectMode, ServerState},
};

/// Xorg's `RECTLIMIT` (`mi/miexpose.c:107`): past this many rects an
/// exposure is sent as its extents.
pub(crate) const RECTLIMIT: usize = 25;

/// pixman's canonical form of the union of `rects`: y-x banded, each band
/// split only where the set of x spans changes, adjacent bands with the
/// same spans merged. Unique for a region, so two servers that agree on
/// the region agree on the rects.
pub(crate) fn canonical(rects: Vec<RegionRect>) -> Vec<RegionRect> {
    let banded = crate::nested::normalize_region_rects(rects);
    let mut out: Vec<RegionRect> = Vec::with_capacity(banded.len());
    // The previous band: its index range in `out`.
    let mut prev: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < banded.len() {
        let y = banded[i].y;
        let mut j = i;
        while j < banded.len() && banded[j].y == y {
            j += 1;
        }
        let band = &banded[i..j];
        let merged = prev.is_some_and(|(s, e)| {
            let p = &out[s..e];
            i32::from(p[0].y) + i32::from(p[0].height) == i32::from(y)
                && p.len() == band.len()
                && p.iter()
                    .zip(band)
                    .all(|(a, b)| a.x == b.x && a.width == b.width)
        });
        if merged {
            let (s, e) = prev.expect("merged implies a previous band");
            for r in &mut out[s..e] {
                r.height = r.height.saturating_add(band[0].height);
            }
        } else {
            let s = out.len();
            out.extend_from_slice(band);
            prev = Some((s, out.len()));
        }
        i = j;
    }
    out
}

/// `a ∩ b`, canonical.
pub(crate) fn intersect(a: &[RegionRect], b: &[RegionRect]) -> Vec<RegionRect> {
    canonical(crate::nested::intersect_regions(a, b))
}

/// `a − b`, canonical.
pub(crate) fn subtract(a: &[RegionRect], b: &[RegionRect]) -> Vec<RegionRect> {
    canonical(crate::nested::subtract_regions(a, b))
}

/// `rects` moved by `(dx, dy)`, saturating at the wire's range.
pub(crate) fn translate(mut rects: Vec<RegionRect>, dx: i32, dy: i32) -> Vec<RegionRect> {
    for r in &mut rects {
        r.x = (i32::from(r.x) + dx).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        r.y = (i32::from(r.y) + dy).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    }
    rects
}

/// Whether `rect` lies wholly inside `region` (`RegionContainsRect == rgnIN`).
pub(crate) fn contains(region: &[RegionRect], rect: RegionRect) -> bool {
    rect.width == 0 || rect.height == 0 || subtract(&[rect], region).is_empty()
}

/// A viewable `InputOutput` window: what takes part in clipping. An
/// `InputOnly` window is never viewable (`RealizeTree`, `dix/window.c`).
fn viewable(state: &ServerState, w: ResourceId) -> bool {
    w == ROOT_WINDOW
        || state.resources.window(w).is_some_and(|win| {
            win.map_state == MapState::Viewable && win.class == WindowClass::InputOutput
        })
}

/// A Manual-redirected window clips neither its siblings nor its parent
/// (`TreatAsTransparent`, `mi/mivaltree.c:171`).
fn transparent(state: &ServerState, w: ResourceId) -> bool {
    state.composite_redirects.window_mode(w) == Some(CompositeRedirectMode::Manual)
}

/// Xorg's `borderSize` of `w` in its parent's content space: its outer
/// rect, cut to its bounding shape (`SetBorderSize`, `dix/window.c:1747`).
pub(crate) fn border_size_in_parent(state: &ServerState, w: ResourceId) -> Vec<RegionRect> {
    let Some(win) = state.resources.window(w) else {
        return Vec::new();
    };
    let bw2 = win.border_width.saturating_mul(2);
    let outer = RegionRect {
        x: win.x,
        y: win.y,
        width: win.width.saturating_add(bw2),
        height: win.height.saturating_add(bw2),
    };
    match state.shape_windows.get(&w).and_then(|s| s.bounding.clone()) {
        Some(shape) => {
            let bw = i32::from(win.border_width);
            let shape = translate(shape, i32::from(win.x) + bw, i32::from(win.y) + bw);
            intersect(&[outer], &shape)
        }
        None => vec![outer],
    }
}

/// Xorg's `winSize` of `w` in its own content space, short of its
/// parent's: its content rect cut to its bounding and clip shapes
/// (`SetWinSize`, `dix/window.c:1710`).
fn win_size(state: &ServerState, w: ResourceId) -> Vec<RegionRect> {
    let Some(win) = state.resources.window(w) else {
        return Vec::new();
    };
    let mut region = vec![RegionRect {
        x: 0,
        y: 0,
        width: win.width,
        height: win.height,
    }];
    if let Some(shape) = state.shape_windows.get(&w) {
        for s in [&shape.bounding, &shape.clip].into_iter().flatten() {
            region = intersect(&region, s);
        }
    }
    region
}

/// What of `w` shows, its inferiors included, in its content space:
/// Xorg's `borderClip ∩ winSize`, which `NotClippedByChildren` returns and
/// its children's universes are cut from. A redirected window's is its own
/// shape, not clipped by its parent or the screen (`mi/mivaltree.c:233-
/// 239`); every other window's is its parent's less the windows stacked
/// above it there.
pub(crate) fn not_clipped_by_children(state: &ServerState, w: ResourceId) -> Vec<RegionRect> {
    if !viewable(state, w) {
        return Vec::new();
    }
    if w == ROOT_WINDOW {
        return state
            .resources
            .window(ROOT_WINDOW)
            .map_or_else(Vec::new, |r| {
                vec![RegionRect {
                    x: 0,
                    y: 0,
                    width: r.width,
                    height: r.height,
                }]
            });
    }
    if state.composite_redirects.window_mode(w).is_some() {
        return win_size(state, w);
    }
    let Some(parent) = state.resources.window(w).map(|win| win.parent) else {
        return Vec::new();
    };
    let universe = not_clipped_by_children(state, parent);
    child_universe(state, &universe, parent, w)
}

/// [`not_clipped_by_children`] of `child` from its parent's.
fn child_universe(
    state: &ServerState,
    parent_universe: &[RegionRect],
    parent: ResourceId,
    child: ResourceId,
) -> Vec<RegionRect> {
    if !viewable(state, child) {
        return Vec::new();
    }
    if state.composite_redirects.window_mode(child).is_some() {
        return win_size(state, child);
    }
    if parent_universe.is_empty() {
        return Vec::new();
    }
    let Some((x, y, bw)) = state
        .resources
        .window(child)
        .map(|c| (i32::from(c.x), i32::from(c.y), i32::from(c.border_width)))
    else {
        return Vec::new();
    };
    let mut universe = parent_universe.to_vec();
    let siblings = state.resources.children(parent);
    if let Some(at) = siblings.iter().position(|s| *s == child) {
        for s in &siblings[at + 1..] {
            if viewable(state, *s) && !transparent(state, *s) {
                universe = subtract(&universe, &border_size_in_parent(state, *s));
            }
        }
    }
    let universe = translate(universe, -(x + bw), -(y + bw));
    intersect(&universe, &win_size(state, child))
}

/// `w`'s clip list in its content space: what of it shows and is not under
/// a child (ClipByChildren).
pub(crate) fn clip_list(state: &ServerState, w: ResourceId) -> Vec<RegionRect> {
    clip_list_from(state, w, not_clipped_by_children(state, w))
}

fn clip_list_from(
    state: &ServerState,
    w: ResourceId,
    universe: Vec<RegionRect>,
) -> Vec<RegionRect> {
    let mut region = universe;
    for c in state.resources.children(w) {
        if region.is_empty() {
            break;
        }
        if viewable(state, *c) && !transparent(state, *c) {
            region = subtract(&region, &border_size_in_parent(state, *c));
        }
    }
    region
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i16, y: i16, width: u16, height: u16) -> RegionRect {
        RegionRect {
            x,
            y,
            width,
            height,
        }
    }

    /// pixman merges bands whose spans agree: the union of two stacked
    /// rects with the same x range is one rect, and a rect minus an inner
    /// one is four, top band, two sides, bottom band.
    #[test]
    fn canonical_coalesces_bands_like_pixman() {
        assert_eq!(
            canonical(vec![r(0, 0, 10, 5), r(0, 5, 10, 5)]),
            vec![r(0, 0, 10, 10)]
        );
        assert_eq!(
            subtract(&[r(0, 0, 10, 10)], &[r(3, 3, 4, 4)]),
            vec![r(0, 0, 10, 3), r(0, 3, 3, 4), r(7, 3, 3, 4), r(0, 7, 10, 3)]
        );
        // UnmapSubwindows in draw-clip's expose-probe: P1 ∪ P5 seen on P.
        assert_eq!(
            canonical(vec![r(40, 10, 70, 50), r(40, 40, 40, 60)]),
            vec![r(40, 10, 70, 50), r(40, 60, 40, 40)]
        );
    }
}
