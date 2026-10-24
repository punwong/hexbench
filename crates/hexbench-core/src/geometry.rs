//! Probe-based edge segmentation.
//!
//! Instead of special-casing `min(left)` / `max(right)`, we ask the system for
//! every pixel row whether the column just outside the edge is empty. The
//! result is a set of *exposed* vertical segments per edge, which is a strict
//! generalisation of the min/max rule and also covers L-shapes and stacked
//! layouts.
//!
//! The probe itself is abstracted behind [`EdgeProbe`], so the geometry can be
//! unit-tested on any host, including layouts that do not exist on this PC.

use crate::model::{BandPosition, Monitor, Side, TriggerConfig};

/// A contiguous vertical run of one monitor edge that the user can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// Index into the monitor list.
    pub monitor: usize,
    pub side: Side,
    /// The pixel column the cursor has to be on (`m.left` or `m.right - 1`).
    pub edge_x: i32,
    /// Half-open vertical range `[y0, y1)`.
    pub y0: i32,
    pub y1: i32,
}

impl Region {
    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }
}

/// Answers "is this point on some monitor?".
pub trait EdgeProbe {
    fn covered(&self, x: i32, y: i32) -> bool;
}

/// Probe backed by an in-memory monitor list (tests, synthetic layouts).
pub struct SliceProbe<'a>(pub &'a [Monitor]);

impl EdgeProbe for SliceProbe<'_> {
    fn covered(&self, x: i32, y: i32) -> bool {
        self.0.iter().any(|m| m.rect.contains(x, y))
    }
}

/// Stable order: primary first (so `屏 0` is always the main display), then
/// left-to-right, then top-to-bottom. The index is what the UI and the config
/// refer to, so it must not depend on enumeration order.
pub fn sort_monitors(monitors: &mut [Monitor]) {
    monitors.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top, m.rect.right, m.rect.bottom));
}

/// The trigger bands of a monitor, as half-open `[y0, y1)` ranges.
///
/// `Both` yields two ranges (top and bottom); if a large ratio makes them meet
/// they collapse into one, so a region is never produced twice.
pub fn bands(m: &Monitor, cfg: &TriggerConfig) -> Vec<(i32, i32)> {
    let height = m.rect.height().max(0);
    let band_height = ((f64::from(height) * cfg.ratio).round() as i32).clamp(0, height);
    let top = (m.rect.top, m.rect.top + band_height);
    let bottom = (m.rect.bottom - band_height, m.rect.bottom);
    match cfg.position {
        BandPosition::Bottom => vec![bottom],
        BandPosition::Top => vec![top],
        BandPosition::Both => {
            if top.1 >= bottom.0 {
                vec![(m.rect.top, m.rect.bottom)]
            } else {
                vec![top, bottom]
            }
        }
    }
}

/// Split one edge into maximal segments whose outside column is empty, cutting
/// at every other monitor's top/bottom so boundaries land on real geometry.
pub fn exposed_segments(
    probe: &dyn EdgeProbe,
    monitors: &[Monitor],
    index: usize,
    side: Side,
) -> Vec<(i32, i32)> {
    let m = &monitors[index];
    let px = side.probe_x(m);
    let mut cuts = vec![m.rect.top, m.rect.bottom];
    for other in monitors {
        for y in [other.rect.top, other.rect.bottom] {
            if y > m.rect.top && y < m.rect.bottom {
                cuts.push(y);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut segments: Vec<(i32, i32)> = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b <= a || probe.covered(px, a) {
            continue;
        }
        match segments.last_mut() {
            Some(last) if last.1 == a => last.1 = b,
            _ => segments.push((a, b)),
        }
    }
    segments
}

/// Every triggerable region, sorted by monitor then side.
pub fn regions(monitors: &[Monitor], cfg: &TriggerConfig) -> Vec<Region> {
    let probe = SliceProbe(monitors);
    let mut out = Vec::new();
    for (index, m) in monitors.iter().enumerate() {
        let bands = bands(m, cfg);
        for side in Side::all() {
            if !cfg.side_enabled(side) {
                continue;
            }
            for (a, b) in exposed_segments(&probe, monitors, index, side) {
                for (band_y0, band_y1) in &bands {
                    let y0 = a.max(*band_y0);
                    let y1 = b.min(*band_y1);
                    if y1 > y0 {
                        out.push(Region { monitor: index, side, edge_x: side.edge_x(m), y0, y1 });
                    }
                }
            }
        }
    }
    out
}

/// The region containing a cursor position, if any.
pub fn region_at(regions: &[Region], x: i32, y: i32) -> Option<&Region> {
    regions.iter().find(|r| r.edge_x == x && y >= r.y0 && y < r.y1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    fn monitor(l: i32, t: i32, r: i32, b: i32) -> Monitor {
        Monitor {
            rect: Rect::new(l, t, r, b),
            work: Rect::new(l, t, r, b),
            primary: false,
            device: String::new(),
        }
    }

    /// The layout from the plan's section 3.
    fn dev_layout() -> Vec<Monitor> {
        let mut v = vec![
            monitor(0, 0, 3440, 1440),
            monitor(-1080, -232, 0, 1688),
        ];
        v[0].primary = true;
        sort_monitors(&mut v);
        v
    }

    #[test]
    fn dev_layout_has_three_regions() {
        let monitors = dev_layout();
        let cfg = TriggerConfig::default();
        let regions = regions(&monitors, &cfg);

        assert_eq!(regions.len(), 3, "{regions:#?}");

        // Screen 0 right edge, full 10% band.
        let r = regions.iter().find(|r| r.monitor == 0 && r.side == Side::Right).unwrap();
        assert_eq!(r.edge_x, 3439);
        assert_eq!((r.y0, r.y1), (1296, 1440));

        // Screen 1 left edge, full 10% band.
        let l = regions.iter().find(|r| r.monitor == 1 && r.side == Side::Left).unwrap();
        assert_eq!(l.edge_x, -1080);
        assert_eq!((l.y0, l.y1), (1496, 1688));

        // Screen 1 right edge is a real edge only below screen 0.
        let rr = regions.iter().find(|r| r.monitor == 1 && r.side == Side::Right).unwrap();
        assert_eq!(rr.edge_x, -1);
        assert_eq!((rr.y0, rr.y1), (1496, 1688));
    }

    #[test]
    fn seam_is_not_triggerable() {
        let monitors = dev_layout();
        let regions = regions(&monitors, &TriggerConfig::default());
        // x = 0 on screen 0's left edge is covered by the portrait monitor.
        assert!(regions.iter().all(|r| !(r.monitor == 0 && r.side == Side::Left)));
        // x = -1 above screen 0 is covered as well.
        assert!(regions.iter().all(|r| !(r.monitor == 1 && r.side == Side::Right && r.y0 < 1440)));
    }

    #[test]
    fn single_monitor_exposes_both_edges() {
        let mut v = vec![monitor(0, 0, 1920, 1080)];
        sort_monitors(&mut v);
        let regions = regions(&v, &TriggerConfig::default());
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.height() == 108));
    }

    #[test]
    fn stacked_monitors_keep_left_edges_exposed() {
        let mut v = vec![monitor(0, 0, 1920, 1080), monitor(0, 1080, 1920, 2160)];
        sort_monitors(&mut v);
        let regions = regions(&v, &TriggerConfig::default());
        let lefts: Vec<_> = regions.iter().filter(|r| r.side == Side::Left).collect();
        assert_eq!(lefts.len(), 2);
        // The lower monitor's band is at the very bottom, the upper one's band
        // sits at its own bottom edge (the seam) but the left side is still a
        // real outer edge there.
        assert!(lefts.iter().any(|r| r.monitor == 0 && r.y0 == 972));
        assert!(lefts.iter().any(|r| r.monitor == 1 && r.y0 == 2052));
    }

    #[test]
    fn top_position_moves_the_band() {
        let mut v = vec![monitor(0, 0, 1920, 1080)];
        sort_monitors(&mut v);
        let cfg = TriggerConfig { position: BandPosition::Top, ..Default::default() };
        let regions = regions(&v, &cfg);
        assert!(regions.iter().all(|r| (r.y0, r.y1) == (0, 108)));
    }

    #[test]
    fn both_bands_give_four_corners() {
        let mut v = vec![monitor(0, 0, 1920, 1080)];
        sort_monitors(&mut v);
        let cfg = TriggerConfig { position: BandPosition::Both, ..Default::default() };
        let regions = regions(&v, &cfg);

        assert_eq!(regions.len(), 4, "{regions:#?}");
        for side in [Side::Left, Side::Right] {
            assert!(regions.iter().any(|r| r.side == side && (r.y0, r.y1) == (0, 108)));
            assert!(regions.iter().any(|r| r.side == side && (r.y0, r.y1) == (972, 1080)));
        }
    }

    #[test]
    fn both_bands_merge_when_ratio_is_large() {
        let mut v = vec![monitor(0, 0, 1000, 1000)];
        sort_monitors(&mut v);
        let cfg = TriggerConfig {
            position: BandPosition::Both,
            ratio: 0.6,
            ..Default::default()
        };
        let regions = regions(&v, &cfg);

        assert_eq!(regions.len(), 2, "{regions:#?}");
        assert!(regions.iter().all(|r| (r.y0, r.y1) == (0, 1000)));
    }

    #[test]
    fn region_at_matches_the_pixel() {
        let monitors = dev_layout();
        let regions = regions(&monitors, &TriggerConfig::default());
        assert!(region_at(&regions, 3439, 1300).is_some());
        assert!(region_at(&regions, 3438, 1300).is_none());
        assert!(region_at(&regions, 3439, 1200).is_none());
    }
}
