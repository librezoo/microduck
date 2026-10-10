//! Simulation harness: compile every shipped piece, run its beak track through the mechanism
//! models, compare against the baselines, and write the tracks out as CSV.
//!
//!   cargo run -p singer --example sing_report -- [--out DIR]
//!
//! Every number it prints is a *signal-level* measurement on generated motion (see
//! `singer::metrics`); none of it says how a viewer will find the result.

use std::fmt::Write as _;

use singer::metrics::{head_report, mouth_report, repeat_similarity};
use singer::{Layers, Mechanism, Profile, Show, Style, baselines, from_score};
use sounds::chorale::Score;

fn main() {
    let out_dir = std::env::args().skip_while(|a| a != "--out").nth(1);
    let profile = Profile::default();
    let (vd, ad) = (profile.mouth.max_vel * 0.8, profile.mouth.max_acc * 0.8);
    let mech = [
        ("ideal", Mechanism::IDEAL),
        ("plausible", Mechanism::plausible()),
        ("pessimistic", Mechanism::pessimistic()),
    ];

    for score in [Score::wistful(), Score::duck_strut(), Score::outer_wilds()] {
        for part in score.parts() {
            if part.as_str() != "soprano" && part.as_str() != "tenor" {
                continue;
            }
            let perf = from_score(&score, part);
            let style = Style::auto(&perf);
            let show = Show::compile(perf.clone(), style.clone(), profile).expect("compiles");
            println!(
                "\n== {}  ({:.0} s, {} syllables, {} phrases, {} sections, style {}, density {:.2}/s)",
                perf.name,
                perf.duration,
                perf.syllables.len(),
                perf.phrases.len(),
                perf.sections.len(),
                style.name,
                perf.sung_density()
            );
            println!(
                "   mouth stats: depth-limited {} / limiter {:.3}   head: accents {} nods {} clamped {:?}",
                show.mouth_stats.depth_limited,
                show.mouth_stats.limiter_frac,
                show.head_stats.accents,
                show.head_stats.nods,
                show.head_stats
                    .clamped_frac
                    .map(|v| (v * 1000.0).round() / 1000.0)
            );
            for (name, m) in &mech {
                let exec = m.run(&show.mouth, profile.rate);
                let r = mouth_report(&perf, &exec, profile.rate, vd, ad);
                println!(
                    "   beak [{name:11}] articulated {}/{} (feasible {}/{}) legato-dip {}/{} on-time {}/{} held {}/{}  vmax {:.2}/s  pitch-corr {:+.2} vowel-corr {:+.2} rest-closed {:.2}",
                    r.gaps_articulated,
                    r.gaps,
                    r.gaps_feasible_articulated,
                    r.gaps_feasible,
                    r.legato_dipped,
                    r.legato,
                    r.opened_on_time,
                    r.onsets,
                    r.long_notes_stable,
                    r.long_notes,
                    r.max_vel,
                    r.pitch_corr,
                    r.vowel_corr,
                    r.rest_closed
                );
            }
            let base = baselines::energy_mouth(&perf, &profile);
            let r = mouth_report(&perf, &base, profile.rate, vd, ad);
            println!(
                "   beak [energy-only ] articulated {}/{} (feasible {}/{}) legato-dip {}/{} on-time {}/{} held {}/{}  pitch-corr {:+.2} vowel-corr {:+.2}",
                r.gaps_articulated,
                r.gaps,
                r.gaps_feasible_articulated,
                r.gaps_feasible,
                r.legato_dipped,
                r.legato,
                r.opened_on_time,
                r.onsets,
                r.long_notes_stable,
                r.long_notes,
                r.pitch_corr,
                r.vowel_corr
            );

            let h = head_report(&show);
            println!(
                "   head peak {:?} budget-use {:?}  rms-vel {:.3} rad/s  path {:.2} rad/min  longest-still-in-phrase {:.1}s  loop (motion, music) {:?}",
                h.peak.map(|v| (v * 1000.0).round() / 1000.0),
                h.budget_use.map(|v| (v * 100.0).round() / 100.0),
                h.rms_vel,
                h.path_per_min,
                h.longest_still_in_phrase,
                (
                    h.loop_score.map(|v| (v * 100.0).round() / 100.0),
                    singer::metrics::music_loop(&perf, profile.rate)
                        .map(|v| (v * 100.0).round() / 100.0)
                )
            );
            for (kind, a, b, c) in repeat_similarity(&show) {
                println!("   repeat {kind:?} #{a} vs #{b}: head similarity {c:+.2}");
            }
            let beat_only = Show::compile_with(perf.clone(), style, profile, Layers::beat_only())
                .expect("compiles");
            let hb = head_report(&beat_only);
            println!(
                "   head [beat-only] path {:.2} rad/min  rms-vel {:.3}",
                hb.path_per_min, hb.rms_vel
            );

            if let Some(dir) = &out_dir {
                std::fs::create_dir_all(dir).expect("dir");
                let mut csv =
                    String::from("t,mouth,neck_pitch,head_pitch,head_yaw,head_roll,energy_mouth\n");
                for k in 0..show.head.len() {
                    let t = k as f32 / profile.rate;
                    let h = show.head[k];
                    let _ = writeln!(
                        csv,
                        "{t:.3},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
                        show.mouth.get(k).copied().unwrap_or(0.0),
                        h[0],
                        h[1],
                        h[2],
                        h[3],
                        base.get(k).copied().unwrap_or(0.0)
                    );
                }
                std::fs::write(format!("{dir}/{}.csv", perf.name), csv).expect("write");
                std::fs::write(
                    format!("{dir}/{}.performance.json", perf.name),
                    perf.to_json(),
                )
                .expect("write");
            }
        }
    }
}
