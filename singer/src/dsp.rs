//! The small numerical pieces everything else is built from. No state a caller cannot see, and
//! no randomness: a "random" variation here is a hash of a seed and an index, so the same song
//! compiles to the same motion on every duck and every run.

use std::f32::consts::PI;

/// 0→1 ease with zero slope at both ends.
pub fn raised_cos(x: f32) -> f32 {
    0.5 - 0.5 * (PI * x.clamp(0.0, 1.0)).cos()
}

/// 0 outside (0,1), one raised-cosine hump inside, peaking at 1 in the middle.
pub fn bump(x: f32) -> f32 {
    if x <= 0.0 || x >= 1.0 {
        0.0
    } else {
        0.5 - 0.5 * (2.0 * PI * x).cos()
    }
}

/// The "alpha function" impulse response `(τ/τ0)·e^(1−τ/τ0)`: zero before τ=0, peaks at exactly
/// 1 when τ=τ0, then decays without ringing. A nod is a quick lean in and a slower recovery,
/// which is what this is; a sinusoid is the thing a viewer reads as a metronome.
pub fn alpha(tau: f32, tau0: f32) -> f32 {
    if tau <= 0.0 {
        0.0
    } else {
        let r = tau / tau0;
        r * (1.0 - r).exp()
    }
}

/// splitmix64 finaliser → `[0, 1)`. Deterministic variation: `hash01(seed, a, b)` is a pure
/// function, so "a bit different each chorus" is reproducible and testable.
pub fn hash01(seed: u64, a: u64, b: u64) -> f32 {
    let mut z = seed
        .wrapping_add(a.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(b.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 40) as f32) / (1u64 << 24) as f32
}

/// Seed from a name, so a piece keeps its choreography when it is renamed nowhere else.
pub fn seed_of(name: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// A tracker that follows a target without exceeding a velocity or an acceleration.
///
/// It is the *last* thing in every channel, whatever produced the target: a seek, a pause, a
/// stop or a bad analysis can ask for any jump it likes and the joint still moves the way a
/// joint can. It brakes early (`v_des` is the speed from which it can still stop on the target),
/// so it does not overshoot, and it snaps only when both the error and the speed are below what
/// one step could change.
#[derive(Debug, Clone, Copy, Default)]
pub struct Slew {
    pub x: f32,
    pub v: f32,
}

impl Slew {
    pub fn at(x: f32) -> Self {
        Self { x, v: 0.0 }
    }

    pub fn step(&mut self, target: f32, dt: f32, vmax: f32, amax: f32) -> f32 {
        let err = target - self.x;
        if err.abs() < 0.5 * amax * dt * dt + 1e-5 && self.v.abs() <= amax * dt {
            self.x = target;
            self.v = 0.0;
            return self.x;
        }
        let v_stop = (1.6 * amax * err.abs()).sqrt();
        let v_des = if err >= 0.0 { 1.0 } else { -1.0 } * v_stop.min(vmax);
        let dv = (v_des - self.v).clamp(-amax * dt, amax * dt);
        self.v += dv;
        self.x += self.v * dt;
        self.x
    }
}

/// A value that holds, then eases to the next value inside each `(w0, w1, value)` window:
/// the held pose of a phrase, which moves *between* phrases and not during them. Windows are
/// applied in order and their eased deltas add, so overlapping windows still sum to the target.
pub fn stepped(t: f32, init: f32, steps: &[(f32, f32, f32)]) -> f32 {
    let (mut v, mut prev) = (init, init);
    for &(w0, w1, value) in steps {
        v += (value - prev) * raised_cos((t - w0) / (w1 - w0).max(1e-3));
        prev = value;
    }
    v
}

/// One step of a pure velocity clamp. Unlike [`Slew`] it has no state and no braking, so a
/// curve that already respects the limit passes through unchanged — which is what a safety net
/// on an offline-designed curve must do — and a jump becomes a ramp that never overshoots.
pub fn rate_limit(prev: f32, target: f32, dt: f32, vmax: f32) -> f32 {
    prev + (target - prev).clamp(-vmax * dt, vmax * dt)
}

/// Linear interpolation of a uniformly sampled track at time `t` seconds, clamped at the ends.
pub fn sample_at(samples: &[f32], rate: f32, t: f64) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let f = (t * f64::from(rate)).max(0.0);
    let i = f.floor() as usize;
    if i + 1 >= samples.len() {
        return *samples.last().expect("non-empty");
    }
    let frac = (f - i as f64) as f32;
    samples[i] * (1.0 - frac) + samples[i + 1] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_a_function_and_spreads() {
        assert_eq!(hash01(1, 2, 3), hash01(1, 2, 3));
        assert_ne!(hash01(1, 2, 3), hash01(1, 2, 4));
        let mean: f32 = (0..1000).map(|i| hash01(7, i, 0)).sum::<f32>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }

    #[test]
    fn alpha_peaks_at_one_at_tau0() {
        assert!((alpha(0.1, 0.1) - 1.0).abs() < 1e-6);
        assert_eq!(alpha(-0.1, 0.1), 0.0);
        assert!(alpha(1.0, 0.1) < 0.01);
    }

    /// A jump of any size is a ramp that respects both limits, and it lands without overshoot.
    #[test]
    fn slew_respects_limits_and_does_not_overshoot() {
        let (vmax, amax, dt) = (2.0, 20.0, 0.02);
        let mut s = Slew::at(0.0);
        let mut peak_v = 0.0f32;
        let mut peak_a = 0.0f32;
        let mut last_v = 0.0;
        let mut max_x = 0.0f32;
        for _ in 0..300 {
            s.step(1.0, dt, vmax, amax);
            peak_v = peak_v.max(s.v.abs());
            peak_a = peak_a.max(((s.v - last_v) / dt).abs());
            last_v = s.v;
            max_x = max_x.max(s.x);
        }
        assert!(peak_v <= vmax + 1e-4, "{peak_v}");
        assert!(peak_a <= amax + 1e-2, "{peak_a}");
        assert!(max_x < 1.02, "overshoot {max_x}");
        assert!((s.x - 1.0).abs() < 1e-3, "{}", s.x);
    }
}
