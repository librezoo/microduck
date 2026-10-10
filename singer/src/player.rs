//! Playback: turning "the song is at t" into a frame the joints can follow, whatever happened to
//! the song clock.
//!
//! The compiled show is feasible *along its own timeline*. A seek, a pause, a resume or a start
//! from rest jumps off that timeline, and a joint cannot follow a jump. So the player never
//! outputs `show(t)` raw after a discontinuity: it outputs `show(t) + e`, where `e` is the
//! difference between where the joint *was* and where the show says it should be, and `e`
//! decays to zero under a slew limit. In steady state `e = 0` and the output is exactly the
//! designed curve — no tracking filter in the way — and after any jump the output is continuous.
//! Stopping and pausing go the other way: the output is slewed to the rest pose, which is the
//! beak closed and the head offsets zero. Rest is always reachable and always safe to arrive at.

use crate::dsp::Slew;
use crate::show::{Frame, Show};

/// What the song clock is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clock {
    /// The song is playing and is at this time, seconds. Negative is the lead-in.
    Playing(f64),
    /// Paused: go to rest and wait.
    Paused,
    /// Stopped or finished: go to rest, then report idle.
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Performing,
    /// Returning to rest after a pause or stop.
    Releasing,
}

/// A time jump bigger than this between ticks is a seek, not playback, seconds.
const SEEK: f64 = 0.25;
/// The error `e` decays within what the designed curve leaves free (`mouth::CEILING` is 0.85 of
/// the limit), so the sum stays inside the profile's limits.
const E_SCALE: f32 = 0.15;

#[derive(Debug, Clone)]
pub struct Player {
    show: Show,
    phase: Phase,
    last_t: Option<f64>,
    /// Output state per channel: beak, neck, head pitch, yaw, roll.
    out: [f32; 5],
    /// The decaying difference between the joint and the show.
    err: [Slew; 5],
    rest: [Slew; 5],
}

fn flat(f: Frame) -> [f32; 5] {
    [f.mouth, f.head[0], f.head[1], f.head[2], f.head[3]]
}

impl Player {
    pub fn new(show: Show) -> Self {
        Self {
            show,
            phase: Phase::Idle,
            last_t: None,
            out: [0.0; 5],
            err: [Slew::default(); 5],
            rest: [Slew::default(); 5],
        }
    }

    pub fn show(&self) -> &Show {
        &self.show
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn output(&self) -> Frame {
        Frame {
            mouth: self.out[0],
            head: [self.out[1], self.out[2], self.out[3], self.out[4]],
        }
    }

    fn limits(&self, ch: usize, scale: f32) -> (f32, f32) {
        let p = &self.show.profile;
        if ch == 0 {
            (p.mouth.max_vel * scale, p.mouth.max_acc * scale)
        } else {
            (
                p.head.max_vel[ch - 1] * scale,
                p.head.max_acc[ch - 1] * scale,
            )
        }
    }

    /// Advance by `dt` seconds with the clock in state `clock`.
    pub fn tick(&mut self, dt: f32, clock: Clock) -> Frame {
        match clock {
            Clock::Playing(t) if t <= f64::from(self.show.duration()) => {
                let base = flat(self.show.sample(t));
                let jump = self.last_t.is_none_or(|l| (t - l).abs() > SEEK)
                    || self.phase != Phase::Performing;
                if jump {
                    // Absorb whatever the joints and the show disagree by into `e`.
                    for (c, b) in base.iter().enumerate() {
                        self.err[c] = Slew::at(self.out[c] - b);
                    }
                }
                self.phase = Phase::Performing;
                self.last_t = Some(t);
                for (c, b) in base.iter().enumerate() {
                    let (v, a) = self.limits(c, E_SCALE);
                    let e = self.err[c].step(0.0, dt, v, a);
                    self.out[c] = b + e;
                }
                self.out[0] = self.out[0].clamp(0.0, 1.0);
                for c in 1..5 {
                    let b = self.show.profile.head.budget[c - 1];
                    self.out[c] = self.out[c].clamp(-b, b);
                }
            }
            // Past the end of the show, a pause and a stop all mean the same thing: go to rest.
            _ => {
                if self.phase == Phase::Performing {
                    self.phase = Phase::Releasing;
                    for c in 0..5 {
                        self.rest[c] = Slew {
                            x: self.out[c],
                            v: 0.0,
                        };
                    }
                }
                self.last_t = None;
                if self.phase == Phase::Releasing {
                    let mut still = true;
                    for c in 0..5 {
                        let (v, a) = self.limits(c, 1.0);
                        self.out[c] = self.rest[c].step(0.0, dt, v, a);
                        self.out[c] = if c == 0 {
                            self.out[c].clamp(0.0, 1.0)
                        } else {
                            self.out[c].clamp(
                                -self.show.profile.head.budget[c - 1],
                                self.show.profile.head.budget[c - 1],
                            )
                        };
                        still &= self.out[c].abs() < 1e-4 && self.rest[c].v.abs() < 1e-3;
                    }
                    if still {
                        self.out = [0.0; 5];
                        self.phase = Phase::Idle;
                    }
                }
            }
        }
        self.output()
    }
}
