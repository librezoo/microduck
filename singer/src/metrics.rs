//! Measurements on a compiled show. Each one is tagged with what kind of evidence it is: these
//! are **signal-level** facts (does the curve do what the design says), not perceptual ones — a
//! beak that passes all of them can still look wrong, and the design document says how that is
//! judged instead.

use crate::perf::{Performance, SectionKind, Vowel};
use crate::show::Show;

/// The smallest beak dip counted as articulation, as a fraction of the stroke (4° of 35°). A
/// **design target**, not a perceptual threshold: it is where the design says a closure is worth
/// commanding, and the perceptual test (stage G) is what says whether it was the right number.
pub const VISIBLE_DIP: f32 = 0.12;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MouthReport {
    pub syllables: usize,
    /// Boundaries between syllables with at least 80 ms between their sounds or a re-attack.
    pub gaps: usize,
    /// …of which the beak actually dips by at least [`VISIBLE_DIP`] below the lower neighbour.
    pub gaps_articulated: usize,
    /// Gaps long enough for the mechanism to do a full close-and-open, and how many of those
    /// were articulated. If this is below 1.0 the generator dropped a gesture it could have made.
    pub gaps_feasible: usize,
    pub gaps_feasible_articulated: usize,
    /// Syllables held at least 0.8 s, and how many stay within ±0.06 of their level from 0.5 s
    /// after the attack to 0.2 s before the end (a held note that is *held*).
    pub long_notes: usize,
    pub long_notes_stable: usize,
    pub max_vel: f32,
    pub max_acc: f32,
    /// Correlation between each syllable's peak opening and its pitch (should be positive but
    /// modest: pitch widens, vowel decides).
    pub pitch_corr: f32,
    /// Correlation between each syllable's peak opening and the opening its vowel asks for. An
    /// amplitude follower has none; a vowel-aware beak should be strongly positive.
    pub vowel_corr: f32,
    /// Touching notes (no gap, a change of pitch or vowel) and how many show a dip of at least
    /// [`VISIBLE_DIP`] of the lower neighbour between them: legato that is *articulated*, not
    /// smeared.
    pub legato: usize,
    pub legato_dipped: usize,
    /// Sung syllables checked for *timing*, and how many have the beak at ≥ 80% of that syllable's
    /// own peak within 50 ms of the onset. An amplitude follower reopens late — after the sound —
    /// which is the lag a viewer reads as a mouth chasing the voice; this is what catches it.
    pub onsets: usize,
    pub opened_on_time: usize,
    /// Fraction of each ≥ 0.6 s rest during which the beak is within 0.08 of closed or within
    /// the breath cue.
    pub rest_closed: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeadReport {
    pub peak: [f32; 4],
    pub max_vel: [f32; 4],
    pub rms_vel: f32,
    /// Radians travelled per minute of song, summed over joints.
    pub path_per_min: f32,
    /// Longest stretch (seconds) with speed below 0.02 rad/s on every joint, inside a sung phrase.
    pub longest_still_in_phrase: f32,
    /// Largest |offset|/budget, per joint: ≤ 1 by construction.
    pub budget_use: [f32; 4],
    /// See [`loop_score`]; for songs long enough to have one.
    pub loop_score: Option<f32>,
}

fn slice(track: &[f32], rate: f32, a: f32, b: f32) -> &[f32] {
    let i = ((a * rate).max(0.0) as usize).min(track.len());
    let j = ((b * rate).max(0.0) as usize).min(track.len());
    &track[i..j.max(i)]
}

fn pearson(x: &[f32], y: &[f32]) -> f32 {
    let n = x.len().min(y.len());
    if n < 3 {
        return 0.0;
    }
    let (mx, my) = (
        x[..n].iter().sum::<f32>() / n as f32,
        y[..n].iter().sum::<f32>() / n as f32,
    );
    let (mut sxy, mut sxx, mut syy) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        let (dx, dy) = (x[i] - mx, y[i] - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= 1e-9 || syy <= 1e-9 {
        0.0
    } else {
        sxy / (sxx * syy).sqrt()
    }
}

/// Report on a beak track against the performance it was made for. `track` may be the designed
/// curve or the output of a `Mechanism` — the point of the latter is to see what survives.
pub fn mouth_report(
    perf: &Performance,
    track: &[f32],
    rate: f32,
    vmax_design: f32,
    amax_design: f32,
) -> MouthReport {
    let mut r = MouthReport {
        syllables: perf.syllables.len(),
        ..Default::default()
    };
    let dt = 1.0 / rate;
    let mut peaks = Vec::new();
    let mut pitches = Vec::new();
    let mut vowels = Vec::new();
    for (i, s) in perf.syllables.iter().enumerate() {
        let inside = slice(track, rate, s.start, s.end);
        let peak = inside.iter().cloned().fold(0.0f32, f32::max);
        if s.vowel != Vowel::Mm && peak > 0.05 {
            r.onsets += 1;
            let at50 = slice(track, rate, s.start + 0.05, s.start + 0.07)
                .iter()
                .cloned()
                .fold(0.0f32, f32::max);
            r.opened_on_time += usize::from(at50 >= 0.8 * peak);
        }
        if s.vowel != Vowel::Mm {
            peaks.push(peak);
            pitches.push(s.pitch);
            vowels.push(s.vowel.open());
        }
        let d = s.duration();
        if d >= 0.8 {
            r.long_notes += 1;
            let hold = slice(track, rate, s.start + 0.5, s.end - 0.2);
            if hold.len() >= 2 {
                let (lo, hi) = (
                    hold.iter().cloned().fold(f32::MAX, f32::min),
                    hold.iter().cloned().fold(f32::MIN, f32::max),
                );
                if hi - lo <= 0.12 {
                    r.long_notes_stable += 1;
                }
            }
        }
        if let Some(n) = perf.syllables.get(i + 1) {
            let gap = n.start - s.end;
            let mid_a = 0.5 * (s.start + s.end);
            let mid_b = 0.5 * (n.start + n.end);
            if gap < 0.08
                && n.onset != crate::perf::Onset::Closure
                && s.duration() >= 0.15
                && n.duration() >= 0.15
            {
                r.legato += 1;
                let valley = slice(track, rate, mid_a, mid_b)
                    .iter()
                    .cloned()
                    .fold(f32::MAX, f32::min);
                let lower = peak.min(
                    slice(track, rate, n.start, n.end)
                        .iter()
                        .cloned()
                        .fold(0.0f32, f32::max),
                );
                r.legato_dipped += usize::from(lower - valley >= VISIBLE_DIP);
            }
            let articulated_gap = gap >= 0.08 || n.onset == crate::perf::Onset::Closure;
            if articulated_gap {
                r.gaps += 1;
                let valley = slice(track, rate, mid_a, mid_b)
                    .iter()
                    .cloned()
                    .fold(f32::MAX, f32::min);
                let lower = peak.min(
                    slice(track, rate, n.start, n.end)
                        .iter()
                        .cloned()
                        .fold(0.0f32, f32::max),
                );
                let dipped = lower - valley >= VISIBLE_DIP;
                r.gaps_articulated += usize::from(dipped);
                // Enough period (onset to onset) for a full-depth stroke both ways at the design limits.
                let period = n.start - s.start;
                let stroke = lower.max(0.2) * 0.6;
                let need = 2.0 * crate::mouth::transition(stroke, vmax_design, amax_design);
                if period >= need * 1.1 {
                    r.gaps_feasible += 1;
                    r.gaps_feasible_articulated += usize::from(dipped);
                }
            }
        }
    }
    r.pitch_corr = pearson(&peaks, &pitches);
    r.vowel_corr = pearson(&peaks, &vowels);
    for w in track.windows(2) {
        r.max_vel = r.max_vel.max(((w[1] - w[0]) / dt).abs());
    }
    for w in track.windows(3) {
        r.max_acc = r
            .max_acc
            .max(((w[2] - 2.0 * w[1] + w[0]) / (dt * dt)).abs());
    }
    // Rests
    let (mut rest_total, mut rest_ok) = (0usize, 0usize);
    let mut prev_end = 0.0f32;
    for s in &perf.syllables {
        if s.start - prev_end >= 0.6 && prev_end > 0.0 {
            let seg = slice(track, rate, prev_end + 0.15, s.start - 0.05);
            rest_total += seg.len();
            rest_ok += seg.iter().filter(|v| **v <= 0.2).count();
        }
        prev_end = s.end;
    }
    r.rest_closed = if rest_total == 0 {
        1.0
    } else {
        rest_ok as f32 / rest_total as f32
    };
    r
}

pub fn head_report(show: &Show) -> HeadReport {
    let rate = show.profile.rate;
    let dt = 1.0 / rate;
    let track = &show.head;
    let mut r = HeadReport::default();
    let budget = show.profile.head.budget;
    let mut speed_env = Vec::with_capacity(track.len());
    let mut sq = 0.0f32;
    let mut path = 0.0f32;
    for (k, f) in track.iter().enumerate() {
        for j in 0..4 {
            r.peak[j] = r.peak[j].max(f[j].abs());
            r.budget_use[j] = r.budget_use[j].max(f[j].abs() / budget[j]);
        }
        let mut s = 0.0f32;
        if k > 0 {
            for j in 0..4 {
                let v = (f[j] - track[k - 1][j]) / dt;
                r.max_vel[j] = r.max_vel[j].max(v.abs());
                s += v * v;
                path += v.abs() * dt;
            }
        }
        sq += s;
        speed_env.push(s.sqrt());
    }
    r.rms_vel = (sq / track.len().max(1) as f32).sqrt();
    r.path_per_min = path / (track.len() as f32 / rate / 60.0).max(1e-3);
    // Longest stillness inside phrases.
    let mut longest = 0.0f32;
    for p in &show.perf.phrases {
        let mut run = 0usize;
        let seg = slice(&speed_env, rate, p.start, p.end);
        let mut best = 0usize;
        for v in seg {
            if *v < 0.02 {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        longest = longest.max(best as f32 / rate);
    }
    r.longest_still_in_phrase = longest;
    let dur = track.len() as f32 / rate;
    r.loop_score = loop_score(&speed_env, rate, dur);
    r
}

/// Peak autocorrelation of an activity envelope at lags of 8–40 s, after removing everything
/// slower than ~6 s (a slow swell is not a loop). `None` for a track too short to contain one.
/// High means the motion repeats on a multi-second cycle; compare it with the same measure on the
/// *music* (`music_loop`), since a song that loops is allowed to have motion that does too.
pub fn loop_score(envelope: &[f32], rate: f32, duration: f32) -> Option<f32> {
    if duration < 90.0 {
        return None;
    }
    let step = ((rate / 4.0) as usize).max(1);
    let env4: Vec<f32> = envelope
        .chunks(step)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    let w = 12usize;
    let detr: Vec<f32> = (0..env4.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env4.len()));
            env4[i] - env4[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect();
    let var: f32 = detr.iter().map(|v| v * v).sum();
    if var <= 1e-9 {
        return Some(0.0);
    }
    let mut best = f32::MIN;
    for lag in (8 * 4)..(40 * 4).min(detr.len() / 2) {
        let c: f32 = detr.iter().zip(&detr[lag..]).map(|(a, b)| a * b).sum();
        best = best.max(c / var);
    }
    Some(best)
}

/// The loop score of the *song*: syllable loudness on the same grid.
pub fn music_loop(perf: &Performance, rate: f32) -> Option<f32> {
    let n = (perf.duration * rate) as usize + 1;
    let mut env = vec![0.0f32; n];
    for s in &perf.syllables {
        let (a, b) = ((s.start * rate) as usize, ((s.end * rate) as usize).min(n));
        if a < b {
            env[a..b].fill(s.level);
        }
    }
    loop_score(&env, rate, perf.duration)
}

/// Similarity of the head motion across repeats of the same kind of section. Each pair is
/// resampled to a common length and compared on the three angle channels; the result is the
/// Pearson correlation. **Near 1.0 is a copy; near 0 is unrelated.** A repeated chorus should
/// sit between: recognisably the same family, visibly not a replay.
pub fn repeat_similarity(show: &Show) -> Vec<(SectionKind, u32, u32, f32)> {
    let rate = show.profile.rate;
    let mut out = Vec::new();
    let secs = &show.perf.sections;
    for i in 0..secs.len() {
        for j in i + 1..secs.len() {
            if secs[i].kind != secs[j].kind
                || secs[i].kind == SectionKind::Intro
                || secs[i].kind == SectionKind::Outro
            {
                continue;
            }
            let len = (secs[i].end - secs[i].start).min(secs[j].end - secs[j].start);
            if len < 4.0 {
                continue;
            }
            let n = (len * 10.0) as usize;
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for k in 0..n {
                let ta = secs[i].start + k as f32 * 0.1;
                let tb = secs[j].start + k as f32 * 0.1;
                let fa = show.head[((ta * rate) as usize).min(show.head.len() - 1)];
                let fb = show.head[((tb * rate) as usize).min(show.head.len() - 1)];
                for c in 1..4 {
                    a.push(fa[c]);
                    b.push(fb[c]);
                }
            }
            out.push((
                secs[i].kind,
                secs[i].variant,
                secs[j].variant,
                pearson(&a, &b),
            ));
        }
    }
    out
}
