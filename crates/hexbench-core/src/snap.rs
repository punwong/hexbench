//! 桌面格子移动 / 缩放时的吸附（纯几何，方便单测）。
//!
//! 规则照成熟实现（PecoFence）：阈值内取「拉动最小」的候选，X / Y 各自独立；
//! 移动时对齐工作区边缘、工作区中线、其他格子的对应边；缩放时只吸附正在拖的边。
//! 所有数值都是**物理像素**，调用方负责把 DIP 阈值按窗口所在显示器的 DPI 换算。

use crate::model::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl Edge {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "n" => Self::North,
            "s" => Self::South,
            "e" => Self::East,
            "w" => Self::West,
            "ne" => Self::NorthEast,
            "nw" => Self::NorthWest,
            "se" => Self::SouthEast,
            "sw" => Self::SouthWest,
            _ => return None,
        })
    }

    pub fn moves_west(self) -> bool {
        matches!(self, Self::West | Self::NorthWest | Self::SouthWest)
    }

    pub fn moves_east(self) -> bool {
        matches!(self, Self::East | Self::NorthEast | Self::SouthEast)
    }

    pub fn moves_north(self) -> bool {
        matches!(self, Self::North | Self::NorthWest | Self::NorthEast)
    }

    pub fn moves_south(self) -> bool {
        matches!(self, Self::South | Self::SouthWest | Self::SouthEast)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapConfig {
    pub enabled: bool,
    /// 吸附阈值（物理像素）。
    pub distance: i32,
    /// 网格吸附步长；0 表示不吸附网格。
    pub grid: i32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self { enabled: true, distance: 8, grid: 0 }
    }
}

/// 一条对齐参考线：`vertical` 时画在 x = `at`，从 `from` 到 `to`（y 坐标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guide {
    pub vertical: bool,
    pub at: i32,
    pub from: i32,
    pub to: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapResult {
    pub rect: Rect,
    pub guides: [Option<Guide>; 2],
}

impl SnapResult {
    fn plain(rect: Rect) -> Self {
        Self { rect, guides: [None, None] }
    }
}

fn round_to(value: i32, grid: i32) -> i32 {
    if grid <= 1 {
        return value;
    }
    let half = grid / 2;
    (value + half).div_euclid(grid) * grid
}

/// 找一个轴上「拉动最小」的吸附。`candidates` 是目标边坐标（带上产生它的那个
/// 兄弟格子，工作区自己的边没有）；`edge_offsets` 是被拖边相对矩形原点（左 / 上）
/// 的偏移，例如右边是 `width`，左边是 0。
/// 返回：新的原点坐标、命中的目标坐标、命中的是谁。
fn best_offset(
    origin: i32,
    edge_offsets: &[i32],
    candidates: &[(i32, Option<Rect>)],
    distance: i32,
) -> Option<(i32, i32, Option<Rect>)> {
    let mut best: Option<(i32, i32, Option<Rect>)> = None;
    for &(target, source) in candidates {
        for &offset in edge_offsets {
            let pull = target - (origin + offset);
            if pull.abs() > distance {
                continue;
            }
            let better = match best {
                Some((_, best_pull, _)) => pull.abs() < best_pull.abs(),
                None => true,
            };
            if better {
                best = Some((target - offset, target, source));
            }
        }
    }
    best
}

/// 参考线两端要比被拖格子多探出去一点，看起来才像「连到那块格子」而不是顶着边。
const GUIDE_PAD: i32 = 10;

/// 参考线的长度。贴着兄弟格子时画两块格子的并集；贴工作区边、吸中线时画满屏幕。
fn guide_span(vertical: bool, rect: Rect, work: Rect, source: Option<Rect>) -> (i32, i32) {
    match (source, vertical) {
        (Some(other), true) => (
            rect.top.min(other.top) - GUIDE_PAD,
            rect.bottom.max(other.bottom) + GUIDE_PAD,
        ),
        (Some(other), false) => (
            rect.left.min(other.left) - GUIDE_PAD,
            rect.right.max(other.right) + GUIDE_PAD,
        ),
        (None, true) => (work.top, work.bottom),
        (None, false) => (work.left, work.right),
    }
}

/// 移动吸附：返回吸附后的左上角。
pub fn snap_move(rect: Rect, work: Rect, others: &[Rect], cfg: SnapConfig) -> SnapResult {
    let mut result = SnapResult::plain(rect);
    if !cfg.enabled {
        if cfg.grid > 1 {
            result.rect.left = round_to(rect.left, cfg.grid);
            result.rect.top = round_to(rect.top, cfg.grid);
            result.rect.right = result.rect.left + rect.width();
            result.rect.bottom = result.rect.top + rect.height();
        }
        return result;
    }

    let mut xs = vec![(work.left, None), (work.right, None)];
    let mut ys = vec![(work.top, None), (work.bottom, None)];
    for other in others {
        xs.push((other.left, Some(*other)));
        xs.push((other.right, Some(*other)));
        ys.push((other.top, Some(*other)));
        ys.push((other.bottom, Some(*other)));
    }

    let width = rect.width();
    let height = rect.height();
    // 先试边缘 / 兄弟格子，没有再试工作区居中（居中的参考线要画在中线上）。
    let mut snapped_x = best_offset(rect.left, &[0, width], &xs, cfg.distance);
    let mut snapped_y = best_offset(rect.top, &[0, height], &ys, cfg.distance);
    if snapped_x.is_none() {
        let centered = work.left + (work.width() - width) / 2;
        if (rect.left - centered).abs() <= cfg.distance {
            snapped_x = Some((centered, work.left + work.width() / 2, None));
        }
    }
    if snapped_y.is_none() {
        let centered = work.top + (work.height() - height) / 2;
        if (rect.top - centered).abs() <= cfg.distance {
            snapped_y = Some((centered, work.top + work.height() / 2, None));
        }
    }
    match snapped_x {
        Some((left, at, source)) => {
            result.rect.left = left;
            result.rect.right = left + width;
            let (from, to) = guide_span(true, result.rect, work, source);
            result.guides[0] = Some(Guide { vertical: true, at, from, to });
        }
        None if cfg.grid > 1 => {
            result.rect.left = round_to(rect.left, cfg.grid);
            result.rect.right = result.rect.left + width;
        }
        None => {}
    }
    match snapped_y {
        Some((top, at, source)) => {
            result.rect.top = top;
            result.rect.bottom = top + height;
            let (from, to) = guide_span(false, result.rect, work, source);
            result.guides[1] = Some(Guide { vertical: false, at, from, to });
        }
        None if cfg.grid > 1 => {
            result.rect.top = round_to(rect.top, cfg.grid);
            result.rect.bottom = result.rect.top + height;
        }
        None => {}
    }
    result
}

/// 缩放吸附：只动 `edge` 指定的边，返回吸附 / 夹到最小尺寸之后的矩形。
pub fn snap_resize(
    rect: Rect,
    edge: Edge,
    work: Rect,
    others: &[Rect],
    cfg: SnapConfig,
    min_width: i32,
    min_height: i32,
) -> SnapResult {
    let mut result = SnapResult::plain(rect);
    let mut xs = vec![(work.left, None), (work.right, None)];
    let mut ys = vec![(work.top, None), (work.bottom, None)];
    for other in others {
        xs.push((other.left, Some(*other)));
        xs.push((other.right, Some(*other)));
        ys.push((other.top, Some(*other)));
        ys.push((other.bottom, Some(*other)));
    }

    if edge.moves_east() {
        let snapped = if cfg.enabled {
            best_offset(rect.right, &[0], &xs, cfg.distance)
                .filter(|(right, _, _)| right - rect.left >= min_width)
        } else {
            None
        };
        if let Some((right, at, source)) = snapped {
            result.rect.right = right;
            let (from, to) = guide_span(true, result.rect, work, source);
            result.guides[0] = Some(Guide { vertical: true, at, from, to });
        } else if cfg.grid > 1 {
            result.rect.right = round_to(rect.right, cfg.grid);
        }
        result.rect.right = result.rect.right.max(result.rect.left + min_width);
    }
    if edge.moves_west() {
        let snapped = if cfg.enabled {
            best_offset(rect.left, &[0], &xs, cfg.distance)
                .filter(|(left, _, _)| rect.right - left >= min_width)
        } else {
            None
        };
        if let Some((left, at, source)) = snapped {
            result.rect.left = left;
            let (from, to) = guide_span(true, result.rect, work, source);
            result.guides[0] = Some(Guide { vertical: true, at, from, to });
        } else if cfg.grid > 1 {
            result.rect.left = round_to(rect.left, cfg.grid);
        }
        result.rect.left = result.rect.left.min(result.rect.right - min_width);
    }
    if edge.moves_south() {
        let snapped = if cfg.enabled {
            best_offset(rect.bottom, &[0], &ys, cfg.distance)
                .filter(|(bottom, _, _)| bottom - rect.top >= min_height)
        } else {
            None
        };
        if let Some((bottom, at, source)) = snapped {
            result.rect.bottom = bottom;
            let (from, to) = guide_span(false, result.rect, work, source);
            result.guides[1] = Some(Guide { vertical: false, at, from, to });
        } else if cfg.grid > 1 {
            result.rect.bottom = round_to(rect.bottom, cfg.grid);
        }
        result.rect.bottom = result.rect.bottom.max(result.rect.top + min_height);
    }
    if edge.moves_north() {
        let snapped = if cfg.enabled {
            best_offset(rect.top, &[0], &ys, cfg.distance)
                .filter(|(top, _, _)| rect.bottom - top >= min_height)
        } else {
            None
        };
        if let Some((top, at, source)) = snapped {
            result.rect.top = top;
            let (from, to) = guide_span(false, result.rect, work, source);
            result.guides[1] = Some(Guide { vertical: false, at, from, to });
        } else if cfg.grid > 1 {
            result.rect.top = round_to(rect.top, cfg.grid);
        }
        result.rect.top = result.rect.top.min(result.rect.bottom - min_height);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    fn cfg() -> SnapConfig {
        SnapConfig { enabled: true, distance: 8, grid: 0 }
    }

    #[test]
    fn move_snaps_to_work_area_edge() {
        let rect = Rect::new(6, 300, 306, 500);
        let result = snap_move(rect, work(), &[], cfg());
        assert_eq!(result.rect.left, 0);
        assert!(result.guides[0].is_some());
        assert_eq!(result.rect.width(), 300);
    }

    #[test]
    fn move_keeps_free_position_beyond_threshold() {
        let rect = Rect::new(30, 300, 330, 500);
        let result = snap_move(rect, work(), &[], cfg());
        assert_eq!(result.rect, rect);
        assert!(result.guides.iter().all(Option::is_none));
    }

    #[test]
    fn move_aligns_to_sibling_and_center() {
        let sibling = Rect::new(500, 200, 800, 500);
        // 右边贴到兄弟左边
        let rect = Rect::new(194, 100, 494, 300);
        let result = snap_move(rect, work(), &[sibling], cfg());
        assert_eq!(result.rect.right, 500);
        // 水平居中
        let centered = Rect::new(812, 100, 1112, 300);
        let result = snap_move(centered, work(), &[], cfg());
        assert_eq!(result.rect.left, 810);
    }

    #[test]
    fn move_grid_rounds_when_no_edge_matches() {
        let rect = Rect::new(103, 207, 403, 407);
        let result = snap_move(rect, work(), &[], SnapConfig { enabled: true, distance: 4, grid: 16 });
        assert_eq!(result.rect.left, 96);
        assert_eq!(result.rect.top, 208);
    }

    #[test]
    fn resize_snaps_dragged_edge_and_respects_minimum() {
        let sibling = Rect::new(500, 200, 800, 500);
        let rect = Rect::new(200, 100, 497, 300);
        let result = snap_resize(rect, Edge::East, work(), &[sibling], cfg(), 240, 160);
        assert_eq!(result.rect.right, 500);

        // 越过了最小宽度：不吸过去，夹在最小宽度上
        let tight = Rect::new(200, 100, 430, 300);
        let result = snap_resize(tight, Edge::East, work(), &[sibling], cfg(), 240, 160);
        assert_eq!(result.rect.width(), 240);
    }

    #[test]
    fn resize_west_snaps_to_work_area() {
        let rect = Rect::new(5, 100, 405, 300);
        let result = snap_resize(rect, Edge::West, work(), &[], cfg(), 240, 160);
        assert_eq!(result.rect.left, 0);
        assert!(result.guides[0].is_some());
    }

    #[test]
    fn guide_spans_between_aligned_siblings() {
        let sibling = Rect::new(500, 200, 800, 500);
        let rect = Rect::new(196, 300, 496, 600);
        let result = snap_move(rect, work(), &[sibling], cfg());
        let guide = result.guides[0].expect("竖线");
        assert_eq!(guide.at, 500);
        // 两块格子的并集 200..600，两头各留 10px。
        assert_eq!((guide.from, guide.to), (190, 610));
    }

    #[test]
    fn guide_spans_whole_work_area_for_edges() {
        let rect = Rect::new(6, 300, 306, 500);
        let result = snap_move(rect, work(), &[], cfg());
        let guide = result.guides[0].expect("竖线");
        assert_eq!(guide.at, 0);
        assert_eq!((guide.from, guide.to), (0, 1080));
    }

    #[test]
    fn negative_coordinates_work() {
        let second = Rect::new(-1920, 0, 0, 1080);
        let rect = Rect::new(-1914, 200, -1614, 400);
        let result = snap_move(rect, second, &[], cfg());
        assert_eq!(result.rect.left, -1920);
    }
}
