//! How a song is performed: the structural choices that make a ballad look different from a
//! chant, kept apart from the amplitudes of a particular duck.
//!
//! **A style is not a gain.** What differs between them is *which gestures exist and how they
//! are timed*: whether a ballad's beak runs legato through a phrase while a chant's snaps shut
//! on every syllable; whether the head moves on phrases or on beats; whether a long note gets a
//! slow sway; how much of the motion happens in the breath before a phrase. A single global
//! amplitude knob (`Style::scaled`) exists for fitting a duck, and is deliberately not how the
//! presets differ.
//!
//! The duck's own character is the constant under all of them: it nods more than it rolls, it
//! keeps its heading on the audience (small yaw), and its beak is crisp rather than rubbery.
//! That character lives in the presets' *ratios*, and `character_holds` in the tests pins it.
//!
//! The presets are engineering hypotheses informed by the literature summarised in the design
//! document — they are *not* measurements of how people perform and they carry no claim to
//! universality. `Style::auto` picks one from the song's own tempo and syllable density, and a
//! person can override it.

use serde::{Deserialize, Serialize};

use crate::perf::Performance;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MouthStyle {
    /// Overall opening scale, 0..~1.2.
    pub gain: f32,
    /// How much a low note narrows the opening (0 = pitch does nothing). Hypothesis H1: opening
    /// rises with pitch in singers; the size of the effect is individual, hence a parameter.
    pub pitch_gain: f32,
    pub level_gain: f32,
    /// How far a high, loud note pulls a *narrow* vowel toward open (0 = never). Sundberg & Skoog
    /// (1997) found jaw opening widening with pitch for every vowel at the top of a singer's range,
    /// not just for /a/; without this the climax of a song sung on "ee" is a narrower beak than its
    /// quiet opening, which is backwards. Scales with pitch² and loudness.
    pub climax_open: f32,
    /// Smallest opening on any non-hum syllable: a sung vowel never looks closed.
    pub floor_open: f32,
    /// How deep the dip between two touching syllables goes, as a fraction of the lower
    /// opening. Ballad: shallow (legato). Chant: deep.
    pub legato_dip: f32,
    /// A gap at least this long closes the beak fully, seconds.
    pub closure_gap: f32,
    /// How much earlier than the sound the jaw starts to move, seconds. A style choice about
    /// *looking* prepared; mechanical latency is `Profile::mouth_latency`, handled elsewhere.
    pub anticipation: f32,
    /// How much a step in pitch changes the opening: a rise of eight semitones (or more) opens the
    /// beak by this much more, a fall closes it by as much, within a phrase. This is what makes a
    /// run or a slide *look* like one; the pitch-normalised term alone moves the beak by only a few
    /// percent for a step of a tone or two. 0 = off.
    #[serde(default)]
    pub interval_gain: f32,
    /// How much a held note relaxes after the attack, fraction of its opening.
    pub settle: f32,
    /// Amplitude of the slow breathing drift on held notes, fraction of the opening. Keeps a
    /// 4-second note from being a frozen pose without being a wobble.
    pub drift: f32,
    /// A gap this long (seconds) is a breath, and the beak cues it.
    pub breath_min: f32,
    /// How far the beak parts to cue an inhale, 0..1.
    pub breath_open: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HeadStyle {
    /// Phrase arc: lift to the climax, radians.
    pub arc_pitch: f32,
    /// Yaw offset between successive phrases (a scan of the audience), radians.
    pub arc_yaw: f32,
    pub arc_roll: f32,
    /// Inhale lift before a phrase, radians.
    pub prep: f32,
    /// The small dip as a phrase starts, radians.
    pub onset_nod: f32,
    /// Nod on the beat, radians. The only layer that runs on the beat grid.
    pub beat_nod: f32,
    pub accent: f32,
    /// Roll sway on notes held longer than `sway_after` seconds, radians.
    pub sway_roll: f32,
    pub sway_yaw: f32,
    pub sway_period: f32,
    pub sway_after: f32,
    /// Slow drop at the very end of the song, radians.
    pub final_bow: f32,
    /// Fastest beat-nod rate, Hz. Above it the nod thins to downbeats.
    pub nod_rate_max: f32,
    /// How much nodding survives an instrumental passage (the duck "listening"), 0..1.
    pub listen: f32,
    /// Amplitude of the always-on slow drift that keeps a held head from freezing, radians.
    pub idle: f32,
    /// How far a section's overall intensity lifts or lowers the head, radians. The motion
    /// starts well *before* the section does (see `head::MOOD_LEAD`).
    pub mood_pitch: f32,
    /// The head follows the melody: radians of head pitch per octave the voice is above or below
    /// the phrase's own centre, lagging a little (a lifted head on a high note, a lowered one on a
    /// low note). 0 = off.
    #[serde(default)]
    pub follow: f32,
    /// A slow side-to-side groove locked to the bars, radians of yaw (and a little roll). Each
    /// bar's size and direction vary, so it is a dance and not a pendulum. 0 = off.
    #[serde(default)]
    pub swing: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    pub mouth: MouthStyle,
    pub head: HeadStyle,
    /// Seeds every deterministic variation. Derived from the performance name unless pinned.
    pub seed: u64,
}

impl Style {
    pub fn ballad() -> Self {
        Self {
            name: "ballad".into(),
            mouth: MouthStyle {
                gain: 1.0,
                pitch_gain: 0.30,
                level_gain: 0.25,
                climax_open: 0.50,
                floor_open: 0.22,
                legato_dip: 0.28,
                closure_gap: 0.10,
                anticipation: 0.05,
                interval_gain: 0.0,
                settle: 0.14,
                drift: 0.035,
                breath_min: 0.35,
                breath_open: 0.14,
            },
            head: HeadStyle {
                arc_pitch: 0.060,
                arc_yaw: 0.050,
                arc_roll: 0.035,
                prep: 0.035,
                onset_nod: 0.025,
                beat_nod: 0.008,
                accent: 0.025,
                sway_roll: 0.050,
                sway_yaw: 0.020,
                sway_period: 2.6,
                sway_after: 0.9,
                final_bow: 0.045,
                nod_rate_max: 1.2,
                listen: 0.7,
                idle: 0.008,
                mood_pitch: 0.025,
                follow: 0.0,
                swing: 0.0,
            },
            seed: 0,
        }
        .visible()
    }

    pub fn pop() -> Self {
        Self {
            name: "pop".into(),
            mouth: MouthStyle {
                gain: 1.0,
                pitch_gain: 0.25,
                level_gain: 0.25,
                climax_open: 0.40,
                floor_open: 0.22,
                legato_dip: 0.45,
                closure_gap: 0.07,
                anticipation: 0.04,
                interval_gain: 0.0,
                settle: 0.10,
                drift: 0.025,
                breath_min: 0.30,
                breath_open: 0.10,
            },
            head: HeadStyle {
                arc_pitch: 0.045,
                arc_yaw: 0.040,
                arc_roll: 0.030,
                prep: 0.022,
                onset_nod: 0.020,
                beat_nod: 0.030,
                accent: 0.035,
                sway_roll: 0.030,
                sway_yaw: 0.012,
                sway_period: 1.8,
                sway_after: 0.7,
                final_bow: 0.035,
                nod_rate_max: 2.2,
                listen: 0.9,
                idle: 0.007,
                mood_pitch: 0.020,
                follow: 0.0,
                swing: 0.0,
            },
            seed: 0,
        }
        .visible()
    }

    /// Big and playful: the beak opens wide and works on every syllable, the head follows the
    /// tune, nods hard on stress and sways side to side with the bars. Amplitudes are near what
    /// the head is allowed to do (see `HeadProfile::lively` for the matching budgets); it is for a
    /// duck that is sitting, and it trades the believable restraint of the other three for fun.
    pub fn lively() -> Self {
        Self {
            name: "lively".into(),
            mouth: MouthStyle {
                gain: 1.0,
                pitch_gain: 0.55,
                level_gain: 0.45,
                climax_open: 0.55,
                floor_open: 0.10,
                legato_dip: 0.70,
                closure_gap: 0.06,
                anticipation: 0.04,
                interval_gain: 0.28,
                settle: 0.28,
                drift: 0.05,
                breath_min: 0.30,
                breath_open: 0.16,
            },
            head: HeadStyle {
                arc_pitch: 0.15,
                arc_yaw: 0.30,
                arc_roll: 0.10,
                prep: 0.09,
                onset_nod: 0.08,
                beat_nod: 0.07,
                accent: 0.14,
                sway_roll: 0.14,
                sway_yaw: 0.20,
                sway_period: 2.2,
                sway_after: 0.5,
                final_bow: 0.14,
                nod_rate_max: 2.2,
                listen: 0.8,
                idle: 0.02,
                mood_pitch: 0.08,
                follow: 0.30,
                swing: 0.30,
            },
            seed: 0,
        }
    }

    /// Fast, clipped delivery: the beak snaps on every syllable, the head hits accents and the
    /// beat, and there is little room for sway.
    pub fn chant() -> Self {
        Self {
            name: "chant".into(),
            mouth: MouthStyle {
                gain: 0.9,
                pitch_gain: 0.15,
                level_gain: 0.30,
                climax_open: 0.25,
                floor_open: 0.18,
                legato_dip: 0.75,
                closure_gap: 0.05,
                anticipation: 0.03,
                interval_gain: 0.0,
                settle: 0.05,
                drift: 0.015,
                breath_min: 0.25,
                breath_open: 0.06,
            },
            head: HeadStyle {
                arc_pitch: 0.025,
                arc_yaw: 0.022,
                arc_roll: 0.020,
                prep: 0.010,
                onset_nod: 0.015,
                beat_nod: 0.035,
                accent: 0.050,
                sway_roll: 0.015,
                sway_yaw: 0.005,
                sway_period: 1.4,
                sway_after: 0.6,
                final_bow: 0.030,
                nod_rate_max: 2.5,
                listen: 1.0,
                idle: 0.005,
                mood_pitch: 0.015,
                follow: 0.0,
                swing: 0.0,
            },
            seed: 0,
        }
        .visible()
    }

    /// The amplitudes above are the *shape* of each style; this puts them at the scale the
    /// chorale's sway is already known to read at on the bench (roll 0.08 rad, lift 0.10 rad)
    /// rather than at a fraction of it. A starting point for calibration, not a measurement.
    fn visible(self) -> Self {
        self.scaled(1.0, 1.3)
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "ballad" => Some(Self::ballad()),
            "pop" => Some(Self::pop()),
            "chant" => Some(Self::chant()),
            "lively" => Some(Self::lively()),
            _ => None,
        }
    }

    /// Choose from the song itself: slow and sustained is a ballad, dense is a chant, the rest
    /// is pop. A heuristic with two thresholds, easy to override and easy to check.
    pub fn auto(perf: &Performance) -> Self {
        let density = perf.sung_density();
        let mut style = if density >= 3.2 {
            Self::chant()
        } else if perf.bpm < 96.0 && density < 2.0 {
            Self::ballad()
        } else {
            Self::pop()
        };
        style.seed = crate::dsp::seed_of(&perf.name);
        style
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Fit the whole show to a duck whose head is more or less tolerant. For *calibration*; the
    /// presets do not differ by this.
    pub fn scaled(mut self, mouth: f32, head: f32) -> Self {
        self.mouth.gain *= mouth;
        let h = &mut self.head;
        for v in [
            &mut h.arc_pitch,
            &mut h.arc_yaw,
            &mut h.arc_roll,
            &mut h.prep,
            &mut h.onset_nod,
            &mut h.beat_nod,
            &mut h.accent,
            &mut h.sway_roll,
            &mut h.sway_yaw,
            &mut h.final_bow,
            &mut h.idle,
            &mut h.mood_pitch,
            &mut h.follow,
            &mut h.swing,
        ] {
            *v *= head;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The duck is the constant under every style: it nods more than it rolls and keeps its
    /// eyes front. Without this a preset tweak can quietly turn the duck into a different
    /// animal.
    #[test]
    fn character_holds() {
        for s in [Style::ballad(), Style::pop(), Style::chant()] {
            let h = s.head;
            let nod = h.arc_pitch + h.prep + h.onset_nod + h.accent + h.beat_nod;
            let roll = h.arc_roll + h.sway_roll;
            assert!(nod >= roll, "{}: it nods more than it rolls", s.name);
            assert!(
                h.arc_yaw <= h.arc_pitch * 1.0 + 0.001,
                "{}: yaw stays small",
                s.name
            );
            assert!(
                s.mouth.floor_open >= 0.15,
                "{}: a sung vowel is never shut",
                s.name
            );
        }
    }

    /// Styles differ in structure, not in a gain: every pair differs in at least three
    /// independent parameters, in different directions.
    #[test]
    fn styles_differ_in_structure() {
        let (b, p, c) = (Style::ballad(), Style::pop(), Style::chant());
        assert!(b.mouth.legato_dip < p.mouth.legato_dip && p.mouth.legato_dip < c.mouth.legato_dip);
        assert!(b.head.sway_roll > c.head.sway_roll);
        assert!(b.head.beat_nod < p.head.beat_nod);
        assert!(b.head.sway_period > c.head.sway_period);
        assert!(b.head.listen < c.head.listen);
    }
}
