//! The first front end: a [`sounds::chorale::Score`] part becomes a [`Performance`].
//!
//! A score is the *easy* input — pitch, onset, duration and loudness are exact and there is no
//! lyric — which is exactly why it is first: the generator and the player can be built, judged
//! and measured against ground truth before any audio analysis can add its own errors. The
//! structure it infers (phrases at rests, sections in groups of phrases) is heuristic and tagged
//! as such (`source`), and is the part an analysed song replaces with measured boundaries.
//!
//! What it does not know, and says so by what it writes: there are no consonants, so onsets are
//! guessed from what touches what (`Closure` on a re-attack of the same pitch or a hum, `Soft`
//! on a change of pitch, `None` after a breath), and stress is inferred from metre, pitch peaks
//! and length — all things an aligner and a beat tracker would measure for real.

use sounds::chorale::{Note, Part, Score, Vowel as SVowel};

use crate::perf::{Beat, Onset, Performance, Phrase, Section, SectionKind, Syllable, Vowel};

/// The audio engine releases the last 8% of a note so a repeated pitch re-articulates; the
/// description ends the sound there too, so the beak and the voice agree.
const SOUNDING: f32 = 0.92;

fn map_vowel(v: SVowel) -> Vowel {
    match v {
        SVowel::Ah => Vowel::Ah,
        SVowel::Eh => Vowel::Eh,
        SVowel::Ee => Vowel::Ee,
        SVowel::Oh => Vowel::Oh,
        SVowel::Oo => Vowel::Oo,
        SVowel::Mm => Vowel::Mm,
    }
}

/// Describe `part` of `score`.
pub fn from_score(score: &Score, part: Part) -> Performance {
    let spb = score.beat_s() as f32;
    // One mouth sings one line: where a part has notes on top of each other (a MIDI staff with a
    // chord in it), keep the first and let each note give way to the next that starts inside it.
    let mut notes: Vec<Note> = Vec::new();
    for n in score.line(part).copied() {
        match notes.last_mut() {
            Some(last) if n.start_beat <= last.start_beat + 1e-6 => continue,
            Some(last) if n.start_beat < last.end_beat() => {
                last.beats = n.start_beat - last.start_beat
            }
            _ => {}
        }
        notes.push(n);
    }
    let mut syllables = Vec::with_capacity(notes.len());
    let mut phrases: Vec<Phrase> = Vec::new();

    let mean_len = if notes.is_empty() {
        1.0
    } else {
        notes.iter().map(|n| n.beats as f32).sum::<f32>() / notes.len() as f32
    };
    let mut phrase_first = 0usize;
    for (i, n) in notes.iter().enumerate() {
        let start = n.start_beat as f32 * spb;
        let dur = n.beats as f32 * spb;
        let prev = i.checked_sub(1).map(|j| &notes[j]);
        let next = notes.get(i + 1);
        let gap_before = prev.map_or(f32::INFINITY, |p| (n.start_beat - p.end_beat()) as f32);
        let onset = match prev {
            Some(p) if gap_before < 0.5 => {
                if n.vowel == SVowel::Mm || p.midi == n.midi {
                    Onset::Closure
                } else {
                    Onset::Soft
                }
            }
            _ => Onset::None,
        };
        let metric: f32 = {
            let pos = (n.start_beat % 4.0) as f32;
            if pos.fract().abs() > 1e-3 {
                0.1
            } else if pos < 0.01 {
                1.0
            } else if (pos - 2.0).abs() < 0.01 {
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
        syllables.push(Syllable {
            start,
            end: start + SOUNDING * dur,
            vowel: map_vowel(n.vowel),
            onset,
            pitch: f32::from(n.midi),
            level: n.level as f32,
            accent: (0.45f32 * metric + 0.35 * peak + 0.2 * long).min(1.0),
            confidence: 1.0,
            tie: false,
        });
        // A phrase ends at a breath, or at a bar line once it has run long.
        let last = i + 1 == notes.len();
        let rest_after = next.map_or(f32::INFINITY, |q| (q.start_beat - n.end_beat()) as f32);
        let run = (n.end_beat() - notes[phrase_first].start_beat) as f32;
        let bar_next = next.is_some_and(|q| (q.start_beat % 4.0).abs() < 1e-3);
        if last || rest_after >= 0.5 || (run >= 8.0 && bar_next) {
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
        .map(|(k, t)| Beat {
            t,
            strength: match k % 4 {
                0 => 1.0,
                2 => 0.6,
                _ => 0.4,
            },
        })
        .collect();

    let mut perf = Performance {
        name: format!("{}-{}", score.name, part.as_str()),
        bpm: score.bpm as f32,
        source: format!(
            "score:{}/{} (structure inferred)",
            score.name,
            part.as_str()
        ),
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

    // Sections: groups of four phrases, labelled by how big they are against the rest.
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
        perf.sections.push(Section {
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
    for (gi, g) in groups.drain(..).enumerate() {
        let chorus = g.2 >= cut && n_groups > 2;
        let variant = if chorus { &mut choruses } else { &mut verses };
        perf.sections.push(Section {
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
        let _ = gi;
    }
    // Close the gaps between sections so every instant has one; the last runs to the end.
    let mut secs = std::mem::take(&mut perf.sections);
    secs.sort_by(|a, b| a.start.partial_cmp(&b.start).expect("finite"));
    for i in 0..secs.len() {
        secs[i].end = if i + 1 < secs.len() {
            secs[i + 1].start
        } else {
            perf.duration
        };
    }
    if let Some(last) = secs.last_mut()
        && perf.duration - perf.phrases.last().map_or(0.0, |p| p.end) > 2.0
        && last.kind != SectionKind::Intro
    {
        let end = perf.phrases.last().expect("has phrases").end + 0.5;
        let tail = last.end;
        last.end = end;
        secs.push(Section {
            kind: SectionKind::Outro,
            start: end,
            end: tail,
            intensity: 0.3,
            variant: 0,
        });
    }
    perf.sections = secs;
    perf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_piece_becomes_a_valid_performance() {
        for score in [Score::wistful(), Score::duck_strut(), Score::outer_wilds()] {
            for part in score.parts() {
                let p = from_score(&score, part);
                p.validate()
                    .unwrap_or_else(|e| panic!("{} {}: {e}", score.name, part.as_str()));
                assert!(!p.syllables.is_empty());
                assert!(!p.phrases.is_empty(), "{} {}", score.name, part.as_str());
                assert!(p.sections.iter().all(|s| s.end > s.start));
            }
        }
    }
}
