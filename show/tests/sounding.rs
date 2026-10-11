//! The duck track can state when each note is really audible; the beak follows that, not the
//! written grid.

use show::duck::DuckScore;

fn score(events: &str) -> DuckScore {
    let json = format!(r#"{{"schema":"kivi.duck/1","tempoBpm":120,"events":[{events}]}}"#);
    DuckScore::parse(json.as_bytes()).expect("valid duck track")
}

#[test]
fn a_breath_inside_a_note_becomes_a_real_closure() {
    // 2 beats at 120 bpm = 1 s written, but audible 0–400 ms and 700–1000 ms.
    let s = score(
        r#"{"eventId":"a","startBeat":0,"durationBeats":2,"voice":"ah","midiNote":62,"volume":0.8,
            "sounding":[[0,400],[700,1000]]}"#,
    );
    let perf = s.to_performance("t").unwrap();
    assert_eq!(perf.syllables.len(), 2);
    assert!(
        (perf.syllables[0].end - 0.4).abs() < 1e-3,
        "{:?}",
        perf.syllables[0]
    );
    assert!((perf.syllables[1].start - 0.7).abs() < 1e-3);
    assert!(
        (perf.syllables[1].end - 1.0).abs() < 1e-3,
        "no end rule on a stated span"
    );
}

#[test]
fn without_sounding_the_written_note_is_used_as_before() {
    let s = score(
        r#"{"eventId":"a","startBeat":0,"durationBeats":2,"voice":"ah","midiNote":62,"volume":0.8}"#,
    );
    let perf = s.to_performance("t").unwrap();
    assert_eq!(perf.syllables.len(), 1);
    assert!(perf.syllables[0].end < 1.0, "the release is still applied");
}

#[test]
fn spans_that_lean_on_the_next_note_are_cut_at_its_start() {
    let s = score(
        r#"{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,600]]},
           {"eventId":"b","startBeat":1,"durationBeats":1,"voice":"oh","midiNote":64,"volume":0.8,"sounding":[[500,1000]]}"#,
    );
    let perf = s.to_performance("t").unwrap();
    assert!(perf.syllables[0].end <= perf.syllables[1].start + 1e-6);
}

#[test]
fn bad_spans_are_refused() {
    let json = r#"{"schema":"kivi.duck/1","tempoBpm":120,"events":[{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[500,400]]}]}"#;
    assert!(DuckScore::parse(json.as_bytes()).is_err());
}

#[test]
fn the_show_lasts_until_the_last_audible_moment() {
    let s = score(
        r#"{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,900]]}"#,
    );
    assert_eq!(s.end_ms(), 900);
}

fn deepest_dip(onset: &str) -> f32 {
    let s = score(&format!(
        r#"{{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,500]]}},
           {{"eventId":"b","startBeat":1,"durationBeats":1,"voice":"ah","midiNote":64,"volume":0.8,"sounding":[[500,1000]],"onset":"{onset}"}}"#
    ));
    let perf = s.to_performance("t").unwrap();
    let show = singer::Show::compile(
        perf.clone(),
        singer::Style::auto(&perf),
        singer::Profile::default(),
    )
    .unwrap();
    let (a, b) = ((0.3f32 * 50.0) as usize, (0.6f32 * 50.0) as usize);
    let lo = show.mouth[a..=b].iter().cloned().fold(f32::MAX, f32::min);
    let peak = show.mouth[..a].iter().cloned().fold(0.0f32, f32::max);
    peak - lo
}

#[test]
fn touching_syllables_still_articulate_and_a_lip_consonant_shuts_further() {
    let none = deepest_dip("none");
    let closure = deepest_dip("closure");
    assert!(
        none >= 0.15,
        "even a vowel-initial join shows a dip: {none}"
    );
    assert!(
        closure > none + 0.05,
        "a lip consonant dips further: {closure} vs {none}"
    );
}

/// Beak opening in the middle of each of two tied notes (a run), in the lively style. A narrow
/// vowel, because a wide one (ah) is already at the top of the beak's range and cannot open more.
fn run_openings(from: u8, to: u8) -> (f32, f32) {
    let json = format!(
        r#"{{"schema":"kivi.duck/1","tempoBpm":120,"events":[
        {{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"oo","midiNote":{from},"volume":0.8,"sounding":[[0,500]]}},
        {{"eventId":"b","startBeat":1,"durationBeats":1,"voice":"oo","midiNote":{to},"volume":0.8,"continuesPrevious":true,"sounding":[[500,1000]]}}]}}"#
    );
    let perf = DuckScore::parse(json.as_bytes())
        .unwrap()
        .to_performance("t")
        .unwrap();
    let style = singer::Style::by_name("lively").unwrap();
    let show = singer::Show::compile(perf, style, singer::Profile::default()).unwrap();
    (
        show.mouth[(0.4f32 * 50.0) as usize],
        show.mouth[(0.95f32 * 50.0) as usize],
    )
}

#[test]
fn a_run_up_opens_the_beak_wider_and_a_run_down_narrows_it() {
    let (a, b) = run_openings(60, 67);
    assert!(b > a + 0.1, "up: {a} -> {b}");
    let (a, b) = run_openings(67, 60);
    assert!(b < a - 0.1, "down: {a} -> {b}");
}

/// Opening percentiles while singing, for a song of 40 one-beat notes on one vowel and a gentle
/// melody, in the lively style.
fn song_opening(vowel: &str) -> (f32, f32) {
    let notes: Vec<String> = (0..40)
        .map(|i| {
            let (m, s) = (60 + [0, 2, 4, 2, 0, -2, 0, 3][i % 8], i as f64 * 500.0);
            format!(
                r#"{{"eventId":"n{i}","startBeat":{i},"durationBeats":1,"voice":"{vowel}","midiNote":{m},"volume":0.8,"sounding":[[{s},{e}]]}}"#,
                e = s + 420.0
            )
        })
        .collect();
    let perf = score(&notes.join(",")).to_performance("t").unwrap();
    let style = singer::Style::by_name("lively").unwrap();
    let show = singer::Show::compile(perf, style, singer::Profile::default()).unwrap();
    let mut v: Vec<f32> = show.mouth.iter().cloned().filter(|x| *x > 0.05).collect();
    v.sort_by(f32::total_cmp);
    (v[v.len() / 2], v[v.len() * 9 / 10])
}

#[test]
fn every_song_gets_a_wide_mouth_whatever_vowel_it_is_sung_on() {
    let (ah50, ah90) = song_opening("ah");
    let (oo50, oo90) = song_opening("oo");
    assert!(
        ah90 > 0.8 && oo90 > 0.8,
        "wide ones reach the top: ah {ah90}, oo {oo90}"
    );
    assert!(
        (ah90 - oo90).abs() < 0.12,
        "same reach for both: {ah90} vs {oo90}"
    );
    assert!(
        oo50 > 0.45,
        "a song on oo does not look shut: median {oo50}"
    );
    assert!(
        ah50 < 0.95,
        "a song on ah is not pinned open: median {ah50}"
    );
}

/// Lowest beak opening during the 0.5-0.8 s silence between two notes, lively style.
fn gap_floor(continues: bool) -> f32 {
    let s = score(&format!(
        r#"{{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,400]]}},
           {{"eventId":"b","startBeat":2,"durationBeats":1,"voice":"ah","midiNote":64,"volume":0.8,"continuesPrevious":{continues},"sounding":[[900,1400]]}}"#
    ));
    let perf = s.to_performance("t").unwrap();
    let style = singer::Style::by_name("lively").unwrap();
    let show = singer::Show::compile(perf, style, singer::Profile::default()).unwrap();
    show.mouth[(0.55f32 * 50.0) as usize..=(0.75f32 * 50.0) as usize]
        .iter()
        .cloned()
        .fold(f32::MAX, f32::min)
}

#[test]
fn a_tie_across_an_audible_gap_is_not_a_tie() {
    let free = gap_floor(false);
    let tied = gap_floor(true);
    assert!(
        tied < 0.2,
        "the beak closes in the silence even if marked tied: {tied} (untied {free})"
    );
}

#[test]
fn a_tie_that_touches_is_kept() {
    let (a, b) = run_openings(60, 67);
    assert!(
        b > a + 0.1,
        "touching tied pieces still follow the pitch: {a} -> {b}"
    );
}
