//! HexBench core: monitor geometry, trigger logic and the headless engine.
//!
//! Deliberately free of Tauri and of any UI concern, so it can be unit-tested
//! and driven from a CLI (`hexbench run` / `hexbench --layout`).

pub mod config;
#[cfg(windows)]
pub mod dpapi;
pub mod geometry;
pub mod model;
pub mod sync;
pub mod trigger;

#[cfg(target_os = "windows")]
pub mod engine;
#[cfg(target_os = "windows")]
pub mod sys;

pub use geometry::{bands, region_at, regions, sort_monitors, EdgeProbe, Region, SliceProbe};
pub use model::{BandPosition, Monitor, Rect, Side, TriggerConfig};
