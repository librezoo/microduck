//! A solo performance: the beak and the head following a compiled song, while the duck's own
//! voice sings along or something else does.
//!
//! The *what* lives in the `singer` crate — a [`singer::Show`] is the whole song's beak and head
//! motion, already feasible for the mechanism. This module is the part that belongs to the
//! daemon: it decides whether a request may start, compiles it **off the control thread** (a
//! three-minute song is tens of milliseconds of work, which a 20 ms tick cannot spare), keeps the
//! song clock, and hands the loop one frame a tick. The loop owns everything about the joints:
//! whether they may move, who else is moving them, and what a seek or a stop does to the
//! beak — that last one is the [`singer::Player`]'s job, and it never jumps a joint.
//!
//! Ownership: the theremin and the chorale each *are* the beak while they run, so a solo
//! performance refuses to start under either and yields to either that starts under it. One
//! consent covers all three, because all three move the beak and the head: `[chorale] accept`.
//!
//! Design: `docs/ideas/singing-performance.md`.

use std::time::Instant;

use duck_ipc_proto as proto;
use singer::{Clock, Frame, Performance, Phase, Player, Profile, Show, Style, from_score};
use sounds::chorale::{Part, Score};

/// Seconds between the request and the first note of a shipped piece: the beak is shut and the
/// head takes its breath. Part of the performance, not a delay in front of it.
pub const PRE_ROLL_S: f32 = 1.5;

/// What the control loop needs to start a performance.
pub struct Job {
    pub show: Show,
    /// The score the duck's own voice sings, with the line it sings. `None` for a performance
    /// handed in as data, whose sound — if any — comes from elsewhere.
    pub score: Option<(Score, Part)>,
    pub mute: bool,
    /// Where song time zero is inside the show, seconds: the pre-roll of a shipped piece.
    pub audio_start_s: f64,
}

pub enum Command {
    Stop,
    Start(Box<Job>),
}

fn parse_part(name: &str) -> Option<Part> {
    Part::ALL.into_iter().find(|p| p.as_str() == name)
}

/// Validate and compile a request. Everything that can be refused is refused here, at the door,
/// with the reason — the control loop is handed only something it can run.
pub fn prepare(
    p: &proto::SingParams,
    opted_in: bool,
    has_voice: bool,
) -> Result<(Command, proto::SingResult), String> {
    if !p.active {
        return Ok((Command::Stop, proto::SingResult::default()));
    }
    if !opted_in {
        return Err(
            "this robot has not opted in to performing (`[chorale] accept` in robotd.toml): a \
             performance moves the beak and the head"
                .to_owned(),
        );
    }
    let style_by_name = |perf: &Performance| -> Result<Style, String> {
        match p.style.as_deref() {
            None => Ok(Style::auto(perf)),
            Some(name) => Style::by_name(name)
                .map(|s| s.with_seed(singer::dsp::seed_of(&perf.name)))
                .ok_or_else(|| format!("unknown style {name:?} — have ballad, pop, chant")),
        }
    };

    let (perf, score, audio_start_s) = match &p.performance {
        Some(json) => {
            let perf = Performance::from_json(json).map_err(|e| e.to_string())?;
            (perf, None, 0.0)
        }
        None => {
            let id = p.piece.unwrap_or(2);
            let score = crate::chorale::piece(id).ok_or_else(|| {
                format!(
                    "piece {id} is not on this robot — it has {}",
                    crate::chorale::piece_catalogue()
                )
            })?;
            let part_name = p.part.as_deref().unwrap_or("soprano");
            let part = parse_part(part_name).ok_or_else(|| {
                format!("unknown part {part_name:?} — have bass, tenor, alto, soprano")
            })?;
            if !score.parts().contains(&part) {
                let have: Vec<&str> = score.parts().iter().map(|p| p.as_str()).collect();
                return Err(format!(
                    "{} has no {part_name} line — it has {}",
                    score.name,
                    have.join(", ")
                ));
            }
            let perf = from_score(&score, part).with_pre_roll(PRE_ROLL_S);
            (perf, Some((score, part)), f64::from(PRE_ROLL_S))
        }
    };
    if !p.mute && score.is_some() && !has_voice {
        return Err(
            "this robot has no voice to sing with — ask for `mute` to move the beak and head only"
                .to_owned(),
        );
    }
    let style = style_by_name(&perf)?;
    let style_name = style.name.clone();
    let mut profile = Profile::default();
    for (field, ms) in [
        (&mut profile.mouth_latency, p.mouth_latency_ms),
        (&mut profile.head_latency, p.head_latency_ms),
        (&mut profile.audio_latency, p.audio_latency_ms),
    ] {
        if let Some(ms) = ms {
            if !(0.0..=1000.0).contains(&ms) {
                return Err(format!("a latency of {ms} ms is outside 0..1000"));
            }
            *field = (ms / 1000.0) as f32;
        }
    }
    let show = Show::compile(perf, style, profile).map_err(|e| e.to_string())?;
    let result = proto::SingResult {
        accepted: true,
        reason: None,
        name: Some(show.perf.name.clone()),
        duration_s: f64::from(show.duration()),
        style: Some(style_name),
    };
    Ok((
        Command::Start(Box::new(Job {
            show,
            score,
            mute: p.mute,
            audio_start_s,
        })),
        result,
    ))
}

/// A running performance.
pub struct Performer {
    player: Player,
    score: Option<(Score, Part)>,
    mute: bool,
    audio_start_s: f64,
    started: Instant,
    stopping: bool,
    name: String,
}

impl Performer {
    pub fn new(job: Box<Job>, now: Instant) -> Self {
        let name = job.show.perf.name.clone();
        Self {
            player: Player::new(job.show),
            score: job.score,
            mute: job.mute,
            audio_start_s: job.audio_start_s,
            started: now,
            stopping: false,
            name,
        }
    }

    /// Wind down: the beak closes and the head settles, then [`Self::tick`] reports done.
    pub fn stop(&mut self) {
        self.stopping = true;
    }

    fn song_time(&self, now: Instant) -> f64 {
        now.duration_since(self.started).as_secs_f64()
    }

    /// The line to sing and where in it, in beats — `None` before the music starts, after it
    /// ends, when muted, or when the sound is somebody else's.
    pub fn audio(&self, now: Instant) -> Option<(&Score, Part, f64)> {
        if self.mute || self.stopping {
            return None;
        }
        let (score, part) = self.score.as_ref()?;
        let t = self.song_time(now) - self.audio_start_s;
        let beats = t / score.beat_s();
        (t >= 0.0 && beats * score.beat_s() <= score.duration_s()).then_some((score, *part, beats))
    }

    /// Whether the voice should be silent now (stopped, or the music has ended).
    pub fn audio_over(&self, now: Instant) -> bool {
        self.score.is_some() && self.audio(now).is_none()
    }

    /// One control tick. `None` when the performance is over and the joints are at rest.
    pub fn tick(&mut self, now: Instant, dt: f32) -> Option<(Frame, proto::SingState)> {
        let t = self.song_time(now);
        let clock = if self.stopping {
            Clock::Stopped
        } else {
            Clock::Playing(t)
        };
        let frame = self.player.tick(dt, clock);
        if self.player.phase() == Phase::Idle
            && (self.stopping || t > f64::from(self.player.show().duration()))
        {
            return None;
        }
        let show = self.player.show();
        let phase = match self.player.phase() {
            Phase::Releasing => "settling",
            _ if t < self.audio_start_s => "lead-in",
            _ => "performing",
        };
        Some((
            frame,
            proto::SingState {
                name: self.name.clone(),
                phase: phase.to_owned(),
                t_s: t - self.audio_start_s,
                duration_s: f64::from(show.duration()) - self.audio_start_s,
                section: show.section_name(t),
                mouth: f64::from(frame.mouth),
                head: frame.head.map(f64::from),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn params(piece: Option<u8>) -> proto::SingParams {
        proto::SingParams {
            active: true,
            piece,
            part: None,
            style: None,
            mute: true,
            performance: None,
            mouth_latency_ms: None,
            head_latency_ms: None,
            audio_latency_ms: None,
        }
    }

    #[test]
    fn a_robot_that_has_not_opted_in_refuses_and_says_why() {
        let err = prepare(&params(Some(2)), false, true)
            .err()
            .expect("refused");
        assert!(err.contains("[chorale] accept"), "{err}");
    }

    #[test]
    fn stopping_is_never_refused() {
        let mut p = params(None);
        p.active = false;
        assert!(matches!(prepare(&p, false, false), Ok((Command::Stop, _))));
    }

    #[test]
    fn bad_requests_are_refused_at_the_door_with_what_the_robot_has() {
        let e = prepare(&params(Some(99)), true, true)
            .err()
            .expect("refused");
        assert!(e.contains("duck-strut"), "names the catalogue: {e}");
        let mut p = params(Some(2));
        p.part = Some("kazoo".into());
        assert!(
            prepare(&p, true, true)
                .err()
                .expect("refused")
                .contains("soprano")
        );
        let mut p = params(Some(2));
        p.style = Some("opera".into());
        assert!(
            prepare(&p, true, true)
                .err()
                .expect("refused")
                .contains("ballad")
        );
        let mut p = params(None);
        p.performance = Some("{\"nope\": 1}".into());
        assert!(prepare(&p, true, true).is_err());
    }

    #[test]
    fn a_voiceless_robot_can_still_perform_muted_but_not_sing() {
        let mut p = params(Some(2));
        p.mute = false;
        assert!(
            prepare(&p, true, false)
                .err()
                .expect("refused")
                .contains("mute")
        );
        p.mute = true;
        assert!(prepare(&p, true, false).is_ok());
    }

    /// The whole path at the loop's rate: lead-in, performance, stop — the beak and head stay
    /// inside their limits at every tick and end at rest.
    #[test]
    fn a_performance_runs_at_fifty_hertz_and_ends_at_rest() {
        let (cmd, result) = prepare(&params(Some(2)), true, true).expect("accepted");
        assert!(result.accepted && result.duration_s > 30.0, "{result:?}");
        let Command::Start(job) = cmd else {
            panic!("a start")
        };
        let limits = job.show.profile;
        let t0 = Instant::now();
        let mut perf = Performer::new(job, t0);
        let mut last: Option<Frame> = None;
        let mut sang_something = false;
        let mut section_seen = false;
        for k in 0..2000u32 {
            let now = t0 + Duration::from_millis(20 * u64::from(k));
            let Some((f, st)) = perf.tick(now, 0.02) else {
                panic!("ended early at {k}")
            };
            sang_something |= f.mouth > 0.2;
            section_seen |= st.section.is_some();
            if let Some(l) = last {
                assert!(
                    ((f.mouth - l.mouth) / 0.02).abs() <= limits.mouth.max_vel * 1.001 + 1e-3,
                    "beak too fast at {k}"
                );
                for j in 0..4 {
                    assert!(
                        ((f.head[j] - l.head[j]) / 0.02).abs()
                            <= limits.head.max_vel[j] * 1.001 + 1e-3,
                        "head {j} too fast at {k}"
                    );
                }
            }
            last = Some(f);
        }
        assert!(sang_something && section_seen);
        perf.stop();
        let mut done_at = None;
        for k in 2000..2400u32 {
            let now = t0 + Duration::from_millis(20 * u64::from(k));
            if perf.tick(now, 0.02).is_none() {
                done_at = Some(k);
                break;
            }
        }
        assert!(
            done_at.is_some(),
            "a stopped performance settles and reports done"
        );
    }
}
