//! The duck track: the sung notes. Everything the duck does with its beak, its head and — in the
//! second version — its own voice follows from this one list.

use serde::{Deserialize, Serialize};
use singer::{EndRule, NoteIn, Options, Performance, SectionHint, SectionKind, Vowel};

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
            if e.start_beat < prev_end - 1e-6 {
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
        let end = self
            .events
            .iter()
            .map(|e| e.start_beat + e.duration_beats)
            .fold(0.0, f64::max);
        (end * 60_000.0 / f64::from(self.tempo_bpm)).ceil() as u64
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
        let notes: Vec<NoteIn> = sorted
            .iter()
            .map(|e| NoteIn {
                start_beat: e.start_beat,
                beats: e.duration_beats,
                midi: e.midi_note,
                level: e.volume,
                vowel: e.voice.into(),
                tie: e.continues_previous,
            })
            .collect();
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
