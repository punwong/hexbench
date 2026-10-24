//! The headless engine: poll → guard → hit-test → state machine → switch.
//!
//! Kept free of Tauri and of any UI, so the same code can be driven by the CLI
//! (`hexbench run`) and, later, by the tray app.

use std::time::Instant;

use crate::config::Config;
use crate::geometry::{regions, sort_monitors, Region};
use crate::model::{Monitor, Side};
use crate::trigger::{Fire, Hit, Machine, Timing};
use crate::{sys, Rect};

/// What happened during one tick, for logging.
#[derive(Debug, Clone, Copy)]
pub struct Tick {
    /// Machine phase before this tick: idle / armed / disarmed.
    pub phase: &'static str,
    pub at_ms: u64,
    pub cursor: Option<(i32, i32)>,
    pub hit: Option<Hit>,
    pub fire: Option<Fire>,
    pub switched: bool,
    pub guard: Option<&'static str>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub ticks: u64,
    pub fires: u64,
    pub send_failures: u64,
}

pub struct Engine {
    config: Config,
    monitors: Vec<Monitor>,
    trigger_regions: Vec<Region>,
    machine: Machine,
    started: Instant,
    last_lock_check: Instant,
    locked: bool,
    last_process_check: Instant,
    blacklisted: Option<String>,
    dry_run: bool,
    stats: Stats,
}

impl Engine {
    pub fn new(config: Config, dry_run: bool) -> Self {
        let mut engine = Self {
            config,
            monitors: Vec::new(),
            trigger_regions: Vec::new(),
            machine: Machine::new(),
            started: Instant::now(),
            last_lock_check: Instant::now(),
            locked: false,
            last_process_check: Instant::now(),
            blacklisted: None,
            dry_run,
            stats: Stats::default(),
        };
        engine.refresh();
        engine
    }

    /// Re-read the monitor layout. Call after `WM_DISPLAYCHANGE`.
    pub fn refresh(&mut self) {
        self.monitors = sys::enumerate_monitors();
        sort_monitors(&mut self.monitors);
        self.trigger_regions = regions(&self.monitors, &self.config.geometry());
    }

    /// Apply a new configuration (from the UI) and recompute the regions.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
        self.refresh();
    }

    /// Test mode: report hits but never send the shortcut.
    pub fn set_dry_run(&mut self, dry_run: bool) {
        self.dry_run = dry_run;
    }

    /// Forget any armed/disarmed state (used when pausing).
    pub fn reset(&mut self) {
        self.machine = Machine::new();
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    pub fn regions(&self) -> &[Region] {
        &self.trigger_regions
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Current trigger-machine phase, for diagnostics.
    pub fn phase(&self) -> &'static str {
        self.machine.phase_name()
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        let index = self.monitors.iter().position(|m| m.rect.contains(x, y))?;
        let monitor = &self.monitors[index];
        let geometry = self.config.geometry();
        for side in Side::all() {
            if !geometry.side_enabled(side) || side.edge_x(monitor) != x {
                continue;
            }
            let inside = self.trigger_regions.iter().any(|region| {
                region.monitor == index && region.side == side && y >= region.y0 && y < region.y1
            });
            if inside {
                return Some(Hit { monitor: index, side, edge_x: x });
            }
        }
        None
    }

    fn guard_reason(&mut self, x: i32, y: i32) -> Option<&'static str> {
        let guard = &self.config.guard;

        // Opening the input desktop is comparatively expensive; twice a second
        // is plenty for a lock-screen check.
        if guard.skip_when_locked && self.last_lock_check.elapsed().as_millis() >= 500 {
            self.locked = sys::is_locked();
            self.last_lock_check = Instant::now();
        }
        if guard.skip_when_locked && self.locked {
            return Some("locked");
        }
        if guard.skip_when_button_down && sys::mouse_button_down() {
            return Some("button");
        }
        if guard.skip_fullscreen {
            if let Some(kind) = sys::foreground_fullscreen_kind() {
                return Some(kind);
            }
        }
        if guard.exclude_taskbar && sys::cursor_over_taskbar(x, y) {
            return Some("taskbar");
        }
        // Process lookup opens a handle; only do it when a list exists, and
        // reuse the answer for a moment so the poll rate does not matter.
        if !guard.process_blacklist.is_empty() {
            if self.last_process_check.elapsed().as_millis() >= 250 {
                let foreground = sys::foreground_process_name();
                self.blacklisted = foreground.filter(|name| {
                    guard
                        .process_blacklist
                        .iter()
                        .any(|entry| entry.eq_ignore_ascii_case(name))
                });
                self.last_process_check = Instant::now();
            }
            if self.blacklisted.is_some() {
                return Some("blacklist");
            }
        }
        None
    }

    /// One poll. `dwell`/`cooldown` come from the config.
    pub fn tick(&mut self) -> Tick {
        let at_ms = self.now_ms();
        self.stats.ticks += 1;

        let mut tick = Tick {
            phase: self.machine.phase_name(),
            at_ms,
            cursor: None,
            hit: None,
            fire: None,
            switched: false,
            guard: None,
        };

        if !self.config.enabled {
            tick.guard = Some("disabled");
            self.machine.step(None, at_ms, self.timing());
            return tick;
        }

        let Some((x, y)) = sys::cursor_pos() else {
            self.machine.step(None, at_ms, self.timing());
            return tick;
        };
        tick.cursor = Some((x, y));

        let guard = self.guard_reason(x, y);
        tick.guard = guard;
        let hit = if guard.is_none() { self.hit_at(x, y) } else { None };
        tick.hit = hit;

        let fire = self.machine.step(hit, at_ms, self.timing());
        if let Some(fire) = fire {
            tick.fire = Some(fire);
            self.stats.fires += 1;
            if self.dry_run {
                tick.switched = true;
            } else {
                let ok = sys::send_desktop_switch(fire.hit.direction());
                tick.switched = ok;
                if !ok {
                    self.stats.send_failures += 1;
                } else if self.config.trigger.restore_cursor {
                    let offset = self.config.trigger.restore_offset_px;
                    let target = match fire.hit.side {
                        Side::Left => fire.hit.edge_x + offset,
                        Side::Right => fire.hit.edge_x - offset,
                    };
                    sys::set_cursor_pos(target, y);
                }
            }
        }

        tick
    }

    fn timing(&self) -> Timing {
        Timing {
            dwell_ms: self.config.trigger.dwell_ms,
            cooldown_ms: self.config.trigger.cooldown_ms,
        }
    }
}

/// Small helper for the CLI: a monitor's rect for logging.
pub fn monitor_rect(engine: &Engine, index: usize) -> Option<Rect> {
    engine.monitors().get(index).map(|m| m.rect)
}
