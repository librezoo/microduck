//! The head and neck: phrase-level pose, breath, beat, stress and sway, arbitrated into one
//! bounded motion.
//!
//! ## Why layers, and why they are arbitrated
//!
//! A head that "follows the beat" is the metronome a viewer sees through in ten seconds, and a
//! head that follows the melody is a slow seesaw. What a singing head actually shows is several
//! things on *different time scales*, and the generator keeps them as separate layers so each
//! can be turned off, audited and ablated:
//!
//! | layer  | time scale | what it is |
//! |--------|------------|-----------|
//! | pose   | a phrase   | where the head points: a scan of the audience, a slight lean, moved *during the breath* and held through the phrase |
//! | arc    | a phrase   | a lift into the phrase's climax and a slow settle after it |
//! | prep   | ~0.4 s     | the inhale lift before a phrase and the small dip as it starts — the head *announces* the phrase |
//! | beat   | one beat   | a nod on the beat grid, thinned when the tempo is too fast to nod to |
//! | accent | ~0.1 s     | a quick dip on a stressed syllable |
//! | sway   | 2–3 s      | a slow roll and yaw during long notes, phase-continuous across the song |
//! | bow    | ~1.5 s     | the drop at the end of the piece |
//! | mood   | a section  | the head sits a little higher in a big section and lower in a quiet one, and gets there *seconds before* it starts (`MOOD_LEAD`) |
//! | idle   | 4–8 s      | a very slow drift that stops a held head from freezing |
//!
//! Independent layers added together make too much motion, and make it at the same instant —
//! the nod, the accent and the phrase onset land on one syllable and the head lurches. So the
//! layers have a **priority** (`prep` > `accent` > `beat` > `sway` > `arc`): higher layers *duck*
//! the lower ones while they are active, and the sum is soft-limited to the joint's budget
//! (`budget · tanh(sum / budget)`) rather than clipped, so even a pile-up bends instead of
//! flattening. Every variation between repeats comes from a hash of the seed, the section and
//! the repeat index; there is no per-frame randomness and the same song always compiles to the
//! same motion.
//!
//! The internal convention is *pitch up positive*; `HeadProfile::pitch_up_sign` converts at the
//! very end, because which way a positive head_pitch moves is unverified on hardware.

use std::f32::consts::TAU;

use crate::dsp::{alpha, bump, hash01, raised_cos, rate_limit, stepped};
use crate::perf::{Performance, Phrase, SectionKind};
use crate::profile::Profile;
use crate::style::HeadStyle;

/// How long before a section the head starts moving toward its mood. Livingstone & Palmer (2016)
/// measured emotion-related head motion beginning more than three seconds before the voice in
/// sung phrases; the duck uses two, which is what fits between most section boundaries.
pub const MOOD_LEAD: f32 = 2.0;

/// Which layers are switched on. The default is all of them; the others exist so an ablation
/// ("beat only", "no phrase structure") is a one-line change rather than a second generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layers {
    pub pose: bool,
    pub arc: bool,
    pub prep: bool,
    pub beat: bool,
    pub accent: bool,
    pub sway: bool,
    pub bow: bool,
    pub idle: bool,
    pub mood: bool,
    pub follow: bool,
    pub swing: bool,
}

impl Layers {
    pub const ALL: Layers = Layers {
        pose: true,
        arc: true,
        prep: true,
        beat: true,
        accent: true,
        sway: true,
        bow: true,
        idle: true,
        mood: true,
        follow: true,
        swing: true,
    };
    pub const NONE: Layers = Layers {
        pose: false,
        arc: false,
        prep: false,
        beat: false,
        accent: false,
        sway: false,
        bow: false,
        idle: false,
        mood: false,
        follow: false,
        swing: false,
    };
    pub fn beat_only() -> Self {
        Self {
            beat: true,
            ..Self::NONE
        }
    }
    pub fn no_phrase_structure() -> Self {
        Self {
            pose: false,
            arc: false,
            prep: false,
            mood: false,
            ..Self::ALL
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeadStats {
    /// Fraction of samples where the velocity clamp changed a joint, per joint.
    pub clamped_frac: [f32; 4],
    pub accents: usize,
    pub nods: usize,
}

struct PhraseShape {
    start: f32,
    end: f32,
    climax: f32,
    lift: f32,
    breath_gain: f32,
    yaw: f32,
    roll: f32,
}

fn section_gain(perf: &Performance, t: f32) -> (f32, u32) {
    perf.section_at(t)
        .map_or((0.8, 0), |(_, s)| (0.6 + 0.4 * s.intensity, s.variant))
}

fn phrase_shapes(perf: &Performance, st: &HeadStyle, seed: u64) -> Vec<PhraseShape> {
    let mut side = if hash01(seed, 0, 99) > 0.5 {
        1.0f32
    } else {
        -1.0
    };
    let mut out = Vec::new();
    for (k, p) in perf.phrases.iter().enumerate() {
        let sung: Vec<_> = perf
            .syllables
            .iter()
            .filter(|s| s.start >= p.start - 1e-3 && s.end <= p.end + 1e-3)
            .collect();
        let pn = if sung.is_empty() {
            0.5
        } else {
            sung.iter().map(|s| perf.pitch_norm(s.pitch)).sum::<f32>() / sung.len() as f32
        };
        // Mostly alternate, sometimes stay: a scan, not a pendulum.
        if hash01(seed, k as u64, 7) < 0.7 {
            side = -side;
        }
        let (_, variant) = section_gain(perf, p.start);
        // A repeat of the same material leans the other way round: same family, not a copy.
        let flip = if variant % 2 == 1 { -1.0 } else { 1.0 };
        let var_gain = 0.8 + 0.4 * hash01(seed, k as u64, 11);
        let amp = (0.5 + 0.5 * p.intensity) * var_gain;
        out.push(PhraseShape {
            start: p.start,
            end: p.end,
            climax: p.climax.clamp(p.start, p.end),
            lift: p.intensity
                * (0.5 + 0.5 * pn)
                * st.arc_pitch
                * (1.0 + 0.12 * variant.min(2) as f32),
            breath_gain: (p.breath / 0.5).clamp(0.25, 1.0),
            yaw: side * flip * st.arc_yaw * amp,
            roll: -side * flip * 0.6 * st.arc_roll * amp,
        });
    }
    out
}

/// The window list for a held pose: neutral → first phrase → … → neutral, each move made in the
/// breath between phrases.
fn pose_windows(
    shapes: &[PhraseShape],
    pick: impl Fn(&PhraseShape) -> f32,
) -> Vec<(f32, f32, f32)> {
    let mut w = Vec::new();
    let (Some(first), Some(last)) = (shapes.first(), shapes.last()) else {
        return w;
    };
    w.push((first.start - 0.9, first.start - 0.1, pick(first)));
    for pair in shapes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let gap = (b.start - a.end).max(0.0);
        let c = a.end + 0.5 * gap;
        let h = (0.5 * gap).clamp(0.30, 0.9);
        w.push((c - h, c + h, pick(b)));
    }
    w.push((last.end + 0.3, last.end + 1.5, 0.0));
    w
}

/// Compile the head track at `profile.rate`: `[neck_pitch, head_pitch, head_yaw, head_roll]`
/// offsets in radians, already in the robot's sign convention.
pub fn compile(
    perf: &Performance,
    st: &HeadStyle,
    profile: &Profile,
    seed: u64,
    layers: Layers,
) -> (Vec<[f32; 4]>, HeadStats) {
    let shapes = phrase_shapes(perf, st, seed);
    let mut stats = HeadStats::default();
    let beat_gap = if perf.bpm > 0.0 { 60.0 / perf.bpm } else { 1.0 };
    let thin = 1.0 / beat_gap > st.nod_rate_max;

    // Beat nods: (peak time, amplitude).
    let mut nods: Vec<(f32, f32)> = Vec::new();
    if layers.beat {
        for (i, b) in perf.beats.iter().enumerate() {
            if thin && b.strength < 0.9 {
                continue;
            }
            let in_phrase = perf
                .phrases
                .iter()
                .any(|p| b.t >= p.start - 0.1 && b.t <= p.end + 0.1);
            let w = if in_phrase { 1.0 } else { st.listen };
            let (si, variant) = section_gain(perf, b.t);
            // A first time through restrains the nod on weak beats; each repeat adds a little —
            // the same gesture family, performed a bit more.
            let vgain = if variant == 0 && b.strength < 0.9 {
                0.6
            } else {
                1.0 + 0.1 * variant.min(3) as f32
            };
            let amp = st.beat_nod
                * (0.5 + 0.5 * b.strength)
                * w
                * si
                * vgain
                * (0.85 + 0.3 * hash01(seed, i as u64, 21));
            if amp > 1e-5 {
                nods.push((b.t, amp));
            }
        }
    }
    stats.nods = nods.len();

    // Accents, with a refractory period so a run of stressed syllables is not a seizure.
    let mut accents: Vec<(f32, f32)> = Vec::new();
    if layers.accent {
        let mut last = f32::NEG_INFINITY;
        for s in &perf.syllables {
            if s.accent >= 0.55 && s.start - last >= 0.30 && s.confidence >= 0.4 {
                let (si, _) = section_gain(perf, s.start);
                accents.push((s.start, st.accent * s.accent * (0.8 + 0.4 * s.level) * si));
                last = s.start;
            }
        }
    }
    stats.accents = accents.len();

    // Sway windows: long notes, with a phase that never restarts.
    struct Sway {
        w0: f32,
        w1: f32,
        strength: f32,
    }
    let mut sways: Vec<Sway> = Vec::new();
    if layers.sway {
        for s in &perf.syllables {
            let d = s.duration();
            if d >= st.sway_after {
                sways.push(Sway {
                    w0: s.start + 0.4,
                    w1: s.end - 0.15,
                    strength: ((d - st.sway_after) / 1.5 + 0.4).min(1.0),
                });
            }
        }
    }
    let ph0 = hash01(seed, 1, 31) * TAU;
    let ph1 = hash01(seed, 2, 31) * TAU;
    let ph2 = hash01(seed, 3, 31) * TAU;
    let yaw_w = pose_windows(&shapes, |p| p.yaw);
    let roll_w = pose_windows(&shapes, |p| p.roll);
    // Mood: a quiet section lowers the head a little, a big one lifts it; instrumental parts sit
    // low and listening.
    let mood_w: Vec<(f32, f32, f32)> = perf
        .sections
        .iter()
        .map(|sec| {
            let v = match sec.kind {
                SectionKind::Intro | SectionKind::Interlude | SectionKind::Outro => -0.3,
                _ => (sec.intensity - 0.5) * 2.0,
            };
            (sec.start - MOOD_LEAD, sec.start - 0.2, st.mood_pitch * v)
        })
        .collect();

    let last_end = perf.phrases.last().map_or(perf.duration, |p| p.end);
    let n = ((perf.duration + 1.6) * profile.rate).ceil() as usize + 1;
    let follow_track = if layers.follow && st.follow > 0.0 {
        follow_curve(perf, n, profile.rate)
    } else {
        Vec::new()
    };
    let bar_s = if perf.bpm > 0.0 {
        4.0 * 60.0 / perf.bpm
    } else {
        2.0
    };
    let dt = 1.0 / profile.rate;
    let mut out = Vec::with_capacity(n);
    let mut prev = [0.0f32; 4];
    let mut clamped = [0usize; 4];
    let budget = profile.head.budget;

    for k in 0..n {
        let t = k as f32 * dt;
        // pose: yaw and roll
        let (pose_yaw, pose_roll) = if layers.pose {
            (stepped(t, 0.0, &yaw_w), stepped(t, 0.0, &roll_w))
        } else {
            (0.0, 0.0)
        };
        let mood = if layers.mood {
            stepped(t, 0.0, &mood_w)
        } else {
            0.0
        };
        // Idle drift: three incommensurate periods, so the combined motion does not repeat.
        let idle = if layers.idle {
            [
                st.idle * (TAU * t / 5.3 + ph2).sin(),
                st.idle * (TAU * t / 7.1 + ph1).sin(),
                st.idle * (TAU * t / 3.7 + ph0).sin(),
            ]
        } else {
            [0.0; 3]
        };

        // arc, prep, onset dip (pitch, up positive)
        let (mut arc, mut prep, mut dip) = (0.0f32, 0.0f32, 0.0f32);
        for sh in &shapes {
            if t < sh.start - 0.5 || t > sh.end + 1.0 {
                continue;
            }
            if layers.arc {
                let rise0 = sh.start - 0.2;
                let env = if t < sh.climax {
                    raised_cos((t - rise0) / (sh.climax - rise0).max(0.2))
                } else {
                    1.0 - raised_cos((t - sh.climax) / (sh.end + 0.8 - sh.climax).max(0.2))
                };
                arc += sh.lift * env;
            }
            if layers.prep {
                prep += st.prep * sh.breath_gain * bump((t - (sh.start - 0.45)) / 0.40);
                dip -= st.onset_nod * sh.breath_gain * alpha(t - (sh.start - 0.05), 0.12);
            }
        }
        let prep_norm = (prep.abs() + dip.abs()) / (st.prep + st.onset_nod).max(1e-6);

        // accents
        let mut acc = 0.0f32;
        let mut acc_ref = 1e-6f32;
        for &(t0, a) in &accents {
            if t < t0 - 0.1 || t > t0 + 0.5 {
                continue;
            }
            acc -= a * alpha(t - (t0 - 0.07), 0.07);
            acc_ref = acc_ref.max(a);
        }
        let acc_norm = (acc.abs() / acc_ref.max(st.accent * 0.5).max(1e-6)).min(1.0);

        // beat nods, ducked by prep and accent
        let mut beat = 0.0f32;
        for &(tb, a) in &nods {
            if t < tb - 0.15 || t > tb + 0.6 {
                continue;
            }
            beat -= a * alpha(t - (tb - 0.11), 0.11);
        }
        let g_beat = (1.0 - 0.8 * acc_norm) * (1.0 - 0.8 * prep_norm.min(1.0));
        beat *= g_beat.max(0.0);

        // sway, ducked by accent and by the beat
        let (mut sway_roll, mut sway_yaw) = (0.0f32, 0.0f32);
        for sw in &sways {
            if t < sw.w0 - 0.01 || t > sw.w1 + 0.01 {
                continue;
            }
            let len = (sw.w1 - sw.w0).max(0.2);
            let ramp = (0.5f32).min(len * 0.5);
            let e =
                raised_cos((t - sw.w0) / ramp) * (1.0 - raised_cos((t - (sw.w1 - ramp)) / ramp));
            sway_roll += st.sway_roll * sw.strength * e * (TAU * t / st.sway_period + ph0).sin();
            sway_yaw +=
                st.sway_yaw * sw.strength * e * (TAU * t / (st.sway_period * 1.57) + ph1).sin();
        }
        let g_sway =
            (1.0 - 0.6 * acc_norm) * (1.0 - 0.5 * (beat.abs() / st.beat_nod.max(1e-6)).min(1.0));
        sway_roll *= g_sway.max(0.0);
        sway_yaw *= g_sway.max(0.0);

        // the bow at the end
        let bow = if layers.bow {
            -st.final_bow * bump((t - (last_end + 0.1)) / 1.4)
        } else {
            0.0
        };

        // follow: the head rides the melody (high note, lifted head), a little behind it.
        let follow = follow_track.get(k).map_or(0.0, |v| st.follow * v);
        // swing: a groove across the bars, ducked while a nod or an accent is happening.
        let (swing_yaw, swing_roll) = if layers.swing && st.swing > 0.0 {
            let singing = perf
                .phrases
                .iter()
                .any(|p| t >= p.start - 0.3 && t <= p.end + 0.3);
            let w = if singing { 1.0 } else { st.listen * 0.7 };
            let g = (1.0 - 0.6 * acc_norm) * (1.0 - 0.5 * prep_norm.min(1.0));
            let (_, variant) = section_gain(perf, t);
            let (v, roll_v) = swing_at(t, bar_s, seed, ph1);
            (
                st.swing * w * g * (1.0 + 0.1 * variant.min(3) as f32) * v,
                0.35 * st.swing * w * g * roll_v,
            )
        } else {
            (0.0, 0.0)
        };

        let pitch = arc + prep + dip + acc + beat + bow + mood + idle[0] + follow;
        // The neck takes a third of every pitch movement: the head does not do it alone.
        let raw = [
            0.35 * pitch,
            pitch,
            pose_yaw + sway_yaw + idle[1] + swing_yaw,
            pose_roll + sway_roll + idle[2] + swing_roll,
        ];
        let mut cur = [0.0f32; 4];
        for j in 0..4 {
            let soft = budget[j] * (raw[j] / budget[j]).tanh();
            let signed = if j < 2 {
                soft * profile.head.pitch_up_sign
            } else {
                soft
            };
            let lim = rate_limit(
                prev[j],
                signed,
                dt,
                profile.head.max_vel[j] * crate::mouth::CEILING,
            );
            if (lim - signed).abs() > 0.002 {
                clamped[j] += 1;
            }
            cur[j] = lim;
        }
        prev = cur;
        out.push(cur);
    }
    for (frac, count) in stats.clamped_frac.iter_mut().zip(clamped) {
        *frac = count as f32 / n as f32;
    }
    (out, stats)
}

/// Phrase accessors the metrics use.
pub fn phrase_windows(perf: &Performance) -> Vec<Phrase> {
    perf.phrases.clone()
}

/// How far above (+) or below (−) the phrase's own centre the voice is at each output sample, in
/// octaves, smoothed with a ~0.22 s lag so the head trails the note instead of stepping on it.
/// Between phrases it relaxes to zero.
fn follow_curve(perf: &Performance, n: usize, rate: f32) -> Vec<f32> {
    let mut centre = vec![0.0f32; perf.phrases.len()];
    for (c, p) in centre.iter_mut().zip(&perf.phrases) {
        let (sum, len) = perf
            .syllables
            .iter()
            .filter(|s| s.start >= p.start - 1e-3 && s.end <= p.end + 1e-3)
            .fold((0.0f32, 0.0f32), |(a, l), s| {
                (a + s.pitch * s.duration(), l + s.duration())
            });
        *c = if len > 0.0 { sum / len } else { 60.0 };
    }
    let dt = 1.0 / rate;
    let a = alpha(dt, 0.22);
    let mut out = Vec::with_capacity(n);
    let mut y = 0.0f32;
    let mut si = 0usize;
    for k in 0..n {
        let t = k as f32 * dt;
        while si < perf.syllables.len() && perf.syllables[si].end < t {
            si += 1;
        }
        let target = match perf.syllables.get(si) {
            Some(s) if t >= s.start - 0.02 => {
                let ph = perf
                    .phrases
                    .iter()
                    .position(|p| s.start >= p.start - 1e-3 && s.end <= p.end + 1e-3);
                ph.map_or(0.0, |i| (s.pitch - centre[i]) / 12.0)
            }
            _ => 0.0,
        };
        y += a * (target - y);
        out.push(y.clamp(-1.0, 1.0));
    }
    out
}

/// The side-to-side groove at time `t`: yaw in −1..1 and a roll that leans with it (a quarter
/// cycle behind, so the head traces a slanted figure and not a line). Each bar has its own size
/// (0.4..1) and direction, chosen from the seed, and the bars are joined smoothly (the sine is
/// zero at every bar line), so the sway is locked to the music without being a pendulum.
fn swing_at(t: f32, bar_s: f32, seed: u64, phase: f32) -> (f32, f32) {
    let x = t / bar_s.max(0.5);
    let bar = x.floor();
    let frac = x - bar;
    let amp = |i: f32| {
        let size = 0.4 + 0.6 * hash01(seed, (i + 1000.0) as u64, 41);
        let dir = if hash01(seed, (i + 1000.0) as u64, 43) > 0.5 {
            1.0
        } else {
            -1.0
        };
        size * dir
    };
    // One half-wave per bar: out and back, from this bar's amplitude into the next one's.
    let a0 = amp(bar);
    let a1 = amp(bar + 1.0);
    let w = raised_cos(frac);
    let level = a0 * (1.0 - w) + a1 * w;
    let _ = phase;
    (level * (TAU * frac).sin(), 0.5 * level * (TAU * frac).cos())
}
