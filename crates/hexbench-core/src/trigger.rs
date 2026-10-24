//! Trigger state machine.
//!
//! A plain "cursor is inside the zone" check is not enough: once the user
//! parks the mouse in the corner, a cooldown-only design keeps switching
//! desktops forever. The machine therefore requires the cursor to *leave*
//! before it can fire again.
//!
//! ```text
//!          enter                dwell elapsed
//! Idle ───────────▶ Armed ───────────────▶ Fire ──▶ Disarmed
//!   ▲                 │                                 │
//!   └── left early ◀──┘                                 │
//!   └──────────────── leave the zone ───────────────────┘
//! ```

use crate::model::Side;

/// Which way the desktop should move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Prev,
    Next,
}

/// A cursor position that lies inside one triggerable region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub monitor: usize,
    pub side: Side,
    pub edge_x: i32,
}

impl Hit {
    pub fn direction(self) -> Direction {
        match self.side {
            Side::Left => Direction::Prev,
            Side::Right => Direction::Next,
        }
    }
}

/// Emitted once per accepted trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fire {
    pub hit: Hit,
    pub at_ms: u64,
}

/// Timing knobs, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub dwell_ms: u64,
    pub cooldown_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self { dwell_ms: 120, cooldown_ms: 500 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Armed,
    Disarmed,
}

#[derive(Debug)]
pub struct Machine {
    phase: Phase,
    since: u64,
    armed: Option<Hit>,
    last_fire: Option<u64>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        Self { phase: Phase::Idle, since: 0, armed: None, last_fire: None }
    }

    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    /// Feed one observation. `hit` is `None` when the cursor is not in a zone
    /// (or a guard such as "fullscreen" is active).
    pub fn step(&mut self, hit: Option<Hit>, now_ms: u64, timing: Timing) -> Option<Fire> {
        match self.phase {
            Phase::Idle => {
                if let Some(h) = hit {
                    let cooled = self
                        .last_fire
                        .is_none_or(|fired| now_ms.saturating_sub(fired) >= timing.cooldown_ms);
                    if cooled {
                        self.phase = Phase::Armed;
                        self.since = now_ms;
                        self.armed = Some(h);
                    }
                }
            }
            Phase::Armed => match hit {
                None => {
                    self.phase = Phase::Idle;
                    self.armed = None;
                }
                Some(h) if Some(h) != self.armed => {
                    // Moved to a different zone: restart the dwell.
                    self.since = now_ms;
                    self.armed = Some(h);
                }
                Some(h) => {
                    if now_ms.saturating_sub(self.since) >= timing.dwell_ms {
                        self.phase = Phase::Disarmed;
                        self.armed = None;
                        self.last_fire = Some(now_ms);
                        return Some(Fire { hit: h, at_ms: now_ms });
                    }
                }
            },
            Phase::Disarmed => {
                if hit.is_none() {
                    self.phase = Phase::Idle;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit_left() -> Hit {
        Hit { monitor: 0, side: Side::Left, edge_x: 0 }
    }

    fn timing() -> Timing {
        Timing { dwell_ms: 100, cooldown_ms: 500 }
    }

    #[test]
    fn fires_after_dwell_only() {
        let mut m = Machine::new();
        assert!(m.step(Some(hit_left()), 0, timing()).is_none());
        assert!(m.step(Some(hit_left()), 99, timing()).is_none());
        let fire = m.step(Some(hit_left()), 100, timing()).expect("should fire");
        assert_eq!(fire.hit, hit_left());
        assert_eq!(fire.hit.direction(), Direction::Prev);
    }

    #[test]
    fn leaving_before_dwell_cancels() {
        let mut m = Machine::new();
        m.step(Some(hit_left()), 0, timing());
        assert!(m.step(None, 50, timing()).is_none());
        assert!(m.is_idle());
        // A new entry starts the dwell from scratch.
        assert!(m.step(Some(hit_left()), 60, timing()).is_none());
        assert!(m.step(Some(hit_left()), 160, timing()).is_some());
    }

    #[test]
    fn parking_in_the_corner_fires_once() {
        let mut m = Machine::new();
        let mut fires = 0;
        for t in (0..2000).step_by(50) {
            if m.step(Some(hit_left()), t, timing()).is_some() {
                fires += 1;
            }
        }
        assert_eq!(fires, 1, "must not keep switching while parked");
    }

    #[test]
    fn must_leave_before_rearming() {
        let mut m = Machine::new();
        m.step(Some(hit_left()), 0, timing());
        m.step(Some(hit_left()), 100, timing()).unwrap();

        // Still inside: no second fire, no re-arm.
        for t in (150..2000).step_by(50) {
            assert!(m.step(Some(hit_left()), t, timing()).is_none());
        }
        // Leave and come back after the cooldown.
        m.step(None, 2100, timing());
        m.step(Some(hit_left()), 2110, timing());
        assert!(m.step(Some(hit_left()), 2210, timing()).is_some());
    }

    #[test]
    fn cooldown_blocks_a_quick_return() {
        let mut m = Machine::new();
        m.step(Some(hit_left()), 0, timing());
        m.step(Some(hit_left()), 100, timing()).unwrap(); // fired at 100

        m.step(None, 150, timing());
        m.step(Some(hit_left()), 200, timing()); // inside cooldown: not armed
        assert!(m.step(Some(hit_left()), 300, timing()).is_none());
        // After the cooldown it arms again and needs a fresh dwell.
        m.step(Some(hit_left()), 650, timing());
        assert!(m.step(Some(hit_left()), 750, timing()).is_some());
    }

    #[test]
    fn switching_zones_restarts_the_dwell() {
        let mut m = Machine::new();
        let right = Hit { monitor: 0, side: Side::Right, edge_x: 1919 };
        m.step(Some(hit_left()), 0, timing());
        m.step(Some(right), 80, timing());
        assert!(m.step(Some(right), 150, timing()).is_none());
        assert!(m.step(Some(right), 180, timing()).is_some());
    }
}
