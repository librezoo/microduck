//! How often does the beak close? `cargo run -p show --example inspect -- duck.score.json`
use show::DuckScore;
use singer::{Profile, Show, Style};

fn main() {
    let path = std::env::args().nth(1).expect("a duck.score.json");
    let score = DuckScore::parse(&std::fs::read(path).unwrap()).unwrap();
    let perf = score.to_performance("inspect").unwrap();
    let style = Style::auto(&perf);
    let show = Show::compile(perf.clone(), style.clone(), Profile::default()).unwrap();
    println!("style {}, {} syllables", style.name, perf.syllables.len());
    let mut closed = 0;
    let mut dips = Vec::new();
    for w in perf.syllables.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let (t0, t1) = ((a.end * 50.0) as usize, (b.start * 50.0) as usize + 1);
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
    println!("{closed} of {} boundaries close to below 0.2", dips.len());
}
