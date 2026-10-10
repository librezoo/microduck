//! Behavioural tests: each one is a claim the design document makes, stated as a measurement on
//! generated motion. They use hand-built performances so that the thing under test is the only
//! thing that varies; the real pieces are exercised by `from_score`'s tests and the report.

use singer::metrics::{head_report, mouth_report};
use singer::{
    Beat, Clock, Frame, Onset, Performance, Phase, Phrase, Player, Profile, Section, SectionKind,
    Show, Style, Syllable, Vowel,
};

fn syl(start: f32, end: f32, vowel: Vowel, pitch: f32) -> Syllable {
    Syllable {
        start,
        end,
        vowel,
        onset: Onset::None,
        pitch,
        level: 0.7,
        accent: 0.0,
        confidence: 1.0,
        tie: false,
    }
}

fn perf(duration: f32, syllables: Vec<Syllable>) -> Performance {
    let (s, e) = (
        syllables.first().map_or(0.0, |s| s.start),
        syllables.last().map_or(1.0, |s| s.end),
    );
    Performance {
        name: "t".into(),
        bpm: 100.0,
        source: "test".into(),
        duration,
        syllables,
        phrases: vec![Phrase {
            start: s,
            end: e,
            breath: 1.0,
            climax: s,
            intensity: 0.7,
        }],
        sections: vec![Section {
            kind: SectionKind::Verse,
            start: 0.0,
            end: duration,
            intensity: 0.6,
            variant: 0,
        }],
        beats: vec![],
    }
}

/// A profile with the pitch sign made explicit so a test can say "up" and mean it.
fn profile() -> Profile {
    let mut p = Profile::default();
    p.head.pitch_up_sign = 1.0;
    p
}

fn compile(p: Performance, style: Style) -> Show {
    Show::compile(p, style, profile()).expect("compiles")
}

fn at(show: &Show, t: f32) -> f32 {
    show.sample(f64::from(t)).mouth
}

#[test]
fn a_held_note_stays_held_but_is_not_frozen() {
    let show = compile(
        perf(8.0, vec![syl(1.0, 5.0, Vowel::Ah, 64.0)]),
        Style::ballad(),
    );
    let window: Vec<f32> = (0..300).map(|k| at(&show, 1.6 + k as f32 * 0.01)).collect();
    let (lo, hi) = (
        window.iter().cloned().fold(f32::MAX, f32::min),
        window.iter().cloned().fold(f32::MIN, f32::max),
    );
    assert!(hi - lo <= 0.12, "held within ±0.06: {lo}..{hi}");
    let long: Vec<f32> = (0..300).map(|k| at(&show, 3.0 + k as f32 * 0.01)).collect();
    let span = long.iter().cloned().fold(f32::MIN, f32::max)
        - long.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        span > 0.004,
        "a four-second note is not a frozen pose: {span}"
    );
    assert!(at(&show, 6.5) < 0.05, "and the beak is shut after it");
}

#[test]
fn a_rest_is_closed_and_a_long_one_gets_a_breath() {
    let show = compile(
        perf(
            10.0,
            vec![
                syl(0.5, 1.5, Vowel::Ah, 60.0),
                syl(5.0, 6.0, Vowel::Ah, 60.0),
            ],
        ),
        Style::ballad(),
    );
    let mid: Vec<f32> = (0..200).map(|k| at(&show, 2.0 + k as f32 * 0.01)).collect();
    assert!(mid.iter().all(|v| *v < 0.03), "closed early in the rest");
    let breath = (0..40)
        .map(|k| at(&show, 4.2 + k as f32 * 0.01))
        .fold(0.0f32, f32::max);
    assert!(
        breath > 0.08 && breath < 0.25,
        "a small part to inhale, not a syllable: {breath}"
    );
    assert!(at(&show, 5.3) > 0.4, "and then the vowel");
}

#[test]
fn vowels_order_the_opening_and_the_hum_stays_closed() {
    let open = |v: Vowel| {
        let show = compile(perf(4.0, vec![syl(1.0, 2.5, v, 64.0)]), Style::pop());
        (0..100)
            .map(|k| at(&show, 1.2 + k as f32 * 0.01))
            .fold(0.0f32, f32::max)
    };
    let (ah, eh, oh, ee, oo, mm) = (
        open(Vowel::Ah),
        open(Vowel::Eh),
        open(Vowel::Oh),
        open(Vowel::Ee),
        open(Vowel::Oo),
        open(Vowel::Mm),
    );
    assert!(
        ah > eh && eh > oh && oh > ee && ee > oo,
        "{ah} {eh} {oh} {ee} {oo}"
    );
    assert!(mm < 0.12, "a hum is a closed beak: {mm}");
}

#[test]
fn a_higher_or_louder_note_opens_wider() {
    let song = |pitch: f32, level: f32| {
        let mut a = syl(1.0, 2.0, Vowel::Ah, 55.0);
        let mut b = syl(2.6, 3.6, Vowel::Ah, 75.0);
        a.level = 0.7;
        b.level = 0.7;
        let mut c = syl(4.2, 5.2, Vowel::Ah, pitch);
        c.level = level;
        let show = compile(perf(7.0, vec![a, b, c]), Style::pop());
        (0..100)
            .map(|k| at(&show, 4.3 + k as f32 * 0.01))
            .fold(0.0f32, f32::max)
    };
    assert!(song(75.0, 0.7) > song(55.0, 0.7) + 0.05, "pitch");
    assert!(song(65.0, 1.0) > song(65.0, 0.2) + 0.03, "loudness");
}

/// The mechanism's limits are respected on every sample, and fast lyrics get *smaller* strokes
/// rather than broken timing.
#[test]
fn dense_lyrics_degrade_into_smaller_strokes_within_the_limits() {
    let slow: Vec<Syllable> = (0..6)
        .map(|i| {
            syl(
                1.0 + i as f32 * 0.7,
                1.0 + i as f32 * 0.7 + 0.6,
                Vowel::Ah,
                64.0,
            )
        })
        .collect();
    let fast: Vec<Syllable> = (0..16)
        .map(|i| {
            syl(
                1.0 + i as f32 * 0.14,
                1.0 + i as f32 * 0.14 + 0.11,
                Vowel::Ah,
                64.0,
            )
        })
        .collect();
    let (s, f) = (
        compile(perf(7.0, slow), Style::chant()),
        compile(perf(5.0, fast), Style::chant()),
    );
    let depth = |show: &Show, a: f32, b: f32| {
        let v: Vec<f32> = (0..((b - a) * 100.0) as usize)
            .map(|k| at(show, a + k as f32 * 0.01))
            .collect();
        v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
    };
    let (ds, df) = (depth(&s, 2.0, 4.0), depth(&f, 1.6, 3.0));
    assert!(df < ds, "faster, shallower: {df} vs {ds}");
    assert!(df > 0.02, "but still moving: {df}");
    let rep = mouth_report(&f.perf, &f.mouth, 50.0, 0.0, 0.0);
    assert!(
        rep.max_vel <= f.profile.mouth.max_vel * 1.001,
        "{}",
        rep.max_vel
    );
    assert!(f.mouth_stats.depth_limited > 0, "and it says so");
}

#[test]
fn low_confidence_syllables_move_less() {
    let mut a = syl(1.0, 2.0, Vowel::Ah, 64.0);
    let sure = compile(perf(4.0, vec![a]), Style::pop());
    a.confidence = 0.1;
    let unsure = compile(perf(4.0, vec![a]), Style::pop());
    let peak = |s: &Show| {
        (0..100)
            .map(|k| at(s, 1.1 + k as f32 * 0.01))
            .fold(0.0f32, f32::max)
    };
    assert!(peak(&unsure) < peak(&sure) - 0.05);
}

#[test]
fn the_same_song_compiles_to_the_same_motion_and_a_different_seed_to_a_different_one() {
    let mk = || {
        let mut p = perf(
            20.0,
            (0..8)
                .map(|i| {
                    syl(
                        2.0 + i as f32 * 2.0,
                        3.5 + i as f32 * 2.0,
                        Vowel::Ah,
                        60.0 + i as f32,
                    )
                })
                .collect(),
        );
        p.beats = (0..40)
            .map(|k| Beat {
                t: k as f32 * 0.6,
                strength: if k % 4 == 0 { 1.0 } else { 0.4 },
            })
            .collect();
        p
    };
    let (a, b) = (
        compile(mk(), Style::pop().with_seed(1)),
        compile(mk(), Style::pop().with_seed(1)),
    );
    assert_eq!(a.head, b.head);
    assert_eq!(a.mouth, b.mouth);
    let c = compile(mk(), Style::pop().with_seed(2));
    assert_ne!(a.head, c.head, "the seed changes the choreography");
    let (ra, rc) = (head_report(&a), head_report(&c));
    assert!(
        (ra.path_per_min - rc.path_per_min).abs() < 0.35 * ra.path_per_min,
        "but not its character"
    );
}

#[test]
fn the_head_stays_inside_its_budget_and_its_speed_limit() {
    let mut p = perf(
        40.0,
        (0..20)
            .map(|i| {
                let mut s = syl(
                    2.0 + i as f32 * 1.8,
                    3.4 + i as f32 * 1.8,
                    Vowel::Ah,
                    55.0 + (i % 7) as f32 * 3.0,
                );
                s.accent = if i % 3 == 0 { 0.9 } else { 0.1 };
                s
            })
            .collect(),
    );
    p.beats = (0..70)
        .map(|k| Beat {
            t: k as f32 * 0.55,
            strength: if k % 4 == 0 { 1.0 } else { 0.5 },
        })
        .collect();
    p.phrases = (0..5)
        .map(|i| Phrase {
            start: 2.0 + i as f32 * 7.2,
            end: 2.0 + i as f32 * 7.2 + 6.4,
            breath: 0.8,
            climax: 4.0 + i as f32 * 7.2,
            intensity: 0.9,
        })
        .collect();
    let show = compile(p, Style::chant().scaled(1.0, 3.0));
    let r = head_report(&show);
    for j in 0..4 {
        assert!(
            r.budget_use[j] <= 1.0 + 1e-4,
            "joint {j}: {}",
            r.budget_use[j]
        );
        assert!(
            r.max_vel[j] <= show.profile.head.max_vel[j] * 1.001,
            "joint {j}: {}",
            r.max_vel[j]
        );
    }
}

/// The head announces a phrase: it lifts before the first syllable and dips as it starts.
#[test]
fn the_head_prepares_for_a_phrase() {
    let show = compile(
        perf(14.0, vec![syl(8.0, 10.0, Vowel::Ah, 64.0)]),
        Style::ballad(),
    );
    let pitch = |t: f32| show.sample(f64::from(t)).head[1];
    let rest = pitch(5.0);
    let lift = (0..40)
        .map(|k| pitch(7.55 + k as f32 * 0.01))
        .fold(f32::MIN, f32::max);
    assert!(
        lift > rest + 0.01,
        "lifted before the voice: {rest} → {lift}"
    );
}

/// A big section is anticipated: the head is already up before it starts (Livingstone & Palmer
/// found emotion-related head motion beginning seconds before the voice).
#[test]
fn a_section_is_anticipated() {
    let mut p = perf(30.0, vec![syl(12.0, 14.0, Vowel::Ah, 64.0)]);
    p.sections = vec![
        Section {
            kind: SectionKind::Verse,
            start: 0.0,
            end: 12.0,
            intensity: 0.2,
            variant: 0,
        },
        Section {
            kind: SectionKind::Chorus,
            start: 12.0,
            end: 30.0,
            intensity: 1.0,
            variant: 0,
        },
    ];
    p.phrases = vec![Phrase {
        start: 12.0,
        end: 14.0,
        breath: 2.0,
        climax: 12.5,
        intensity: 0.0,
    }];
    let show = compile(p, Style::ballad());
    let pitch = |t: f32| show.sample(f64::from(t)).head[1];
    assert!(
        pitch(11.7) > pitch(8.0) + 0.01,
        "already rising {} vs {}",
        pitch(11.7),
        pitch(8.0)
    );
}

#[test]
fn the_idle_drift_keeps_a_long_held_head_from_freezing() {
    let show = compile(
        perf(30.0, vec![syl(2.0, 28.0, Vowel::Ah, 64.0)]),
        Style::ballad(),
    );
    let r = head_report(&show);
    assert!(
        r.longest_still_in_phrase < 1.5,
        "{}",
        r.longest_still_in_phrase
    );
}

fn run_player(p: &mut Player, clock: impl Fn(f32) -> Clock, secs: f32) -> Vec<Frame> {
    let dt = 0.02;
    (0..(secs / dt) as usize)
        .map(|k| p.tick(dt, clock(k as f32 * dt)))
        .collect()
}

fn max_step(frames: &[Frame], dt: f32) -> (f32, f32) {
    let mut m = (0.0f32, 0.0f32);
    for w in frames.windows(2) {
        m.0 = m.0.max(((w[1].mouth - w[0].mouth) / dt).abs());
        for j in 0..4 {
            m.1 = m.1.max(((w[1].head[j] - w[0].head[j]) / dt).abs());
        }
    }
    m
}

#[test]
fn seeking_never_jumps_a_joint() {
    let song = perf(
        60.0,
        (0..20)
            .map(|i| {
                syl(
                    2.0 + i as f32 * 2.5,
                    4.0 + i as f32 * 2.5,
                    Vowel::Ah,
                    60.0 + i as f32,
                )
            })
            .collect(),
    );
    let show = compile(song, Style::pop());
    let prof = show.profile;
    let mut p = Player::new(show);
    // Play 10 s, then jump to 40 s, then back to 5 s.
    let frames = run_player(
        &mut p,
        |t| {
            Clock::Playing(if t < 10.0 {
                f64::from(t)
            } else if t < 14.0 {
                40.0 + f64::from(t - 10.0)
            } else {
                5.0 + f64::from(t - 14.0)
            })
        },
        20.0,
    );
    let (vm, vh) = max_step(&frames, 0.02);
    assert!(vm <= prof.mouth.max_vel * 1.02, "beak step {vm}");
    assert!(vh <= prof.head.max_vel[1] * 1.02, "head step {vh}");
}

#[test]
fn pause_goes_to_rest_and_resume_does_not_jump() {
    let song = perf(
        60.0,
        (0..20)
            .map(|i| syl(2.0 + i as f32 * 2.5, 4.0 + i as f32 * 2.5, Vowel::Ah, 60.0))
            .collect(),
    );
    let show = compile(song, Style::pop());
    let prof = show.profile;
    let mut p = Player::new(show);
    let mut frames = run_player(&mut p, |t| Clock::Playing(f64::from(t) + 2.0), 3.0);
    assert_eq!(p.phase(), Phase::Performing);
    frames.extend(run_player(&mut p, |_| Clock::Paused, 3.0));
    assert_eq!(p.phase(), Phase::Idle, "rest reached");
    let last = frames.last().unwrap();
    assert!(last.mouth < 1e-3 && last.head.iter().all(|h| h.abs() < 1e-3));
    frames.extend(run_player(
        &mut p,
        |t| Clock::Playing(f64::from(t) + 5.0),
        3.0,
    ));
    let (vm, vh) = max_step(&frames, 0.02);
    assert!(
        vm <= prof.mouth.max_vel * 1.02 && vh <= prof.head.max_vel[1] * 1.02,
        "{vm} {vh}"
    );
}

#[test]
fn running_off_the_end_settles_and_reports_idle() {
    let song = perf(10.0, vec![syl(1.0, 3.0, Vowel::Ah, 64.0)]);
    let show = compile(song, Style::ballad());
    let end = f64::from(show.duration());
    let mut p = Player::new(show);
    run_player(&mut p, |t| Clock::Playing(f64::from(t)), 12.0);
    run_player(&mut p, |_| Clock::Playing(end + 1.0), 4.0);
    assert_eq!(p.phase(), Phase::Idle);
}

/// What the mechanism does to the beak: held notes survive a pessimistic servo, and the fine
/// articulation is what is lost — never the other way round. This is the sensitivity the
/// calibration stage exists to resolve: how much of the designed dip survives depends on the
/// deadband and backlash that only a bench measurement can give.
#[test]
fn a_sloppy_beak_loses_articulation_before_it_loses_the_held_note() {
    use singer::Mechanism;
    let sy: Vec<Syllable> = (0..8)
        .map(|i| {
            let mut s = syl(
                1.0 + i as f32 * 1.2,
                1.0 + i as f32 * 1.2 + 1.0,
                Vowel::Ah,
                62.0,
            );
            s.onset = Onset::Closure;
            s
        })
        .collect();
    let show = compile(perf(12.0, sy), Style::pop());
    let ideal = mouth_report(&show.perf, &show.mouth, 50.0, 4.0, 64.0);
    let bad = Mechanism::pessimistic().run(&show.mouth, 50.0);
    let r = mouth_report(&show.perf, &bad, 50.0, 4.0, 64.0);
    assert_eq!(r.long_notes_stable, r.long_notes, "held notes survive");
    assert!(
        r.gaps_articulated <= ideal.gaps_articulated,
        "and the dips can only shrink"
    );
    assert!(
        ideal.gaps_articulated > 0,
        "the designed beak does articulate"
    );
}

/// 一字多音: three notes on one syllable do not dip between them; the same three as separate
/// syllables do.
#[test]
fn a_melisma_follows_the_pitch_without_closing() {
    let notes = |tie: bool| {
        let mut v = vec![
            syl(1.0, 2.0, Vowel::Ah, 67.0),
            syl(2.0, 3.0, Vowel::Ah, 65.0),
            syl(3.0, 4.2, Vowel::Ah, 62.0),
        ];
        for n in &mut v[1..] {
            n.tie = tie;
            // Separate syllables start on a consonant; a melisma's later notes do not.
            n.onset = if tie { Onset::None } else { Onset::Closure };
        }
        v
    };
    let dip = |tie: bool| {
        let show = compile(perf(7.0, notes(tie)), Style::ballad());
        let show = &show;
        let range = move |a: f32, b: f32| {
            (0..((b - a) * 100.0) as usize + 1).map(move |k| at(show, a + k as f32 * 0.01))
        };
        let valley = range(1.8, 2.0).fold(f32::MAX, f32::min);
        let before = range(1.5, 1.8).fold(0.0f32, f32::max);
        let after = range(1.99, 2.03).fold(0.0f32, f32::max);
        // How far the beak sinks below both of the openings it joins: zero when it just moves
        // from one to the other.
        (before.min(after) - valley).max(0.0)
    };
    assert!(
        dip(true) < 0.01,
        "tied: the beak does not dip at the join, {}",
        dip(true)
    );
    assert!(dip(false) > 0.03, "separate syllables do: {}", dip(false));
}

/// The climax of a song sung on a narrow vowel is not a narrower beak than its quiet start.
#[test]
fn a_high_loud_note_pulls_a_narrow_vowel_open() {
    let mut low = syl(1.0, 2.5, Vowel::Ee, 62.0);
    low.level = 0.5;
    let mut high = syl(4.0, 5.5, Vowel::Ee, 74.0);
    high.level = 1.0;
    let show = compile(perf(8.0, vec![low, high]), Style::ballad());
    let peak = |a: f32| {
        (0..100)
            .map(|k| at(&show, a + k as f32 * 0.01))
            .fold(0.0f32, f32::max)
    };
    assert!(
        peak(4.2) > peak(1.2) + 0.2,
        "{} vs {}",
        peak(4.2),
        peak(1.2)
    );
}
