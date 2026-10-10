use show::DuckScore;
use singer::{Profile, Show, Style};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let score = DuckScore::parse(&std::fs::read(&a[1]).unwrap()).unwrap();
    let (from, to): (f32, f32) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let perf = score.to_performance_for("林俊杰-学不会", 231.0).unwrap().with_pre_roll(0.0);
    let style = Style::by_name("lively").unwrap().with_seed(singer::dsp::seed_of(&perf.name));
    let mut profile = Profile::default();
    profile.head = singer::profile::HeadProfile::lively();
    let show = Show::compile(perf.clone(), style, profile).unwrap();
    println!("mouth stats: {:?}", show.mouth_stats);
    for s in perf.syllables.iter().filter(|s| s.start >= from - 0.2 && s.start <= to) {
        println!("syllable {:.3}-{:.3} ({:3.0} ms) pitch {:.0} onset {:?} tie {}", s.start, s.end, (s.end - s.start) * 1000.0, s.pitch, s.onset, s.tie);
    }
    let r = show.profile.rate;
    let mut t = from;
    let mut line = String::new();
    while t < to {
        let i = (t * r) as usize;
        line += &format!("{:.2}:{:.2} ", t, show.mouth[i]);
        t += 0.05;
    }
    println!("{line}");
}
