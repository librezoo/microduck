//! The beak: from sung syllables to a trajectory the servo can follow.
//!
//! ## What it models, and what it cannot
//!
//! One degree of freedom — how open — has to carry four things a human mouth shows with lips,
//! jaw and tongue at once, and this generator keeps the four that survive on a beak:
//!
//! 1. **Vowel openness** (`Vowel::open`), widened by pitch and loudness (`MouthStyle`).
//! 2. **Closures between syllables** — a shut beak where the consonant is, and a *dip* instead
//!    of a shut where the line is legato. This is what separates articulated singing from a
//!    beak that merely follows loudness.
//! 3. **Held notes** that stay held: a settle after the attack, then a slow drift small enough
//!    to read as breath, not as a second syllable.
//! 4. **Breath**: a closed beak through a rest, with an optional small part before the phrase.
//!
//! What it cannot show: lip rounding (`oo` vs `ah` differ only in how far), the tongue, and any
//! consonant place of articulation. That is the ceiling of a one-joint beak, and the head and
//! eyes carry the rest of the expression (`head.rs`).
//!
//! ## Feasibility is designed in, not clipped in
//!
//! Every transition between two openings takes at least the time the mechanism needs for that
//! stroke (`transition`). When the next syllable arrives too soon for a full close-and-open,
//! the *dip is made shallower* rather than the timing being broken — `max_stroke` is the
//! inverse — so a fast passage degrades into a small flutter on a held opening instead of
//! into a beak that smears. A final velocity clamp at the profile's limit is the safety net
//! (transparent to a feasible curve, unlike a tracking filter, which would lag it), and
//! `MouthStats::limiter_frac` reports how often it had to act (it should be near zero).

use std::f32::consts::{PI, TAU};

use crate::dsp::{bump, hash01, raised_cos, rate_limit};
use crate::perf::{Onset, Performance, Syllable, Vowel};
use crate::profile::Profile;
use crate::style::MouthStyle;

/// Fraction of the profile's limits the *designed* curve uses — the same as [`CEILING`], so the
/// safety clamp is headroom and not what shapes the motion.
const DESIGN: f32 = 0.85;
/// The safety clamp sits at this fraction of the profile's velocity limit, leaving the rest for
/// what the player adds when it absorbs a seek (`player::E_SCALE`): the two together never exceed
/// the limit itself.
pub const CEILING: f32 = 0.85;
/// The jaw reaches the vowel this long after the sound begins, before anticipation.
/// How far past the next syllable's start a dip between touching syllables may finish, seconds.
const LEGATO_LATE: f32 = 0.08;
const ATTACK: f32 = 0.03;
/// Shortest transition worth commanding, seconds.
const MIN_T: f32 = 0.04;

/// How long a cosine-eased stroke of `delta` must take: peak velocity is `π/2·Δ/T` and peak
/// acceleration `π²/2·Δ/T²`.
pub fn transition(delta: f32, vmax: f32, amax: f32) -> f32 {
    let d = delta.abs();
    let tv = (PI / 2.0) * d / vmax;
    let ta = PI * (d / (2.0 * amax)).sqrt();
    tv.max(ta).max(MIN_T)
}

/// The largest stroke a cosine ease can make in `t` seconds — `transition`, inverted.
pub fn max_stroke(t: f32, vmax: f32, amax: f32) -> f32 {
    ((2.0 / PI) * vmax * t).min((2.0 / (PI * PI)) * amax * t * t)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MouthStats {
    pub syllables: usize,
    /// Dips that had to be made shallower because the next syllable came too soon.
    pub depth_limited: usize,
    /// Syllables whose opening was damped for low confidence.
    pub low_confidence: usize,
    /// Fraction of samples where the safety limiter changed the designed curve by > 0.01.
    pub limiter_frac: f32,
}

enum Seg {
    Ease {
        t0: f32,
        t1: f32,
        v0: f32,
        v1: f32,
    },
    Hold {
        t0: f32,
        t1: f32,
        open: f32,
        settle: f32,
        drift: f32,
        period: f32,
        phase: f32,
    },
    /// A closed dwell with an optional small part (the inhale cue) inside it.
    Bump {
        t0: f32,
        t1: f32,
        peak: f32,
    },
}

impl Seg {
    fn t1(&self) -> f32 {
        match self {
            Seg::Ease { t1, .. } | Seg::Hold { t1, .. } | Seg::Bump { t1, .. } => *t1,
        }
    }

    fn value(&self, t: f32) -> f32 {
        match *self {
            Seg::Ease { t0, t1, v0, v1 } => {
                if t1 <= t0 {
                    v1
                } else {
                    v0 + (v1 - v0) * raised_cos((t - t0) / (t1 - t0))
                }
            }
            Seg::Hold {
                t0,
                open,
                settle,
                drift,
                period,
                phase,
                ..
            } => {
                let tau = (t - t0).max(0.0);
                let settled = open * (1.0 - settle * (1.0 - (-tau / 0.35).exp()));
                let ramp = raised_cos((tau - 0.5) / 0.7);
                settled + drift * open * ramp * (TAU * tau / period + phase).sin()
            }
            Seg::Bump { t0, t1, peak } => peak * bump((t - t0) / (t1 - t0).max(1e-3)),
        }
    }
}

fn target_open(perf: &Performance, s: &Syllable, st: &MouthStyle) -> f32 {
    if s.vowel == Vowel::Mm {
        return Vowel::Mm.open();
    }
    let pn = perf.pitch_norm(s.pitch);
    let pitch_factor = 1.0 - st.pitch_gain * (1.0 - pn);
    let level_factor = 1.0 - st.level_gain * (1.0 - s.level);
    let base = s.vowel.open() * st.gain * pitch_factor * level_factor;
    // The top of the range pulls a narrow vowel open: more the higher and the louder.
    let lift = (1.0 - base).max(0.0) * st.climax_open * pn * pn * s.level;
    (base + lift).clamp(st.floor_open, 1.0)
}

/// How much the style's legato dip is scaled by what precedes the vowel. A lip consonant (b, p,
/// m, f) shuts the beak whatever the style; a vowel-initial syllable hardly dips.
fn onset_factor(o: Onset) -> f32 {
    match o {
        Onset::Closure => 2.2,
        Onset::Soft => 1.6,
        Onset::None => 0.9,
    }
}

/// Compile the beak track for a whole performance at `profile.rate`.
pub fn compile(
    perf: &Performance,
    st: &MouthStyle,
    profile: &Profile,
    seed: u64,
) -> (Vec<f32>, MouthStats) {
    let (vmax, amax) = (
        profile.mouth.max_vel * DESIGN,
        profile.mouth.max_acc * DESIGN,
    );
    let mut stats = MouthStats {
        syllables: perf.syllables.len(),
        ..Default::default()
    };
    let sy = &perf.syllables;
    let mut segs: Vec<Seg> = Vec::new();
    // Where the curve has got to: time and opening.
    let (mut t_cur, mut v_cur) = (0.0f32, 0.0f32);
    // Silence since the last closure, for the breath cue.
    let mut gap_before = sy.first().map_or(0.0, |s| s.start);

    let mut opens: Vec<f32> = sy.iter().map(|s| target_open(perf, s, st)).collect();
    for (o, s) in opens.iter_mut().zip(sy) {
        if s.confidence < 1.0 {
            stats.low_confidence += usize::from(s.confidence < 0.5);
            let floor = st.floor_open.min(*o);
            *o = floor + (*o - floor) * (0.7 + 0.3 * s.confidence);
        }
    }
    let arrival = |s: &Syllable| s.start + ATTACK - st.anticipation;

    for (i, s) in sy.iter().enumerate() {
        let o = opens[i];
        let r = arrival(s);
        // Open: dwell where we are, then the stroke that ends at `r`.
        let t_open = transition(o - v_cur, vmax, amax);
        let start = (r - t_open).max(t_cur);
        if start > t_cur + 1e-4 {
            // The breath cue sits in a closed dwell that is long enough to hold it.
            if v_cur < 0.01
                && gap_before >= st.breath_min
                && st.breath_open > 0.0
                && start - t_cur > 0.45
            {
                let b1 = start - 0.12;
                segs.push(Seg::Ease {
                    t0: t_cur,
                    t1: b1 - 0.28,
                    v0: v_cur,
                    v1: v_cur,
                });
                segs.push(Seg::Bump {
                    t0: b1 - 0.28,
                    t1: b1,
                    peak: st.breath_open,
                });
                segs.push(Seg::Ease {
                    t0: b1,
                    t1: start,
                    v0: v_cur,
                    v1: v_cur,
                });
            } else {
                segs.push(Seg::Ease {
                    t0: t_cur,
                    t1: start,
                    v0: v_cur,
                    v1: v_cur,
                });
            }
        }
        // If the stroke could not start early enough to land on time, it lands late rather than
        // not at all: the opening is never skipped, and never a jump.
        let r = if r > start + 1e-4 {
            segs.push(Seg::Ease {
                t0: start,
                t1: r,
                v0: v_cur,
                v1: o,
            });
            r
        } else {
            segs.push(Seg::Ease {
                t0: start,
                t1: start + 0.03,
                v0: v_cur,
                v1: o,
            });
            start + 0.03
        };
        // What follows this syllable decides how it ends.
        let b = s.end;
        let (settle, drift, period, phase) = {
            let hold = (b - r).max(0.0);
            (
                st.settle * (hold / 0.8).min(1.0),
                if hold > 0.9 { st.drift } else { 0.0 },
                1.9 + 1.0 * hash01(seed, i as u64, 2),
                hash01(seed, i as u64, 1) * TAU,
            )
        };
        let hold_val = |t: f32| {
            Seg::Hold {
                t0: r,
                t1: t,
                open: o,
                settle,
                drift,
                period,
                phase,
            }
            .value(t)
        };

        match sy.get(i + 1) {
            None => {
                let t_close = transition(hold_val(b), vmax, amax);
                // The stroke starts no earlier than 0.1 s before the sound ends, so a held vowel
                // stays held until it is over.
                let vt = (b + 0.06).max(b - 0.10 + t_close);
                let s0 = (vt - t_close).max(r);
                if s0 > r {
                    segs.push(Seg::Hold {
                        t0: r,
                        t1: s0,
                        open: o,
                        settle,
                        drift,
                        period,
                        phase,
                    });
                }
                segs.push(Seg::Ease {
                    t0: s0,
                    t1: vt.max(s0 + 1e-3),
                    v0: hold_val(s0),
                    v1: 0.0,
                });
                t_cur = vt.max(s0 + 1e-3);
                v_cur = 0.0;
            }
            Some(next) => {
                let gap = (next.start - b).max(0.0);
                let o_next = opens[i + 1];
                // Where the join has to be open again. Two syllables that touch have no silence to
                // dip in, so the consonant between them is shown as a quick dip that is allowed to
                // finish a little *after* the next vowel begins, as a real consonant does: the
                // vowel starts while the lips are still coming apart. The next syllable's own
                // opening then lands late rather than not at all.
                let lips = next.onset == Onset::Closure;
                let late = if gap < st.closure_gap && !next.tie {
                    if lips { 1.5 * LEGATO_LATE } else { LEGATO_LATE }
                } else {
                    0.0
                };
                let r_next = arrival(next) + late;
                let closed = gap >= st.closure_gap && !next.tie;
                let low = o.min(o_next);
                // How deep the valley between the two vowels is asked to go: all the way for a
                // closure, as far as the line allows for a legato join.
                let mut valley = if next.tie {
                    // One syllable over several notes: no dip, the beak follows the pitch.
                    low
                } else if closed {
                    0.0
                } else {
                    (low - (st.legato_dip * onset_factor(next.onset)).min(0.95) * low).max(0.0)
                };
                // …and how deep the mechanism lets it go in the time there is. A held vowel is held
                // to nearly the end of the sound, so the close-and-open has to fit between
                // `b − 0.1` and the next arrival; where it does not, the dip is made shallower.
                let reserve = (0.4 * (b - r)).clamp(0.0, 0.08);
                let asked = valley;
                let fits = |v: f32, avail: f32| {
                    transition(o - v, vmax, amax) + transition(o_next - v, vmax, amax)
                        <= avail * 0.92
                };
                // The shallowest valley that fits in `avail`, by bisection (more valley is a
                // smaller stroke both ways, so fitting is monotone in it); `None` if even no dip
                // does not fit.
                let solve = |avail: f32| -> Option<f32> {
                    if fits(asked, avail) {
                        return Some(asked);
                    }
                    if !fits(low, avail) {
                        return None;
                    }
                    let (mut lo, mut hi) = (asked, low);
                    for _ in 0..14 {
                        let mid = 0.5 * (lo + hi);
                        if fits(mid, avail) { hi = mid } else { lo = mid }
                    }
                    Some(hi)
                };
                // Keep the vowel held to nearly its end; if that leaves no room for a worthwhile dip,
                // let the close begin earlier — a mouth shuts *into* a silence, it does not wait for
                // it, and the longer the note the more it can spare — and only a *short* syllable
                // may give the stroke its entire period. Where there is a real silence (`closed`)
                // the deepest dip any candidate allows wins, because a beak that stays open through
                // a gap reads as a mouth that never closes; elsewhere the first worthwhile one does.
                // If nothing fits, there is no dip: the two vowels join smoothly.
                let dur = (b - r).max(0.0);
                let mut candidates = vec![(b - reserve).max(r)];
                for lead in [0.08, 0.15, 0.22, 0.30] {
                    // Never more than the second half of the note: its first half is the vowel.
                    // A lip consonant may take more: it has to shut all the way.
                    if lead <= (if lips { 0.7 } else { 0.5 }) * dur {
                        candidates.push(b - lead);
                    }
                }
                if dur < 0.3 {
                    candidates.push(r);
                }
                let (mut s_pref, mut fit) = (candidates[0], None);
                for c in candidates {
                    let f = solve((r_next - c).max(MIN_T));
                    let better = match (f, fit) {
                        (Some(v), Some(best)) => v < best - 0.02,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if better {
                        s_pref = c;
                        fit = f;
                    }
                }
                if fit.is_none_or(|v| low - v < 0.03) {
                    s_pref = (b - reserve).max(r);
                    fit = Some(low);
                }
                valley = fit.unwrap_or(low);
                stats.depth_limited += usize::from(valley > asked + 0.02);
                let tc = transition(o - valley, vmax, amax);
                let s0 = s_pref;
                let vt = s0 + tc;
                if s0 > r {
                    segs.push(Seg::Hold {
                        t0: r,
                        t1: s0,
                        open: o,
                        settle,
                        drift,
                        period,
                        phase,
                    });
                }
                let vt = vt.max(s0 + 1e-3);
                segs.push(Seg::Ease {
                    t0: s0,
                    t1: vt,
                    v0: hold_val(s0),
                    v1: valley,
                });
                t_cur = vt;
                v_cur = valley;
                gap_before = if closed { gap } else { 0.0 };
            }
        }
    }

    // Render: evaluate the segment chain on the output grid, then the safety limiter.
    let n = (perf.duration * profile.rate).ceil() as usize + 1;
    let dt = 1.0 / profile.rate;
    let mut out = Vec::with_capacity(n);
    let mut prev = 0.0f32;
    let mut idx = 0usize;
    let mut limited = 0usize;
    for k in 0..n {
        let t = k as f32 * dt;
        while idx + 1 < segs.len() && t >= segs[idx].t1() {
            idx += 1;
        }
        let ideal = match segs.get(idx) {
            Some(seg) if t <= seg.t1() + 1e-4 => seg.value(t),
            Some(seg) => seg.value(seg.t1()),
            None => 0.0,
        }
        .clamp(0.0, 1.0);
        let v = rate_limit(prev, ideal, dt, profile.mouth.max_vel * CEILING);
        if (v - ideal).abs() > 0.01 {
            limited += 1;
        }
        prev = v;
        out.push(v.clamp(0.0, 1.0));
    }
    stats.limiter_frac = limited as f32 / n as f32;
    (out, stats)
}
