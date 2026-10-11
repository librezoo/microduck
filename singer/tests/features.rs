//! Behavioural tests for the features of the `lively` style and the knobs it turns on: each one
//! states what the feature does to the motion, with the other knobs held fixed, so that switching
//! the feature off (or breaking it) fails the test. Thresholds have margin; they are about the
//! direction and rough size of an effect, not its exact number.

use singer::{
    Beat, HeadProfile, Layers, Onset, Performance, Phrase, Profile, Section, SectionKind, Show,
    Style, Syllable, Vowel,
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

fn perf(bpm: f32, duration: f32, syllables: Vec<Syllable>) -> Performance {
    let (s, e) = (
        syllables.first().map_or(0.0, |s| s.start),
        syllables.last().map_or(1.0, |s| s.end),
    );
    Performance {
        name: "f".into(),
        bpm,
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
            intensity: 0.5,
            variant: 0,
        }],
        beats: vec![],
    }
}

fn profile() -> Profile {
    Profile {
        head: HeadProfile {
            pitch_up_sign: 1.0,
            ..HeadProfile::lively()
        },
        ..Profile::default()
    }
}

fn show(p: Performance, style: Style, layers: Layers) -> Show {
    Show::compile_with(p, style, profile(), layers).expect("compiles")
}

/// The raw beak track between two song times.
fn window(s: &Show, t0: f32, t1: f32) -> &[f32] {
    let r = s.profile.rate;
    &s.mouth[(t0 * r) as usize..(t1 * r) as usize]
}

fn max(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::MIN, f32::max)
}
fn min(v: &[f32]) -> f32 {
    v.iter().cloned().fold(f32::MAX, f32::min)
}

/// A style whose beak does nothing but follow the vowel's target: no settle, no drift, no pitch or
/// level dependence, no auto-range, no steps. A feature is then switched on by itself.
fn plain() -> Style {
    let mut st = Style::ballad();
    st.mouth.gain = 0.6;
    st.mouth.pitch_gain = 0.0;
    st.mouth.level_gain = 0.0;
    st.mouth.climax_open = 0.0;
    st.mouth.settle = 0.0;
    st.mouth.drift = 0.0;
    st.mouth.breath_open = 0.0;
    st
}

// ---- beak -----------------------------------------------------------------------------------

fn local_maxima(v: &[f32]) -> usize {
    (1..v.len() - 1)
        .filter(|&i| v[i] > v[i - 1] && v[i] >= v[i + 1] && v[i] > min(v) + 0.01)
        .count()
}

#[test]
fn a_held_note_pulses_with_the_beat_in_lively_and_only_drifts_without_pulse() {
    // 100 bpm: a beat is 0.6 s, so a pulse of a beat or half a beat gives 3+ peaks in 1.8 s; the
    // old drift (period 1.9-2.9 s) gives at most one.
    for seed in 0..6u64 {
        let p = perf(100.0, 8.0, vec![syl(1.0, 6.0, Vowel::Ah, 64.0)]);
        let lively = Style::lively().with_seed(seed);
        let on = show(p.clone(), lively.clone(), Layers::NONE);
        let mut off_style = lively;
        off_style.mouth.pulse_beats = 0.0;
        let off = show(p, off_style, Layers::NONE);
        let peaks_on = local_maxima(window(&on, 1.8, 3.6));
        let peaks_off = local_maxima(window(&off, 1.8, 3.6));
        assert!(peaks_on >= 3, "seed {seed}: pulse gives {peaks_on} peaks");
        assert!(peaks_off <= 1, "seed {seed}: drift gives {peaks_off} peaks");
    }
}

#[test]
fn the_ballad_has_no_pulse() {
    let p = perf(100.0, 8.0, vec![syl(1.0, 6.0, Vowel::Ah, 64.0)]);
    let s = show(p, Style::ballad(), Layers::NONE);
    assert!(local_maxima(window(&s, 1.8, 3.6)) <= 1);
}

#[test]
fn a_short_tied_fragment_keeps_the_opening_before_it() {
    // low note, then an 80 ms tied fragment on a much higher note, then a rest and another note.
    // Touching: the beak does not re-aim for the fragment. Apart: it does.
    let build = |frag_start: f32| {
        let mut f = syl(frag_start, frag_start + 0.08, Vowel::Ah, 76.0);
        f.tie = true;
        let p = perf(
            100.0,
            5.0,
            vec![
                syl(1.0, 2.0, Vowel::Ah, 60.0),
                f,
                syl(3.0, 4.0, Vowel::Ah, 76.0),
            ],
        );
        let mut st = plain();
        st.mouth.pitch_gain = 0.8;
        show(p, st, Layers::NONE)
    };
    let touching = build(2.0);
    let apart = build(2.2);
    let level = max(window(&touching, 1.5, 1.9));
    let during_touching = max(window(&touching, 2.0, 2.45));
    let during_apart = max(window(&apart, 2.2, 2.65));
    assert!(
        during_touching <= level + 0.03,
        "{during_touching} vs {level}"
    );
    assert!(during_apart >= level + 0.05, "{during_apart} vs {level}");
}

#[test]
fn a_step_in_pitch_opens_or_narrows_the_beak_inside_a_phrase_only() {
    // Two notes of the same vowel (flat style) with a rise, a fall, or a rise across a long rest.
    let peak_of_second = |pitch2: f32, gap: f32, gain: f32| {
        let (s2, e2) = (2.0 + gap, 3.0 + gap);
        let p = perf(
            100.0,
            6.0,
            vec![
                syl(1.0, 2.0, Vowel::Eh, 60.0),
                syl(s2, e2, Vowel::Eh, pitch2),
            ],
        );
        let mut st = plain();
        st.mouth.interval_gain = gain;
        let s = show(p, st, Layers::NONE);
        max(window(&s, s2 + 0.3, e2 - 0.2))
    };
    let flat = peak_of_second(68.0, 0.1, 0.0);
    let up = peak_of_second(68.0, 0.1, 0.3);
    let down = peak_of_second(52.0, 0.1, 0.3);
    let flat_down = peak_of_second(52.0, 0.1, 0.0);
    assert!(up >= flat + 0.15, "rise: {up} vs {flat}");
    assert!(down <= flat_down - 0.1, "fall: {down} vs {flat_down}");
    // Across a 0.6 s rest (a breath) there is no step.
    let rest_on = peak_of_second(68.0, 0.6, 0.3);
    let rest_off = peak_of_second(68.0, 0.6, 0.0);
    assert!((rest_on - rest_off).abs() < 0.01, "{rest_on} vs {rest_off}");
}

#[test]
fn auto_range_stretches_a_song_to_its_own_spread_and_leaves_a_short_one_alone() {
    // Ten syllables, half on "ah" and half on "oo", separated by rests so each is read cleanly.
    let mut sy = Vec::new();
    for i in 0..10 {
        let t = 1.0 + i as f32 * 1.0;
        let v = if i % 2 == 0 { Vowel::Ah } else { Vowel::Oo };
        sy.push(syl(t, t + 0.7, v, 64.0));
    }
    let mut st = plain();
    st.mouth.gain = 1.0;
    st.mouth.auto_range = true;
    let fitted = show(perf(100.0, 13.0, sy.clone()), st.clone(), Layers::NONE);
    st.mouth.auto_range = false;
    let raw = show(perf(100.0, 13.0, sy), st, Layers::NONE);
    let at = |s: &Show, i: usize| max(window(s, 1.0 + i as f32 + 0.3, 1.0 + i as f32 + 0.6));
    // Wide vowels reach the top, narrow ones sit well below them, and more so than unfitted.
    assert!(at(&fitted, 0) >= 0.95, "wide: {}", at(&fitted, 0));
    let spread = at(&fitted, 0) - at(&fitted, 1);
    assert!((0.35..0.6).contains(&spread), "spread {spread}");
    assert!(at(&raw, 0) < 0.92, "unfitted wide: {}", at(&raw, 0));

    // A song with little variation (ee/oo only) is stretched to the same range rather than left
    // looking half shut.
    let sy: Vec<Syllable> = (0..10)
        .map(|i| {
            let v = if i % 2 == 0 { Vowel::Ee } else { Vowel::Oo };
            syl(1.0 + i as f32, 1.7 + i as f32, v, 64.0)
        })
        .collect();
    let mut st = plain();
    st.mouth.gain = 1.0;
    st.mouth.auto_range = true;
    let fitted = show(perf(100.0, 13.0, sy.clone()), st.clone(), Layers::NONE);
    st.mouth.auto_range = false;
    let raw = show(perf(100.0, 13.0, sy), st, Layers::NONE);
    assert!(at(&fitted, 0) >= 0.95 && at(&raw, 0) <= 0.5);

    // Fewer than eight sung syllables: not stretched, the track is the unfitted one.
    let few: Vec<Syllable> = (0..5)
        .map(|i| syl(1.0 + i as f32, 1.7 + i as f32, Vowel::Oo, 64.0))
        .collect();
    let mut st = plain();
    st.mouth.gain = 1.0;
    st.mouth.auto_range = true;
    let a = show(perf(100.0, 8.0, few.clone()), st.clone(), Layers::NONE);
    st.mouth.auto_range = false;
    let b = show(perf(100.0, 8.0, few), st, Layers::NONE);
    assert_eq!(a.mouth, b.mouth);
}

#[test]
fn flatten_pulls_openings_toward_the_songs_mean_before_fitting() {
    let mut sy = Vec::new();
    for i in 0..10 {
        let t = 1.0 + i as f32 * 1.0;
        let v = if i % 2 == 0 { Vowel::Ah } else { Vowel::Oo };
        sy.push(syl(t, t + 0.7, v, 64.0));
    }
    let spread = |flatten: f32| {
        let mut st = plain();
        st.mouth.gain = 1.0;
        st.mouth.auto_range = true;
        st.mouth.flatten = flatten;
        let s = show(perf(100.0, 13.0, sy.clone()), st, Layers::NONE);
        let at = |i: usize| max(window(&s, 1.0 + i as f32 + 0.3, 1.0 + i as f32 + 0.6));
        at(0) - at(1)
    };
    // The fit rescales whatever is left to the same top, but a flattened song has less spread
    // to rescale, and the stretch is bounded (2x), so it ends narrower.
    assert!(
        spread(0.0) > spread(0.8) + 0.1,
        "{} vs {}",
        spread(0.0),
        spread(0.8)
    );
}

#[test]
fn the_dip_between_touching_syllables_is_deeper_for_a_lip_consonant() {
    let depth = |onset: Onset| {
        let mut b = syl(2.0, 2.8, Vowel::Ah, 64.0);
        b.onset = onset;
        let p = perf(100.0, 5.0, vec![syl(1.2, 2.0, Vowel::Ah, 64.0), b]);
        let mut st = plain();
        st.mouth.legato_dip = 0.35;
        let s = show(p, st, Layers::NONE);
        min(window(&s, 1.7, 2.3))
    };
    let (none, soft, closure) = (
        depth(Onset::None),
        depth(Onset::Soft),
        depth(Onset::Closure),
    );
    assert!(
        none > soft + 0.05 && soft > closure + 0.05,
        "{none} {soft} {closure}"
    );
}

#[test]
fn the_beak_never_exceeds_the_velocity_limit_on_touching_syllables() {
    let mut sy = Vec::new();
    for i in 0..16 {
        let t = 1.0 + i as f32 * 0.1;
        let v = if i % 2 == 0 { Vowel::Ah } else { Vowel::Oo };
        sy.push(syl(t, t + 0.1, v, 60.0 + (i % 5) as f32 * 3.0));
    }
    for style in [Style::pop(), Style::lively(), Style::chant()] {
        let s = show(perf(100.0, 4.0, sy.clone()), style.clone(), Layers::NONE);
        let limit = s.profile.mouth.max_vel * singer::mouth::CEILING;
        let worst = s
            .mouth
            .windows(2)
            .map(|w| (w[1] - w[0]).abs() * s.profile.rate)
            .fold(0.0f32, f32::max);
        assert!(worst <= limit * 1.001, "{}: {worst} > {limit}", style.name);
    }
}

// ---- head -----------------------------------------------------------------------------------

fn only(f: impl FnOnce(&mut Layers)) -> Layers {
    let mut l = Layers::NONE;
    f(&mut l);
    l
}

fn mean(v: &[[f32; 4]], j: usize, t0: f32, t1: f32, rate: f32) -> f32 {
    let s = &v[(t0 * rate) as usize..(t1 * rate) as usize];
    s.iter().map(|x| x[j]).sum::<f32>() / s.len() as f32
}

#[test]
fn layers_none_is_a_motionless_head() {
    let p = perf(100.0, 6.0, vec![syl(1.0, 5.0, Vowel::Ah, 64.0)]);
    for style in [
        Style::ballad(),
        Style::pop(),
        Style::chant(),
        Style::lively(),
    ] {
        let s = show(p.clone(), style, Layers::NONE);
        assert!(s.head.iter().all(|f| *f == [0.0; 4]));
    }
}

#[test]
fn the_head_follows_the_melody_up_and_down() {
    // Low notes then high notes in one phrase.
    let mut sy = Vec::new();
    for i in 0..4 {
        sy.push(syl(
            1.0 + i as f32 * 0.8,
            1.7 + i as f32 * 0.8,
            Vowel::Ah,
            55.0,
        ));
    }
    for i in 0..4 {
        sy.push(syl(
            4.5 + i as f32 * 0.8,
            5.2 + i as f32 * 0.8,
            Vowel::Ah,
            67.0,
        ));
    }
    let p = perf(100.0, 10.0, sy);
    let r = 50.0;
    let on = show(p.clone(), Style::lively(), only(|l| l.follow = true));
    let high = mean(&on.head, 1, 6.0, 7.5, r);
    let low = mean(&on.head, 1, 2.0, 3.5, r);
    assert!(high - low > 0.15, "head pitch high {high}, low {low}");
    // Off by the layer, and off by the style knob.
    let off = show(p.clone(), Style::lively(), Layers::NONE);
    assert!(off.head.iter().all(|f| *f == [0.0; 4]));
    let mut st = Style::lively();
    st.head.follow = 0.0;
    let off = show(p, st, only(|l| l.follow = true));
    assert!(off.head.iter().all(|f| *f == [0.0; 4]));
}

#[test]
fn the_swing_is_zero_at_a_bar_line_and_biggest_a_quarter_bar_in_with_the_roll_a_quarter_behind() {
    // 100 bpm: a bar is 2.4 s. A long phrase so the head is "singing" throughout.
    let sy: Vec<Syllable> = (0..20)
        .map(|i| syl(0.5 + i as f32 * 0.9, 1.2 + i as f32 * 0.9, Vowel::Ah, 64.0))
        .collect();
    let mut p = perf(100.0, 22.0, sy);
    p.phrases[0].climax = 10.0;
    let mut st = Style::lively();
    st.head.idle = 0.0;
    let mut lim = profile();
    lim.head.max_vel = [100.0; 4];
    lim.head.max_acc = [1000.0; 4];
    let sh = Show::compile_with(p, st, lim, only(|l| l.swing = true)).unwrap();
    let r = sh.profile.rate;
    let bar = 2.4;
    let (mut yaw_q, mut yaw_0, mut roll_q, mut roll_0) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for b in 2..8 {
        let t0 = b as f32 * bar;
        let at = |t: f32, j: usize| sh.head[(t * r).round() as usize][j].abs();
        yaw_0 = yaw_0.max(at(t0, 2));
        roll_q = roll_q.max(at(t0 + bar / 4.0, 3));
        yaw_q = yaw_q.max(at(t0 + bar / 4.0, 2));
        roll_0 = roll_0.max(at(t0, 3));
    }
    assert!(yaw_0 < 0.01, "yaw at a bar line {yaw_0}");
    assert!(yaw_q > 0.08, "yaw a quarter bar in {yaw_q}");
    assert!(
        roll_q < 0.5 * yaw_q,
        "roll should be near zero there: {roll_q} vs {yaw_q}"
    );
    assert!(roll_0 > 0.01, "roll at the bar line {roll_0}");
    // The style knob switches it off.
    let mut st = Style::lively();
    st.head.swing = 0.0;
    let off = Show::compile_with(
        perf(100.0, 5.0, vec![syl(1.0, 4.0, Vowel::Ah, 64.0)]),
        st,
        profile(),
        only(|l| l.swing = true),
    )
    .unwrap();
    assert!(off.head.iter().all(|f| *f == [0.0; 4]));
}

#[test]
fn the_bow_comes_after_the_last_note_and_only_then() {
    let p = perf(100.0, 6.0, vec![syl(1.0, 3.0, Vowel::Ah, 64.0)]);
    let s = show(p, Style::lively(), only(|l| l.bow = true));
    let before = min(&s.head[..(3.0 * 50.0) as usize]
        .iter()
        .map(|f| f[1])
        .collect::<Vec<_>>());
    let after = min(&s.head.iter().map(|f| f[1]).collect::<Vec<_>>());
    assert!(before > -0.01, "{before}");
    assert!(after < -0.08, "the bow {after}");
}

#[test]
fn a_big_section_lifts_the_head_before_it_arrives() {
    let mut p = perf(100.0, 14.0, vec![syl(1.0, 2.0, Vowel::Ah, 64.0)]);
    p.sections = vec![
        Section {
            kind: SectionKind::Verse,
            start: 0.0,
            end: 8.0,
            intensity: 0.5,
            variant: 0,
        },
        Section {
            kind: SectionKind::Chorus,
            start: 8.0,
            end: 14.0,
            intensity: 1.0,
            variant: 0,
        },
    ];
    let s = show(p, Style::lively(), only(|l| l.mood = true));
    let r = 50.0;
    let well_before = mean(&s.head, 1, 4.0, 5.5, r);
    let two_seconds_ahead = mean(&s.head, 1, 6.4, 7.7, r);
    assert!(well_before.abs() < 0.005, "{well_before}");
    assert!(two_seconds_ahead > 0.04, "{two_seconds_ahead}");
}

#[test]
fn the_neck_takes_about_a_third_of_the_head_pitch() {
    let mut p = perf(
        100.0,
        8.0,
        vec![
            syl(1.0, 3.0, Vowel::Ah, 60.0),
            syl(3.0, 6.0, Vowel::Ah, 72.0),
        ],
    );
    p.phrases[0].climax = 3.0;
    let s = show(p, Style::ballad(), only(|l| l.arc = true));
    let (mut best, mut ratio) = (0.0f32, 0.0f32);
    for f in &s.head {
        if f[1].abs() > best {
            best = f[1].abs();
            ratio = f[0] / f[1];
        }
    }
    assert!(best > 0.02, "the arc moved the head: {best}");
    assert!((0.25..0.45).contains(&ratio), "neck/head {ratio}");
}

#[test]
fn a_run_of_stressed_syllables_is_not_a_seizure() {
    let sy: Vec<Syllable> = (0..10)
        .map(|i| {
            let mut s = syl(
                1.0 + i as f32 * 0.15,
                1.15 + i as f32 * 0.15,
                Vowel::Ah,
                64.0,
            );
            s.accent = 1.0;
            s
        })
        .collect();
    let s = show(perf(100.0, 5.0, sy), Style::lively(), Layers::ALL);
    let n = s.head_stats.accents;
    assert!((3..=5).contains(&n), "{n} accents in 1.5 s");
}

#[test]
fn nodding_thins_to_the_downbeats_when_the_tempo_is_too_fast_to_nod_to() {
    let build = |bpm: f32| {
        let gap = 60.0 / bpm;
        let beats: Vec<Beat> = (0..(12.0 / gap) as usize)
            .map(|i| Beat {
                t: i as f32 * gap,
                strength: if i % 4 == 0 { 1.0 } else { 0.5 },
            })
            .collect();
        let mut p = perf(bpm, 13.0, vec![syl(1.0, 11.0, Vowel::Ah, 64.0)]);
        p.beats = beats.clone();
        let strong = beats.iter().filter(|b| b.strength >= 0.9).count();
        (
            show(p, Style::lively(), Layers::ALL).head_stats.nods,
            beats.len(),
            strong,
        )
    };
    // 100 bpm is 1.7 Hz, under the lively limit of 2.2 Hz: every beat nods. 200 bpm is 3.3 Hz.
    let (nods, all, _) = build(100.0);
    assert_eq!(nods, all);
    let (nods, all, strong) = build(200.0);
    assert_eq!(nods, strong);
    assert!(nods < all / 2);
}
