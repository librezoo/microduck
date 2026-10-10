//! The worked example of `docs/ideas/singing-performance.md` §10: a hand-annotated 12-second
//! excerpt with a breath, a melisma, a stressed word, a held note, a rest and an ending, taken
//! from the performance description to the numbers a joint would be sent.
//!
//!   cargo run -p singer --example walkthrough

use singer::{
    Beat, Mechanism, Onset, Performance, Phrase, Profile, Section, SectionKind, Show, Style,
    Syllable, Vowel,
};

#[allow(clippy::too_many_arguments)]
fn syl(
    word: &str,
    a: f32,
    b: f32,
    v: Vowel,
    on: Onset,
    pitch: f32,
    level: f32,
    accent: f32,
    tie: bool,
) -> (String, Syllable) {
    (
        word.to_owned(),
        Syllable {
            start: a,
            end: b,
            vowel: v,
            onset: on,
            pitch,
            level,
            accent,
            confidence: 1.0,
            tie,
        },
    )
}

fn main() {
    let words = vec![
        syl(
            "你  ni",
            2.40,
            2.85,
            Vowel::Ee,
            Onset::Soft,
            62.0,
            0.60,
            0.2,
            false,
        ),
        syl(
            "好  hao",
            2.95,
            3.50,
            Vowel::Ah,
            Onset::Soft,
            67.0,
            0.85,
            0.9,
            false,
        ),
        syl(
            "呀  ya",
            3.60,
            5.10,
            Vowel::Ah,
            Onset::None,
            67.0,
            0.80,
            0.3,
            false,
        ),
        syl(
            "(a)",
            5.10,
            5.80,
            Vowel::Ah,
            Onset::None,
            65.0,
            0.75,
            0.0,
            true,
        ),
        syl(
            "(a)",
            5.80,
            7.40,
            Vowel::Ah,
            Onset::None,
            62.0,
            0.70,
            0.0,
            true,
        ),
        syl(
            "再  zai",
            9.30,
            9.80,
            Vowel::Eh,
            Onset::Closure,
            64.0,
            0.70,
            0.6,
            false,
        ),
        syl(
            "见  jian",
            9.90,
            12.20,
            Vowel::Ee,
            Onset::Soft,
            72.0,
            1.00,
            0.8,
            false,
        ),
    ];
    let perf = Performance {
        name: "walkthrough".into(),
        bpm: 100.0,
        source: "hand-annotated".into(),
        duration: 14.0,
        syllables: words.iter().map(|w| w.1).collect(),
        phrases: vec![
            Phrase {
                start: 2.40,
                end: 7.40,
                breath: 2.40,
                climax: 2.95,
                intensity: 0.70,
            },
            Phrase {
                start: 9.30,
                end: 12.20,
                breath: 1.90,
                climax: 9.90,
                intensity: 1.00,
            },
        ],
        sections: vec![
            Section {
                kind: SectionKind::Verse,
                start: 0.0,
                end: 8.5,
                intensity: 0.45,
                variant: 0,
            },
            Section {
                kind: SectionKind::Chorus,
                start: 8.5,
                end: 14.0,
                intensity: 1.0,
                variant: 0,
            },
        ],
        beats: (0..24)
            .map(|k| Beat {
                t: k as f32 * 0.6,
                strength: match k % 4 {
                    0 => 1.0,
                    2 => 0.6,
                    _ => 0.4,
                },
            })
            .collect(),
    };
    perf.validate().expect("valid");
    let mut profile = Profile::default();
    profile.head.pitch_up_sign = 1.0; // print "up" as positive
    let style = Style::ballad().with_seed(7);
    let show = Show::compile(perf.clone(), style, profile).expect("compiles");
    let bad = Mechanism::pessimistic().run(&show.mouth, profile.rate);

    println!(
        "style {}  mouth stats {:?}",
        show.style.name, show.mouth_stats
    );
    println!("pitch range (p5..p95): {:?}", perf.pitch_range());
    println!("\nsyllable opening targets (vowel × pitch × level):");
    for (w, s) in &words {
        let k = ((s.start + 0.3) * 50.0) as usize;
        println!(
            "  {w:10} {:.2}-{:.2}s {:?} pitch {:.0} level {:.2} → beak {:.2} at +0.3 s",
            s.start,
            s.end,
            s.vowel,
            s.pitch,
            s.level,
            show.mouth[k.min(show.mouth.len() - 1)]
        );
    }
    println!(
        "\n   t(s)   what                         beak(designed) beak(pessimistic servo)   head: pitch   yaw   roll   [rad, + = up/left]"
    );
    let marks: &[(f32, &str)] = &[
        (0.5, "intro, listening"),
        (1.0, "intro"),
        (1.8, "before phrase 1 (inhale lift)"),
        (2.2, "just before 你"),
        (2.5, "你 (ee, narrow)"),
        (3.2, "好 (ah, accented, high)"),
        (3.7, "呀 attack"),
        (4.4, "呀 held"),
        (5.2, "melisma join (no dip)"),
        (6.5, "held low note"),
        (7.3, "end of note"),
        (7.8, "rest"),
        (8.6, "rest, chorus coming"),
        (9.0, "breath before phrase 2"),
        (9.4, "再 (eh)"),
        (10.0, "见 attack (climax)"),
        (11.0, "见 held"),
        (12.0, "end of note"),
        (12.6, "bow"),
        (13.4, "settled"),
    ];
    for (t, what) in marks {
        let f = show.sample(f64::from(*t));
        let k = ((t * 50.0) as usize).min(bad.len() - 1);
        println!(
            "  {t:5.1}   {what:28} {:6.2}          {:6.2}                    {:+.3} {:+.3} {:+.3}",
            f.mouth, bad[k], f.head[1], f.head[2], f.head[3]
        );
    }
    let r = singer::metrics::head_report(&show);
    println!(
        "\nhead peak {:?} (budget {:?})  max speed {:?} rad/s",
        r.peak.map(|v| (v * 1000.0).round() / 1000.0),
        profile.head.budget,
        r.max_vel.map(|v| (v * 100.0).round() / 100.0)
    );
}
