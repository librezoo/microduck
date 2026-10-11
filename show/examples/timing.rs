use show::DuckScore;
use singer::{Profile, Show, Style};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let score = DuckScore::parse(&std::fs::read(path).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let perf = score.to_performance_for("林俊杰-学不会", 231.0).unwrap();
    println!(
        "perf {:?} syllables {} phrases {} sections {}",
        t.elapsed(),
        perf.syllables.len(),
        perf.phrases.len(),
        perf.sections.len()
    );
    println!("performance json {} bytes", perf.to_json().len());
    let style = Style::by_name("lively")
        .unwrap()
        .with_seed(singer::dsp::seed_of(&perf.name))
        .scaled(1.0, 1.0);
    let mut profile = Profile::default();
    profile.head = singer::profile::HeadProfile::lively();
    let t = std::time::Instant::now();
    let show = Show::compile(perf, style, profile).unwrap();
    println!(
        "compile {:?} mouth {} head {}",
        t.elapsed(),
        show.mouth.len(),
        show.head.len()
    );
}
