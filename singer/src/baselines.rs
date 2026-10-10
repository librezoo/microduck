//! The things the real generator is compared against.
//!
//! An ablation is only honest if the baseline is what people actually build first. The beak
//! baseline is the standard amplitude follower — open in proportion to loudness, smoothed — and
//! the head baseline is the beat-only nod. Both run through the *same* profile limits as the
//! real generator, so the comparison is about information used, not about who got the faster
//! servo — velocity *and* acceleration.

use crate::dsp::Slew;
use crate::perf::Performance;
use crate::profile::Profile;

/// Beak opening proportional to the loudness of the note being sung, with a first-order
/// envelope. No vowels, no closures between touching notes, no breath: what an energy-driven
/// lip-sync does.
pub fn energy_mouth(perf: &Performance, profile: &Profile) -> Vec<f32> {
    let n = (perf.duration * profile.rate).ceil() as usize + 1;
    let dt = 1.0 / profile.rate;
    let tau = 0.06f32;
    let mut out = Vec::with_capacity(n);
    let mut env = 0.0f32;
    let mut slew = Slew::at(0.0);
    let mut idx = 0usize;
    for k in 0..n {
        let t = k as f32 * dt;
        while idx < perf.syllables.len() && perf.syllables[idx].end < t {
            idx += 1;
        }
        let target = perf
            .syllables
            .get(idx)
            .filter(|s| t >= s.start)
            .map_or(0.0, |s| 0.15 + 0.75 * s.level);
        env += (target - env) * (1.0 - (-dt / tau).exp());
        out.push(slew.step(env, dt, profile.mouth.max_vel, profile.mouth.max_acc));
    }
    out
}
