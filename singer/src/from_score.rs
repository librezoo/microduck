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

use crate::notes::{EndRule, NoteIn, Options, from_notes};
use crate::perf::{Performance, Vowel};

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
    let input: Vec<NoteIn> = notes
        .iter()
        .map(|n| NoteIn {
            start_beat: n.start_beat,
            beats: n.beats,
            midi: n.midi,
            level: n.level as f32,
            vowel: map_vowel(n.vowel),
            tie: false,
            exact: false,
            onset: None,
        })
        .collect();
    let mut opts = Options::new(
        format!("{}-{}", score.name, part.as_str()),
        format!(
            "score:{}/{} (structure inferred)",
            score.name,
            part.as_str()
        ),
        score.bpm as f32,
    );
    opts.end_rule = EndRule::Release(SOUNDING);
    from_notes(&input, &opts)
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
