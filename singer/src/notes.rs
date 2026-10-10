//! The shared front end: a sequence of sung notes becomes a [`Performance`].
//!
//! Two callers feed it — a [`sounds::chorale::Score`] part ([`crate::from_score`]) and the note
//! track of a Kivi Studio show (the `show` crate) — so the inference of phrases, stress, onsets
//! and structure lives in exactly one place. A note here is what both sources know for certain:
//! when, how long, which pitch, how loud, which vowel, and whether it continues the previous one.
//!
//! What it does not know, and says so by what it writes: there are no consonants, so onsets are
//! guessed from what touches what (`Closure` on a re-attack of the same pitch or a hum, `Soft`
//! on a change of pitch, `None` after a breath), and stress is inferred from metre, pitch peaks
//! and length — all things an aligner and a beat tracker would measure for real.

use crate::perf::{Beat, Onset, Performance, Phrase, Section, SectionKind, Syllable, Vowel};

/// One sung note, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteIn {
    pub start_beat: f64,
    pub beats: f64,
    pub midi: u8,
    /// 0..1.
    pub level: f32,
    pub vowel: Vowel,
    /// This note continues the previous syllable's vowel (一字多音). On the *same* pitch and
    /// touching, it is simply the previous note held longer.
    pub tie: bool,
    /// `beats` is exactly how long the note is audible — the source measured it (Kivi Studio's
    /// render states its audible spans), so no end rule is applied.
    pub exact: bool,
    /// What the source knows about the consonant before this vowel (from the lyric); `None` lets
    /// the front end guess from what touches what.
    pub onset: Option<Onset>,
}

/// How long a note is *audible* inside its written slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EndRule {
    /// The audio engine releases the last 8% of a note so a repeated pitch re-articulates; the
    /// description ends the sound there too, so the beak and the voice agree.
    Release(f32),
    /// Kivi Studio's render: the 8% release, and in addition a silent gap at the end of every
    /// note that is directly followed by another that is *not* tied to it
    /// (`applyDefaultArticulationGaps` in the app: 80–200 ms, at most 60% of the slot). The beak
    /// has to follow what is actually heard.
    Articulated,
}

/// A structure the source states outright, in beats. Used instead of the heuristic when present.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionHint {
    pub kind: SectionKind,
    pub start_beat: f64,
    pub end_beat: f64,
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    pub source: String,
    pub bpm: f32,
    pub beats_per_bar: f64,
    pub end_rule: EndRule,
    pub sections: Vec<SectionHint>,
}

impl Options {
    pub fn new(name: impl Into<String>, source: impl Into<String>, bpm: f32) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            bpm,
            beats_per_bar: 4.0,
            end_rule: EndRule::Release(0.92),
            sections: Vec::new(),
        }
    }
}

/// The audible gap Kivi Studio cuts into a note followed by a separately attacked one.
fn studio_gap_s(slot_s: f32) -> f32 {
    (slot_s * 0.55).clamp(0.08, 0.2).min(slot_s * 0.6)
}

/// Merge same-pitch tied continuations into the note they continue.
fn merge_ties(notes: &[NoteIn]) -> Vec<NoteIn> {
    let mut out: Vec<NoteIn> = Vec::with_capacity(notes.len());
    for n in notes {
        match out.last_mut() {
            Some(last)
                if n.tie
                    && n.midi == last.midi
                    && (n.start_beat - (last.start_beat + last.beats)).abs() < 1e-6 =>
            {
                last.beats += n.beats;
            }
            _ => out.push(*n),
        }
    }
    out
}

/// Describe a sequence of notes (start-ordered, non-overlapping).
pub fn from_notes(notes: &[NoteIn], opts: &Options) -> Performance {
    let notes = merge_ties(notes);
    let spb = 60.0 / opts.bpm.max(1.0);
    let bar = opts.beats_per_bar.max(1.0);
    let mut syllables = Vec::with_capacity(notes.len());
    let mut phrases: Vec<Phrase> = Vec::new();

    let mean_len = if notes.is_empty() {
        1.0
    } else {
        notes.iter().map(|n| n.beats as f32).sum::<f32>() / notes.len() as f32
    };
    let end_beat = |n: &NoteIn| n.start_beat + n.beats;
    let mut phrase_first = 0usize;
    for (i, n) in notes.iter().enumerate() {
        let start = n.start_beat as f32 * spb;
        let slot = n.beats as f32 * spb;
        let prev = i.checked_sub(1).map(|j| &notes[j]);
        let next = notes.get(i + 1);
        let gap_before = prev.map_or(f32::INFINITY, |p| (n.start_beat - end_beat(p)) as f32);
        let onset = if n.tie {
            Onset::None
        } else if let Some(stated) = n.onset {
            stated
        } else {
            match prev {
                Some(p) if gap_before < 0.5 => {
                    if n.vowel == Vowel::Mm || p.midi == n.midi {
                        Onset::Closure
                    } else {
                        Onset::Soft
                    }
                }
                _ => Onset::None,
            }
        };
        let metric: f32 = {
            let pos = n.start_beat % bar;
            if pos.fract().abs() > 1e-3 {
                0.1
            } else if pos < 0.01 {
                1.0
            } else if (bar % 2.0 < 1e-6) && (pos - bar / 2.0).abs() < 0.01 {
                0.7
            } else {
                0.5
            }
        };
        let peak: f32 = match (prev, next) {
            (Some(p), Some(q)) if n.midi > p.midi && n.midi > q.midi => 1.0,
            (None, Some(q)) if n.midi > q.midi => 1.0,
            _ => 0.0,
        };
        let long: f32 = if n.beats as f32 >= 1.5 * mean_len {
            1.0
        } else {
            0.0
        };
        let sounding = match opts.end_rule {
            _ if n.exact => slot,
            EndRule::Release(f) => f * slot,
            EndRule::Articulated => {
                let adjacent = next.is_some_and(|q| q.start_beat <= end_beat(n) + 1e-6);
                let release = 0.92 * slot;
                if adjacent {
                    release.min(slot - studio_gap_s(slot))
                } else {
                    release
                }
            }
        };
        syllables.push(Syllable {
            start,
            end: start + sounding.max(0.02),
            vowel: n.vowel,
            onset,
            pitch: f32::from(n.midi),
            level: n.level,
            accent: (0.45f32 * metric + 0.35 * peak + 0.2 * long).min(1.0),
            confidence: 1.0,
            tie: n.tie,
        });
        // A phrase ends at a breath, or at a bar line once it has run long.
        let last = i + 1 == notes.len();
        let rest_after = next.map_or(f32::INFINITY, |q| (q.start_beat - end_beat(n)) as f32);
        let run = (end_beat(n) - notes[phrase_first].start_beat) as f32;
        let bar_next = next.is_some_and(|q| (q.start_beat % bar).abs() < 1e-3);
        if last || rest_after >= 0.5 || (run >= 2.0 * bar as f32 && bar_next) {
            let first = &syllables[phrase_first];
            let end = syllables[i].end;
            let climax = syllables[phrase_first..=i]
                .iter()
                .fold((first.start, f32::MIN), |acc, s| {
                    if s.pitch > acc.1 {
                        (s.start, s.pitch)
                    } else {
                        acc
                    }
                })
                .0;
            let breath = phrases
                .last()
                .map_or(first.start, |p: &Phrase| (first.start - p.end).max(0.0));
            phrases.push(Phrase {
                start: first.start,
                end,
                breath: breath.max(0.0),
                climax,
                intensity: 0.0,
            });
            phrase_first = i + 1;
        }
    }

    let duration = syllables.last().map_or(1.0, |s| s.end) + 1.0;
    let beats: Vec<Beat> = (0..)
        .map(|k| (k, k as f32 * spb))
        .take_while(|&(_, t)| t < duration)
        .map(|(k, t)| {
            let pos = f64::from(k) % bar;
            let strength = if pos < 0.5 {
                1.0
            } else if (bar % 2.0 < 1e-6) && (pos - bar / 2.0).abs() < 0.5 {
                0.6
            } else {
                0.4
            };
            Beat { t, strength }
        })
        .collect();

    let mut perf = Performance {
        name: opts.name.clone(),
        bpm: opts.bpm,
        source: opts.source.clone(),
        duration,
        syllables,
        phrases,
        sections: Vec::new(),
        beats,
    };

    // Phrase intensity: how loud, and how high for this song.
    for i in 0..perf.phrases.len() {
        let p = perf.phrases[i];
        let inside: Vec<&Syllable> = perf
            .syllables
            .iter()
            .filter(|s| s.start >= p.start - 1e-3 && s.end <= p.end + 1e-3)
            .collect();
        if inside.is_empty() {
            continue;
        }
        let level = inside.iter().map(|s| s.level).sum::<f32>() / inside.len() as f32;
        let top = inside
            .iter()
            .map(|s| perf.pitch_norm(s.pitch))
            .fold(0.0f32, f32::max);
        perf.phrases[i].intensity = (level * (0.7 + 0.3 * top)).clamp(0.0, 1.0);
    }

    perf.sections = if opts.sections.is_empty() {
        infer_sections(&perf)
    } else {
        stated_sections(&perf, &opts.sections, spb)
    };
    perf
}

/// Sections the source stated: converted to seconds, ordered, tiling the song, with a repeat
/// index per kind.
fn stated_sections(perf: &Performance, hints: &[SectionHint], spb: f32) -> Vec<Section> {
    let mut sorted = hints.to_vec();
    sorted.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
    let mut counts: std::collections::HashMap<u8, u32> = std::collections::HashMap::new();
    let mut secs: Vec<Section> = sorted
        .iter()
        .map(|h| {
            let n = counts.entry(h.kind as u8).or_insert(0);
            let variant = *n;
            *n += 1;
            Section {
                kind: h.kind,
                start: h.start_beat as f32 * spb,
                end: h.end_beat as f32 * spb,
                intensity: h.intensity,
                variant,
            }
        })
        .collect();
    if let Some(first) = secs.first_mut() {
        first.start = 0.0;
    }
    for i in 0..secs.len() {
        secs[i].end = if i + 1 < secs.len() {
            secs[i + 1].start
        } else {
            perf.duration
        };
    }
    secs
}

/// Sections guessed from the phrases: groups of four, labelled by how big they are against the
/// rest.
fn infer_sections(perf: &Performance) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut groups: Vec<(f32, f32, f32)> = perf
        .phrases
        .chunks(4)
        .map(|c| {
            (
                c[0].start,
                c[c.len() - 1].end,
                c.iter().map(|p| p.intensity).sum::<f32>() / c.len() as f32,
            )
        })
        .collect();
    if let Some(first) = perf.phrases.first().filter(|p| p.start > 1.0) {
        out.push(Section {
            kind: SectionKind::Intro,
            start: 0.0,
            end: first.start,
            intensity: 0.3,
            variant: 0,
        });
    }
    let mut ranked: Vec<f32> = groups.iter().map(|g| g.2).collect();
    ranked.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let cut = ranked
        .get(ranked.len() * 2 / 3)
        .copied()
        .unwrap_or(f32::MAX);
    let (mut verses, mut choruses) = (0u32, 0u32);
    let n_groups = groups.len();
    for g in groups.drain(..) {
        let chorus = g.2 >= cut && n_groups > 2;
        let variant = if chorus { &mut choruses } else { &mut verses };
        out.push(Section {
            kind: if chorus {
                SectionKind::Chorus
            } else {
                SectionKind::Verse
            },
            start: g.0,
            end: g.1,
            intensity: g.2,
            variant: *variant,
        });
        *variant += 1;
    }
    // Close the gaps between sections so every instant has one; the last runs to the end.
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).expect("finite"));
    for i in 0..out.len() {
        out[i].end = if i + 1 < out.len() {
            out[i + 1].start
        } else {
            perf.duration
        };
    }
    if let Some(last) = out.last_mut()
        && perf.duration - perf.phrases.last().map_or(0.0, |p| p.end) > 2.0
        && last.kind != SectionKind::Intro
    {
        let end = perf.phrases.last().expect("has phrases").end + 0.5;
        let tail = last.end;
        last.end = end;
        out.push(Section {
            kind: SectionKind::Outro,
            start: end,
            end: tail,
            intensity: 0.3,
            variant: 0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: f64, beats: f64, midi: u8, tie: bool) -> NoteIn {
        NoteIn {
            start_beat: start,
            beats,
            midi,
            level: 0.8,
            vowel: Vowel::Ah,
            tie,
            exact: false,
            onset: None,
        }
    }

    #[test]
    fn a_tied_continuation_on_the_same_pitch_is_one_longer_note() {
        let p = from_notes(
            &[n(0.0, 1.0, 60, false), n(1.0, 2.0, 60, true)],
            &Options::new("t", "test", 120.0),
        );
        assert_eq!(p.syllables.len(), 1);
        assert!((p.syllables[0].end - 1.38).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn a_melisma_on_a_new_pitch_is_a_new_note_that_does_not_close() {
        let p = from_notes(
            &[n(0.0, 1.0, 60, false), n(1.0, 1.0, 64, true)],
            &Options::new("t", "test", 120.0),
        );
        assert_eq!(p.syllables.len(), 2);
        assert!(p.syllables[1].tie && p.syllables[1].onset == Onset::None);
    }

    /// The beak must follow what is heard: Studio cuts a silence into the end of a note followed
    /// by a separately attacked one, and not into one that is tied or followed by a rest.
    #[test]
    fn the_studio_rule_ends_a_touching_note_early_and_a_note_before_a_rest_normally() {
        let mut o = Options::new("t", "test", 60.0);
        o.end_rule = EndRule::Articulated;
        let p = from_notes(&[n(0.0, 2.0, 60, false), n(2.0, 1.0, 62, false)], &o);
        // slot 2 s: gap = min(0.2, max(0.08, 1.1)) = 0.2 → ends at 1.8, earlier than 0.92·2 = 1.84.
        assert!(
            (p.syllables[0].end - 1.8).abs() < 1e-3,
            "{}",
            p.syllables[0].end
        );
        let q = from_notes(&[n(0.0, 2.0, 60, false), n(4.0, 1.0, 62, false)], &o);
        assert!((q.syllables[0].end - 1.84).abs() < 1e-3);
    }

    #[test]
    fn stated_sections_win_and_count_their_repeats() {
        let mut o = Options::new("t", "test", 120.0);
        o.sections = vec![
            SectionHint {
                kind: SectionKind::Verse,
                start_beat: 0.0,
                end_beat: 8.0,
                intensity: 0.4,
            },
            SectionHint {
                kind: SectionKind::Chorus,
                start_beat: 8.0,
                end_beat: 16.0,
                intensity: 0.9,
            },
            SectionHint {
                kind: SectionKind::Chorus,
                start_beat: 16.0,
                end_beat: 24.0,
                intensity: 1.0,
            },
        ];
        let notes: Vec<NoteIn> = (0..24)
            .map(|i| n(f64::from(i), 0.9, 60 + (i % 5) as u8, false))
            .collect();
        let p = from_notes(&notes, &o);
        assert_eq!(p.sections.len(), 3);
        assert_eq!((p.sections[1].variant, p.sections[2].variant), (0, 1));
        p.validate().expect("valid");
    }
}
