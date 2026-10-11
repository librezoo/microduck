//! The performance representation: what was sung, when, and how it was shaped — and nothing
//! about joints.
//!
//! Everything upstream (a score, an aligner, a person correcting one syllable) writes this;
//! everything downstream (the mouth and head generators, the metrics) reads it. It is events on
//! one clock — **song time, seconds** — because the things that matter to a viewer are events:
//! a syllable starts, a breath is taken, a phrase peaks. Continuous curves are *derived* from it
//! and are never stored as the source of truth, which is what makes a local correction possible:
//! move one syllable boundary and recompile, instead of re-editing a curve.
//!
//! Every event carries a confidence, because an automatic front end is wrong some of the time and
//! the generator should know where: low-confidence events get smaller, slower motion rather than
//! a confident wrong one (see `mouth::compile`).

use serde::{Deserialize, Serialize};

/// The mouth shape a sung vowel asks for. Stage vowels rather than phonetics — the same six the
/// duck's own voice has, in the same order of openness — because on a one-degree-of-freedom beak
/// the shape that survives is "how open", and these six are what an aligner's phonemes collapse
/// to: open (ah), mid (eh, oh), narrow (ee, oo), closed hum (mm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vowel {
    Ah,
    Eh,
    Ee,
    Oh,
    Oo,
    Mm,
}

impl Vowel {
    /// How far the beak opens on it, 0..1 — **the same numbers** `sounds::chorale::Vowel::open`
    /// uses, so the shape a viewer sees is the one the synth is singing. Exaggerated "stage"
    /// values, deliberately (see that function's note on why phonetically honest ones looked
    /// broken on the robot).
    pub fn open(self) -> f32 {
        match self {
            Vowel::Ah => 0.90,
            Vowel::Eh => 0.65,
            Vowel::Ee => 0.45,
            Vowel::Oh => 0.55,
            Vowel::Oo => 0.35,
            Vowel::Mm => 0.05,
        }
    }
}

/// What precedes the vowel — the part of the consonant a one-joint beak can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Onset {
    /// Vowel-initial, or a continuous sound (l, n, s, h): the jaw barely moves.
    #[default]
    None,
    /// A soft consonant: a partial narrowing.
    Soft,
    /// A closure (b, p, m, d, t, g, k) or a re-attack on the same pitch: the beak shuts.
    Closure,
}

/// One sung syllable. `start`/`end` are when the *sound* is there, not when the beak moves — the
/// generator decides that, because anticipation is a property of the style and the mechanism,
/// not of the song.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Syllable {
    pub start: f32,
    pub end: f32,
    pub vowel: Vowel,
    #[serde(default)]
    pub onset: Onset,
    /// MIDI pitch, fractional. Drives how wide the beak opens, never *whether* it opens.
    pub pitch: f32,
    /// Loudness, 0..1.
    pub level: f32,
    /// Stress, 0..1: a syllable the singer leans on. ≥ 0.55 earns a head accent.
    #[serde(default)]
    pub accent: f32,
    /// How much to believe this event, 0..1. 1.0 for hand-authored or score-derived.
    #[serde(default = "one")]
    pub confidence: f32,
    /// This note is a continuation of the previous syllable's vowel — one syllable sung over
    /// several notes (一字多音, a melisma). The beak does not dip between them: it follows the pitch.
    /// Only an alignment against lyrics can know this; a score alone cannot.
    #[serde(default)]
    pub tie: bool,
}

fn one() -> f32 {
    1.0
}

impl Syllable {
    pub fn duration(&self) -> f32 {
        self.end - self.start
    }
}

/// A sung phrase: the unit a head moves in. The breath before it is part of it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    pub start: f32,
    pub end: f32,
    /// Silence before `start` that a singer would breathe in, seconds. 0 when the phrase runs
    /// on from the last one.
    #[serde(default)]
    pub breath: f32,
    /// When the phrase peaks (its highest or strongest moment).
    pub climax: f32,
    /// Expressive weight, 0..1: how big this phrase is in the song.
    pub intensity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    Intro,
    Verse,
    Chorus,
    Bridge,
    Interlude,
    Outro,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub kind: SectionKind,
    pub start: f32,
    pub end: f32,
    /// 0..1, how much the song is asking for here.
    pub intensity: f32,
    /// Which time through this *material* it is: 0 for the first chorus, 1 for the second. The
    /// generator uses it so a repeat is recognisably the same gesture family and visibly not a
    /// copy.
    #[serde(default)]
    pub variant: u32,
}

/// A beat on the song clock. `strength` is 1.0 for a downbeat, lower inside the bar.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    pub t: f32,
    pub strength: f32,
}

/// A sung song, described.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Performance {
    pub name: String,
    pub bpm: f32,
    /// Where the description came from, for the log: `score:duck-strut/soprano`, `aligner:…`.
    #[serde(default)]
    pub source: String,
    pub duration: f32,
    pub syllables: Vec<Syllable>,
    pub phrases: Vec<Phrase>,
    pub sections: Vec<Section>,
    pub beats: Vec<Beat>,
}

/// What is wrong with a performance, so a hand-edit that breaks the invariants says so rather
/// than producing a strange head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid performance: {}", self.0)
    }
}
impl std::error::Error for Invalid {}

/// Longest song a performance may describe, seconds. The tracks are sized from it (200 samples a
/// second, four of them), so an unbounded duration is an unbounded allocation.
pub const MAX_DURATION_S: f32 = 4.0 * 3600.0;
/// Tempo bounds for a known tempo (the Studio route bounds it to 20..300; this is wider).
pub const MIN_BPM: f32 = 1.0;
pub const MAX_BPM: f32 = 1000.0;
/// MIDI-ish pitch bound: far outside any voice, well inside what f32 arithmetic can square.
pub const MAX_PITCH: f32 = 1000.0;

impl Performance {
    pub fn from_json(text: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let p: Performance = serde_json::from_str(text)?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a performance serialises")
    }

    /// Time-ordered, non-overlapping syllables; phrases and sections inside the song; sane ranges.
    pub fn validate(&self) -> Result<(), Invalid> {
        let bad = |m: String| Err(Invalid(m));
        if !(self.duration.is_finite() && self.duration > 0.0 && self.duration <= MAX_DURATION_S) {
            return bad(format!("duration {}", self.duration));
        }
        // 0 or less means "tempo unknown" (every consumer guards `bpm > 0`); anything else has to
        // be a tempo a person could count.
        if !(self.bpm <= 0.0 || (MIN_BPM..=MAX_BPM).contains(&self.bpm)) {
            return bad(format!("bpm {}", self.bpm));
        }
        let mut prev_end = 0.0f32;
        for (i, s) in self.syllables.iter().enumerate() {
            if !(s.start.is_finite() && s.end.is_finite()) || s.end <= s.start {
                return bad(format!("syllable {i}: {}..{}", s.start, s.end));
            }
            if s.start < prev_end - 1e-4 {
                return bad(format!(
                    "syllable {i} starts at {} before the last ends",
                    s.start
                ));
            }
            if s.end > self.duration + 1e-3 {
                return bad(format!("syllable {i} ends past the song"));
            }
            if !(s.pitch.abs() <= MAX_PITCH) {
                return bad(format!("syllable {i}: pitch {}", s.pitch));
            }
            for (name, v) in [
                ("level", s.level),
                ("accent", s.accent),
                ("confidence", s.confidence),
            ] {
                if !(0.0..=1.0).contains(&v) {
                    return bad(format!("syllable {i}: {name} {v} outside 0..1"));
                }
            }
            prev_end = s.end;
        }
        for (i, p) in self.phrases.iter().enumerate() {
            let inside = p.start >= 0.0 && p.end > p.start && p.end <= self.duration + 1e-3;
            if !(inside && p.climax.is_finite() && p.breath.is_finite()) {
                return bad(format!("phrase {i}: {}..{}", p.start, p.end));
            }
        }
        for (i, s) in self.sections.iter().enumerate() {
            if !(s.start.is_finite() && s.end.is_finite() && s.end > s.start) {
                return bad(format!("section {i}: {}..{}", s.start, s.end));
            }
        }
        for (i, b) in self.beats.iter().enumerate() {
            if !(b.t.is_finite() && b.strength.is_finite()) {
                return bad(format!("beat {i}: {} / {}", b.t, b.strength));
            }
        }
        Ok(())
    }

    /// The same performance starting `secs` later: a lead-in in which the head can prepare and
    /// the beak is already shut. Everything moves — syllables, phrases, beats, sections — except
    /// the start of the first section, which stays at 0 so the lead-in belongs to it.
    pub fn with_pre_roll(mut self, secs: f32) -> Self {
        for s in &mut self.syllables {
            s.start += secs;
            s.end += secs;
        }
        for p in &mut self.phrases {
            p.start += secs;
            p.end += secs;
            p.climax += secs;
        }
        for b in &mut self.beats {
            b.t += secs;
        }
        for (i, s) in self.sections.iter_mut().enumerate() {
            if i > 0 || s.start > 0.0 {
                s.start += secs;
            }
            s.end += secs;
        }
        self.duration += secs;
        self
    }

    /// The 5th–95th percentile of sung pitch, which is what "high" and "low" mean *in this
    /// song*: a beak that opens wide on the highest note of the piece, not on a fixed note.
    pub fn pitch_range(&self) -> (f32, f32) {
        let mut p: Vec<f32> = self
            .syllables
            .iter()
            .filter(|s| s.vowel != Vowel::Mm)
            .map(|s| s.pitch)
            .collect();
        if p.is_empty() {
            return (60.0, 72.0);
        }
        p.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        let at = |q: f32| p[((p.len() - 1) as f32 * q).round() as usize];
        let (lo, hi) = (at(0.05), at(0.95));
        if hi - lo < 3.0 {
            (lo - 1.5, hi + 1.5)
        } else {
            (lo, hi)
        }
    }

    /// Where `pitch` sits in the song's range, 0..1.
    pub fn pitch_norm(&self, pitch: f32) -> f32 {
        let (lo, hi) = self.pitch_range();
        ((pitch - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    /// The section containing `t`, if any.
    pub fn section_at(&self, t: f32) -> Option<(usize, &Section)> {
        self.sections
            .iter()
            .enumerate()
            .find(|(_, s)| t >= s.start && t < s.end)
    }

    /// Syllables per second of *singing* — the density that decides how much articulation a
    /// mechanism can be asked for, and which style fits.
    pub fn sung_density(&self) -> f32 {
        let sung: f32 = self.phrases.iter().map(|p| p.end - p.start).sum();
        if sung <= 0.0 {
            0.0
        } else {
            self.syllables.len() as f32 / sung
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn tiny() -> Performance {
        Performance {
            name: "tiny".into(),
            bpm: 100.0,
            source: "test".into(),
            duration: 4.0,
            syllables: vec![
                Syllable {
                    start: 0.5,
                    end: 1.5,
                    vowel: Vowel::Ah,
                    onset: Onset::None,
                    pitch: 62.0,
                    level: 0.7,
                    accent: 0.0,
                    confidence: 1.0,
                    tie: false,
                },
                Syllable {
                    start: 1.6,
                    end: 2.4,
                    vowel: Vowel::Oo,
                    onset: Onset::Closure,
                    pitch: 67.0,
                    level: 0.8,
                    accent: 0.6,
                    confidence: 1.0,
                    tie: false,
                },
            ],
            phrases: vec![Phrase {
                start: 0.5,
                end: 2.4,
                breath: 0.5,
                climax: 1.6,
                intensity: 0.7,
            }],
            sections: vec![Section {
                kind: SectionKind::Verse,
                start: 0.0,
                end: 4.0,
                intensity: 0.6,
                variant: 0,
            }],
            beats: vec![],
        }
    }

    #[test]
    fn a_performance_round_trips_through_json() {
        let p = tiny();
        let q = Performance::from_json(&p.to_json()).expect("parses");
        assert_eq!(p, q);
    }

    #[test]
    fn overlapping_syllables_are_refused_with_the_index() {
        let mut p = tiny();
        p.syllables[1].start = 1.2;
        let e = p.validate().unwrap_err();
        assert!(e.0.contains("syllable 1"), "{e}");
    }

    #[test]
    fn the_pitch_range_is_the_songs_own() {
        let p = tiny();
        let (lo, hi) = p.pitch_range();
        assert!(lo < 62.5 && hi > 66.5);
        assert!(p.pitch_norm(70.0) == 1.0 && p.pitch_norm(50.0) == 0.0);
    }

    #[test]
    fn values_that_would_poison_a_track_are_refused() {
        let mut cases: Vec<(&str, Performance)> = Vec::new();
        let mut p = tiny();
        p.syllables[0].pitch = f32::INFINITY;
        cases.push(("infinite pitch", p));
        let mut p = tiny();
        p.syllables[0].pitch = f32::NAN;
        cases.push(("NaN pitch", p));
        let mut p = tiny();
        p.syllables[1].pitch = 3.0e38;
        cases.push(("absurd pitch", p));
        let mut p = tiny();
        p.bpm = f32::INFINITY;
        cases.push(("infinite bpm", p));
        let mut p = tiny();
        p.bpm = f32::NAN;
        cases.push(("NaN bpm", p));
        let mut p = tiny();
        p.duration = 1.0e9;
        cases.push(("endless duration", p));
        let mut p = tiny();
        p.phrases[0].start = f32::NAN;
        cases.push(("NaN phrase start", p));
        let mut p = tiny();
        p.phrases[0].end = f32::NAN;
        cases.push(("NaN phrase end", p));
        let mut p = tiny();
        p.phrases[0].climax = f32::NAN;
        cases.push(("NaN climax", p));
        let mut p = tiny();
        p.sections[0].start = f32::NAN;
        cases.push(("NaN section start", p));
        let mut p = tiny();
        p.sections[0].end = f32::INFINITY;
        cases.push(("infinite section end", p));
        let mut p = tiny();
        p.beats.push(Beat {
            t: f32::NAN,
            strength: 1.0,
        });
        cases.push(("NaN beat", p));
        for (what, p) in cases {
            assert!(p.validate().is_err(), "{what} must be refused");
        }
    }

    #[test]
    fn a_twenty_minute_song_at_any_ordinary_tempo_validates() {
        for bpm in [0.0, 20.0, 120.0, 300.0] {
            let mut p = tiny();
            p.bpm = bpm;
            p.duration = 1200.0;
            p.sections[0].end = 1200.0;
            p.validate().unwrap_or_else(|e| panic!("bpm {bpm}: {e}"));
        }
    }
}
