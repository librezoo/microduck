//! How often does the beak close? `cargo run -p show --example inspect -- duck.score.json`
use show::DuckScore;
use singer::{Profile, Show, Style};

fn main() {
    let path = std::env::args().nth(1).expect("a duck.score.json");
    let score = DuckScore::parse(&std::fs::read(path).unwrap()).unwrap();
    let perf = score.to_performance("inspect").unwrap();
    let style = match std::env::args().nth(2) {
        Some(name) => Style::by_name(&name)
            .expect("a style")
            .with_seed(singer::dsp::seed_of("inspect")),
        None => Style::auto(&perf),
    };
    let style = match std::env::args().nth(3).and_then(|v| v.parse::<f32>().ok()) {
        Some(k) => style.scaled(k, 1.0),
        None => style,
    };
    let mut profile = Profile::default();
    if style.name == "lively" {
        profile.head = singer::profile::HeadProfile::lively();
    }
    let show = Show::compile(perf.clone(), style.clone(), profile).unwrap();
    println!("style {}, {} syllables", style.name, perf.syllables.len());
    liveliness(&show);
    let mut closed = 0;
    let mut dips = Vec::new();
    for w in perf.syllables.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        // The dip can start before the sound ends and end after the next one begins.
        let (t0, t1) = (
            ((a.end - 0.2).max(a.start) * 50.0) as usize,
            ((b.start + 0.1) * 50.0) as usize + 1,
        );
        let lo = show.mouth[t0.min(show.mouth.len() - 1)..=t1.min(show.mouth.len() - 1)]
            .iter()
            .cloned()
            .fold(f32::MAX, f32::min);
        let peak = show.mouth[(a.start * 50.0) as usize..(a.end * 50.0) as usize]
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        dips.push((b.start - a.end, lo, peak));
        if lo < 0.2 {
            closed += 1;
        }
    }
    for (gap, lo, peak) in &dips {
        println!("  gap {gap:5.2} s → beak down to {lo:.2} (from {peak:.2})");
    }
    let dipped = dips
        .iter()
        .filter(|(_, lo, peak)| peak - lo >= 0.15)
        .count();
    println!(
        "{closed} of {} boundaries close to below 0.2; {dipped} dip by at least 0.15",
        dips.len()
    );
}

/// How lively is it? Numbers for the sung part only (where the beak is not resting closed).
pub fn liveliness(show: &Show) {
    let r = show.profile.rate;
    let m = &show.mouth;
    let active: Vec<usize> = (0..m.len()).filter(|&i| m[i] > 0.05).collect();
    if active.is_empty() {
        return;
    }
    let mean = active.iter().map(|&i| m[i]).sum::<f32>() / active.len() as f32;
    let sd =
        (active.iter().map(|&i| (m[i] - mean).powi(2)).sum::<f32>() / active.len() as f32).sqrt();
    // Direction reversals of the beak per second of singing (turning points with a visible swing).
    let mut turns = 0;
    let mut last_ext = m[active[0]];
    let mut dir = 0i8;
    for &i in &active {
        let d = m[i] - last_ext;
        if d.abs() > 0.08 {
            let nd = if d > 0.0 { 1 } else { -1 };
            if nd != dir && dir != 0 {
                turns += 1;
            }
            dir = nd;
            last_ext = m[i];
        } else if (dir > 0 && m[i] > last_ext) || (dir < 0 && m[i] < last_ext) {
            last_ext = m[i];
        }
    }
    let secs = active.len() as f32 / r;
    let mut sorted: Vec<f32> = active.iter().map(|&i| m[i]).collect();
    sorted.sort_by(f32::total_cmp);
    let q = |f: f32| sorted[((sorted.len() - 1) as f32 * f) as usize];
    println!(
        "beak opening while singing: p10 {:.2}  p50 {:.2}  p90 {:.2}  p99 {:.2}",
        q(0.1),
        q(0.5),
        q(0.9),
        q(0.99)
    );
    println!(
        "beak: mean {mean:.2}, sd {sd:.2}, {:.1} swings/s over {secs:.0} s sung",
        turns as f32 / secs
    );
    for (j, name) in ["neck", "pitch", "yaw", "roll"].iter().enumerate() {
        let mut v: Vec<f32> = show.head.iter().map(|h| h[j].abs()).collect();
        v.sort_by(f32::total_cmp);
        let p = |q: f32| v[((v.len() - 1) as f32 * q) as usize];
        println!(
            "head {name}: p50 {:.3}  p95 {:.3}  max {:.3} rad",
            p(0.5),
            p(0.95),
            p(1.0)
        );
    }
}
