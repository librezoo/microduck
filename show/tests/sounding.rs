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
    assert!((perf.syllables[0].end - 0.4).abs() < 1e-3, "{:?}", perf.syllables[0]);
    assert!((perf.syllables[1].start - 0.7).abs() < 1e-3);
    assert!((perf.syllables[1].end - 1.0).abs() < 1e-3, "no end rule on a stated span");
}

#[test]
fn without_sounding_the_written_note_is_used_as_before() {
    let s = score(r#"{"eventId":"a","startBeat":0,"durationBeats":2,"voice":"ah","midiNote":62,"volume":0.8}"#);
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
    let s = score(r#"{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,900]]}"#);
    assert_eq!(s.end_ms(), 900);
}

fn deepest_dip(onset: &str) -> f32 {
    let s = score(&format!(
        r#"{{"eventId":"a","startBeat":0,"durationBeats":1,"voice":"ah","midiNote":62,"volume":0.8,"sounding":[[0,500]]}},
           {{"eventId":"b","startBeat":1,"durationBeats":1,"voice":"ah","midiNote":64,"volume":0.8,"sounding":[[500,1000]],"onset":"{onset}"}}"#
    ));
    let perf = s.to_performance("t").unwrap();
    let show = singer::Show::compile(perf.clone(), singer::Style::auto(&perf), singer::Profile::default()).unwrap();
    let (a, b) = ((0.3f32 * 50.0) as usize, (0.6f32 * 50.0) as usize);
    let lo = show.mouth[a..=b].iter().cloned().fold(f32::MAX, f32::min);
    let peak = show.mouth[..a].iter().cloned().fold(0.0f32, f32::max);
    peak - lo
}

#[test]
fn touching_syllables_still_articulate_and_a_lip_consonant_shuts_further() {
    let none = deepest_dip("none");
    let closure = deepest_dip("closure");
    assert!(none >= 0.15, "even a vowel-initial join shows a dip: {none}");
    assert!(closure > none + 0.05, "a lip consonant dips further: {closure} vs {none}");
}
