//! Basic geometry types shared by the engine and the UI.
//!
//! All coordinates are **physical pixels**, relative to the virtual screen
//! origin (the primary monitor's top-left is not necessarily `(0, 0)`).

use serde::{Deserialize, Serialize};

/// A half-open rectangle: `[left, right) × [top, bottom)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// The monitor rectangle is half-open, matching `MonitorFromPoint`.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// One physical display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// Full bounds in physical pixels.
    pub rect: Rect,
    /// Work area (excludes the taskbar).
    pub work: Rect,
    pub primary: bool,
    /// Device name such as `\\.\DISPLAY1`; only for diagnostics.
    pub device: String,
}

/// Which vertical edge of a monitor we are looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// The last pixel column that belongs to the monitor.
    pub fn edge_x(self, m: &Monitor) -> i32 {
        match self {
            Side::Left => m.rect.left,
            Side::Right => m.rect.right - 1,
        }
    }

    /// The first column *outside* the monitor. This is what the probe tests.
    pub fn probe_x(self, m: &Monitor) -> i32 {
        match self {
            Side::Left => m.rect.left - 1,
            Side::Right => m.rect.right,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Side::Left => "左",
            Side::Right => "右",
        }
    }

    pub fn all() -> [Side; 2] {
        [Side::Left, Side::Right]
    }
}

/// Where the trigger band sits on the monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BandPosition {
    Bottom,
    Top,
    /// Both bands: with both sides this is the four corners of a monitor.
    Both,
}

/// User-facing trigger configuration (the subset the geometry needs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriggerConfig {
    /// Band height as a fraction of the monitor height, e.g. `0.10`.
    pub ratio: f64,
    pub position: BandPosition,
    pub enable_left: bool,
    pub enable_right: bool,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            ratio: 0.10,
            position: BandPosition::Bottom,
            enable_left: true,
            enable_right: true,
        }
    }
}

impl TriggerConfig {
    pub fn side_enabled(&self, side: Side) -> bool {
        match side {
            Side::Left => self.enable_left,
            Side::Right => self.enable_right,
        }
    }
}
