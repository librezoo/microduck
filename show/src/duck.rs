//! The duck track: the sung notes. Everything the duck does with its beak, its head and — in the
//! second version — its own voice follows from this one list.

use serde::{Deserialize, Serialize};
use singer::{EndRule, NoteIn, Onset, Options, Performance, SectionHint, SectionKind, Vowel};

use crate::error::{Problem, Problems};

pub const SCHEMA: &str = "kivi.duck/1";

/// The six voices of the official synthesiser — the same six `singer::Vowel` has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Voice {
    Ah,
    Eh,
    Ee,
    Oh,
    Oo,
    Mm,
}

impl From<Voice> for Vowel {
    fn from(v: Voice) -> Self {
        match v {
            Voice::Ah => Vowel::Ah,
            Voice::Eh => Vowel::Eh,
            Voice::Ee => Vowel::Ee,
            Voice::Oh => Vowel::Oh,
            Voice::Oo => Vowel::Oo,
            Voice::Mm => Vowel::Mm,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEvent {
    pub event_id: String,
    pub start_beat: f64,
    pub duration_beats: f64,
    pub voice: Voice,
    pub midi_note: u8,
    pub volume: f32,
    #[serde(default)]
    pub continues_previous: bool,
    /// When the note is really audible, `[start_ms, end_ms]` on the show clock. The app states it
    /// from the very render it mixes into the audio (performed timing, breaths inside a note,
    /// the gap cut before a re-attack), so the beak opens and closes with what is heard instead
    /// of with the written grid. Absent: the robot infers it from the written note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounding: Vec<[f64; 2]>,
    /// What the lyric says precedes the vowel: a lip consonant shuts the beak, others narrow it,
    /// a vowel-initial syllable hardly moves it. The app derives it from the pinyin initial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onset: Option<OnsetHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnsetHint {
    Closure,
    Soft,
    None,
}

impl From<OnsetHint> for Onset {
    fn from(o: OnsetHint) -> Self {
        match o {
            OnsetHint::Closure => Onset::Closure,
            OnsetHint::Soft => Onset::Soft,
            OnsetHint::None => Onset::None,
        }
    }
}

/// Structure the app states outright. Optional: without it the robot infers sections from the
/// phrases, which is a guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionEvent {
    pub kind: SectionName,
    pub start_beat: f64,
    pub end_beat: f64,
    pub intensity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionName {
    Intro,
    Verse,
    Chorus,
    Bridge,
    Interlude,
    Outro,
}

impl From<SectionName> for SectionKind {
    fn from(s: SectionName) -> Self {
        match s {
            SectionName::Intro => SectionKind::Intro,
            SectionName::Verse => SectionKind::Verse,
            SectionName::Chorus => SectionKind::Chorus,
            SectionName::Bridge => SectionKind::Bridge,
            SectionName::Interlude => SectionKind::Interlude,
            SectionName::Outro => SectionKind::Outro,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuckScore {
    pub schema: String,
    pub tempo_bpm: f32,
    #[serde(default = "four")]
    pub beats_per_bar: u8,
    pub events: Vec<NoteEvent>,
    #[serde(default)]
    pub sections: Vec<SectionEvent>,
}

fn four() -> u8 {
    4
}

impl DuckScore {
    pub fn parse(bytes: &[u8]) -> Result<Self, Problems> {
        let s: DuckScore = serde_json::from_slice(bytes).map_err(|e| {
            let mut p = Problems::default();
            p.push(Problem::new("Incompatible", format!("duck track: {e}")));
            p
        })?;
        s.validate(usize::MAX)?;
        Ok(s)
    }

    /// Notes are start-ordered or orderable, do not overlap, and stay inside their ranges.
    pub fn validate(&self, max_events: usize) -> Result<(), Problems> {
        let mut p = Problems::default();
        if self.schema != SCHEMA {
            p.push(Problem::new(
                "Incompatible",
                format!(
                    "duck track schema {:?}, this robot reads {SCHEMA:?}",
                    self.schema
                ),
            ));
        }
        if !(20.0..=300.0).contains(&self.tempo_bpm) {
            p.push(Problem::new(
                "Corrupt",
                "duck track: tempoBpm outside 20..300",
            ));
        }
        if !(1..=16).contains(&self.beats_per_bar) {
            p.push(Problem::new(
                "Corrupt",
                "duck track: beatsPerBar outside 1..16",
            ));
        }
        if self.events.len() > max_events {
            p.push(Problem::new(
                "TooLarge",
                format!("{} notes; this robot takes {max_events}", self.events.len()),
            ));
        }
        let mut sorted: Vec<&NoteEvent> = self.events.iter().collect();
        sorted.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        let mut prev_end = 0.0f64;
        for e in &sorted {
            if !e.start_beat.is_finite()
                || e.start_beat < 0.0
                || e.duration_beats.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
            {
                p.push(
                    Problem::new("Corrupt", "a note has a bad start or duration").at(&e.event_id),
                );
            }
            if !(21..=108).contains(&e.midi_note) {
                p.push(Problem::new("OutOfRange", "midiNote outside 21..108").at(&e.event_id));
            }
            if !(0.0..=1.0).contains(&e.volume) {
                p.push(Problem::new("OutOfRange", "volume outside 0..1").at(&e.event_id));
            }
            let mut last_end = 0.0f64;
            for [a, b] in &e.sounding {
                if !a.is_finite() || !b.is_finite() || *a < last_end - 1e-6 || *b <= *a {
                    p.push(
                        Problem::new("Corrupt", "a note's sounding spans are not ordered or not positive")
                            .at(&e.event_id),
                    );
                    break;
                }
                last_end = *b;
            }
            if e.sounding.len() > 64 {
                p.push(Problem::new("TooLarge", "a note has more than 64 sounding spans").at(&e.event_id));
            }
            if e.sounding.is_empty() && e.start_beat < prev_end - 1e-6 {
                p.push(
                    Problem::new("Overlap", "a note starts before the previous one ends")
                        .at(&e.event_id),
                );
            }
            prev_end = prev_end.max(e.start_beat + e.duration_beats);
        }
        for s in &self.sections {
            if s.end_beat.partial_cmp(&s.start_beat) != Some(std::cmp::Ordering::Greater)
                || s.start_beat < 0.0
            {
                p.push(Problem::new("Corrupt", "a section has a bad range"));
            }
        }
        p.into_result()
    }

    /// Where the last note ends, in milliseconds on the show clock.
    pub fn end_ms(&self) -> u64 {
        let written = self
            .events
            .iter()
            .map(|e| e.start_beat + e.duration_beats)
            .fold(0.0, f64::max)
            * 60_000.0
            / f64::from(self.tempo_bpm);
        let heard = self
            .events
            .iter()
            .flat_map(|e| e.sounding.iter().map(|s| s[1]))
            .fold(0.0, f64::max);
        written.max(heard).ceil() as u64
    }

    /// The performance this score describes.
    ///
    /// The ends of the notes follow Studio's own render (`EndRule::Articulated`): the beak has to
    /// agree with what the mixed audio actually sounds like.
    pub fn to_performance(&self, name: &str) -> Result<Performance, Problems> {
        self.to_performance_for(name, 0.0)
    }

    /// As [`Self::to_performance`], but the performance lasts at least `min_duration_s`: a show
    /// whose audio outlasts its last note must not be cut off when the singing is over.
    pub fn to_performance_for(
        &self,
        name: &str,
        min_duration_s: f32,
    ) -> Result<Performance, Problems> {
        self.validate(usize::MAX)?;
        let mut sorted: Vec<&NoteEvent> = self.events.iter().collect();
        sorted.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        let ms_to_beats = f64::from(self.tempo_bpm) / 60_000.0;
        let mut notes: Vec<NoteIn> = Vec::with_capacity(sorted.len());
        for e in &sorted {
            let note = |start_beat: f64, beats: f64, tie: bool, exact: bool| NoteIn {
                start_beat,
                beats,
                midi: e.midi_note,
                level: e.volume,
                vowel: e.voice.into(),
                tie,
                exact,
                onset: e.onset.map(Onset::from),
            };
            if e.sounding.is_empty() {
                notes.push(note(e.start_beat, e.duration_beats, e.continues_previous, false));
            } else {
                // One sung note can be several audible islands (a breath inside it); each is
                // its own stretch of beak, and the gap between them is a real closure.
                for (k, [a, b]) in e.sounding.iter().enumerate() {
                    notes.push(note(
                        a * ms_to_beats,
                        (b - a) * ms_to_beats,
                        e.continues_previous && k == 0,
                        true,
                    ));
                }
            }
        }
        // Performed spans can lean on a neighbour: what is heard never overlaps.
        notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        for i in 0..notes.len().saturating_sub(1) {
            let next_start = notes[i + 1].start_beat;
            if notes[i].start_beat + notes[i].beats > next_start {
                notes[i].beats = next_start - notes[i].start_beat;
            }
        }
        notes.retain(|n| n.beats > 1e-6);
        let mut opts = Options::new(name, format!("kivi.duck/1:{name}"), self.tempo_bpm);
        opts.beats_per_bar = f64::from(self.beats_per_bar);
        opts.end_rule = EndRule::Articulated;
        opts.sections = self
            .sections
            .iter()
            .map(|s| SectionHint {
                kind: s.kind.into(),
                start_beat: s.start_beat,
                end_beat: s.end_beat,
                intensity: s.intensity,
            })
            .collect();
        if notes.is_empty() {
            let mut p = Problems::default();
            p.push(Problem::new("Corrupt", "the duck track has no notes"));
            return Err(p);
        }
        let mut perf = singer::from_notes(&notes, &opts);
        if min_duration_s > perf.duration {
            perf.duration = min_duration_s;
            if let Some(last) = perf.sections.last_mut() {
                last.end = last.end.max(min_duration_s);
            }
        }
        perf.validate().map_err(|e| {
            let mut p = Problems::default();
            p.push(Problem::new("Corrupt", e.to_string()));
            p
        })?;
        Ok(perf)
    }
}
