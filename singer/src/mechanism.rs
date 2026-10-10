//! A stand-in for the physical beak, for asking "what does the *real* mechanism do to the curve
//! I designed?" before there is a robot on the bench.
//!
//! It models the four things that turn a clean command into a different motion: a **delay**
//! (bus and servo response), a first-order **lag**, a **velocity limit**, and the two
//! non-linearities of a geared hobby servo driving a linkage — a **deadband** (the servo does
//! not move for a small error) and **backlash** (the output lags the motor by a play that it
//! has to take up on every reversal).
//!
//! **Every default is an assumption, flagged as such and never used as a fact.** The design
//! document's stage H replaces them with measurements from a high-speed camera. What this is for
//! is *sensitivity*: a show whose articulation survives a pessimistic mechanism has margin, and a
//! show that does not is telling you which parameter to measure first.

use crate::dsp::sample_at;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mechanism {
    /// Seconds between command and the start of motion.
    pub delay: f32,
    /// First-order lag time constant, seconds.
    pub tau: f32,
    /// Fastest the output may move, fraction of stroke per second.
    pub vmax: f32,
    /// Command changes smaller than this do not move the servo, fraction of stroke.
    pub deadband: f32,
    /// Play between motor and beak, fraction of stroke.
    pub backlash: f32,
}

impl Mechanism {
    /// An ideal beak: executes the command exactly.
    pub const IDEAL: Mechanism = Mechanism {
        delay: 0.0,
        tau: 0.0,
        vmax: f32::INFINITY,
        deadband: 0.0,
        backlash: 0.0,
    };

    /// A plausible beak. **ASSUMPTION** — see the module note.
    pub fn plausible() -> Self {
        Self {
            delay: 0.020,
            tau: 0.030,
            vmax: 5.0,
            deadband: 0.010,
            backlash: 0.015,
        }
    }

    /// A pessimistic beak: slower, sloppier, later.
    pub fn pessimistic() -> Self {
        Self {
            delay: 0.045,
            tau: 0.060,
            vmax: 3.5,
            deadband: 0.025,
            backlash: 0.04,
        }
    }

    /// Execute a command track sampled at `rate`; the result is on the same grid.
    pub fn run(&self, cmd: &[f32], rate: f32) -> Vec<f32> {
        let dt = 1.0 / rate;
        let mut motor = cmd.first().copied().unwrap_or(0.0);
        let mut out_pos = motor;
        let mut held = motor;
        let mut result = Vec::with_capacity(cmd.len());
        for k in 0..cmd.len() {
            let t = k as f64 / f64::from(rate) - f64::from(self.delay);
            let target = sample_at(cmd, rate, t);
            // Deadband: the servo only takes a new setpoint once it differs enough.
            if (target - held).abs() > self.deadband {
                held = target;
            }
            // Lag and velocity limit on the motor.
            let want = if self.tau > 0.0 {
                motor + (held - motor) * (1.0 - (-dt / self.tau).exp())
            } else {
                held
            };
            let step = (want - motor).clamp(-self.vmax * dt, self.vmax * dt);
            motor += step;
            // Backlash: the beak is only dragged once the play is taken up.
            let half = self.backlash * 0.5;
            if motor > out_pos + half {
                out_pos = motor - half;
            } else if motor < out_pos - half {
                out_pos = motor + half;
            }
            result.push(out_pos.clamp(0.0, 1.0));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ideal_beak_executes_exactly() {
        let cmd: Vec<f32> = (0..100).map(|k| (k as f32 * 0.1).sin().abs()).collect();
        let out = Mechanism::IDEAL.run(&cmd, 50.0);
        for (a, b) in cmd.iter().zip(&out) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn a_real_beak_lags_and_flattens_small_moves() {
        let mut cmd = vec![0.0f32; 20];
        cmd.extend(vec![1.0; 60]);
        let out = Mechanism::plausible().run(&cmd, 50.0);
        let first = out.iter().position(|v| *v > 0.5).expect("it opens");
        assert!(first > 22, "delay and lag: opens at sample {first}");
        // A 1% flutter is inside the deadband and does nothing.
        let flutter: Vec<f32> = (0..100).map(|k| 0.5 + 0.005 * (k as f32).sin()).collect();
        let out = Mechanism::plausible().run(&flutter, 50.0);
        let span = out.iter().cloned().fold(f32::MIN, f32::max)
            - out.iter().cloned().fold(f32::MAX, f32::min);
        assert!(span < 0.01, "{span}");
    }
}
