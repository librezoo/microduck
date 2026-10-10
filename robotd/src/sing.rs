//! A solo performance: the beak and the head following a compiled song, while the duck's own
//! voice sings along or the show's audio plays.
//!
//! The *what* lives in the `singer` crate — a [`singer::Show`] is the whole song's beak and head
//! motion, already feasible for the mechanism. This module is the part that belongs to the
//! daemon: it decides whether a request may start, compiles it **off the control thread** (a
//! three-minute song is tens of milliseconds of work, which a 20 ms tick cannot spare), keeps the
//! show clock, and hands the loop one frame a tick. The loop owns everything about the joints:
//! whether they may move, who else is moving them, and what a seek or a stop does to the
//! beak — that last one is the [`singer::Player`]'s job, and it never jumps a joint.
//!
//! ## The show clock
//!
//! Zero is the request, and the first [`PRE_ROLL_S`] seconds are a lead-in — beak shut, head
//! taking its breath — before song time zero. Everything on the clock is shifted by it together:
//! the performance, the audio, the extra head keyframes. `SingState::t_s` reports **song** time
//! (negative in the lead-in), which is what an app and a sequencer want to compare against.
//!
//! Pause holds the clock; resume and seek move it. The beak and the head go to rest while it is
//! held and are walked back to where the show says they should be afterwards.
//!
//! ## Ownership
//!
//! The theremin and the chorale each *are* the beak while they run, so a solo performance refuses
//! to start under either and yields to either that starts under it. One consent covers all three,
//! because all three move the beak and the head: `[chorale] accept`.
//!
//! Design: `docs/ideas/singing-performance.md` and `docs/ideas/kivi-show-integration.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use duck_ipc_proto as proto;
use singer::{Clock, Frame, Performance, Phase, Player, Profile, Show, Style, from_score};
use sounds::chorale::{Part, Score};

/// Seconds between the request and song time zero: the beak is shut and the head takes its breath.
/// Part of the performance, not a delay in front of it.
pub const PRE_ROLL_S: f32 = 1.5;

/// The head's absolute ceiling, performance and head clips together, radians. The show library's
/// own validation is stricter; this is the last line, and it clamps rather than refuses because
/// two layers legitimately add.
pub const HEAD_ABS_MAX: [f32; 4] = [0.20, 0.30, 0.85, 0.25];

/// The largest WAV the robot will decode into memory.
const MAX_AUDIO_BYTES: u64 = 160 << 20;

/// A WAV, decoded once at request time, off the control thread.
pub struct AudioJob {
    pub pcm: Arc<Vec<i16>>,
    pub rate: u32,
    pub channels: u8,
    /// Where the audio starts on the show clock, seconds (pre-roll included).
    pub start_s: f64,
}

/// What the control loop needs to start a performance.
pub struct Job {
    pub show: Show,
    /// The score the duck's own voice sings, with the line it sings. `None` for a performance
    /// handed in as data, whose sound — if any — comes from elsewhere.
    pub score: Option<(Score, Part)>,
    pub mute: bool,
    /// Where song time zero is on the show clock: the pre-roll.
    pub audio_start_s: f64,
    pub file: Option<AudioJob>,
    /// Extra head motion on the show clock: `(seconds, offsets)`.
    pub head_keys: Vec<(f64, [f32; 4])>,
}

/// A control for the performance that is running.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    Pause,
    Resume,
    /// Song time, seconds.
    Seek(f64),
    Volume(u8),
}

pub enum Command {
    Stop,
    Start(Box<Job>),
    Control(Control),
}

fn parse_part(name: &str) -> Option<Part> {
    Part::ALL.into_iter().find(|p| p.as_str() == name)
}

/// Decode a 16-bit PCM WAV that lies under `root`.
fn load_audio(path: &str, root: &Path, offset_ms: i64) -> Result<AudioJob, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("the show library {} is not readable: {e}", root.display()))?;
    let file = PathBuf::from(path)
        .canonicalize()
        .map_err(|e| format!("audio {path:?}: {e}"))?;
    if !file.starts_with(&root) {
        return Err(format!(
            "audio {path:?} is outside the show library {}",
            root.display()
        ));
    }
    let len = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
    if len > MAX_AUDIO_BYTES {
        return Err(format!(
            "audio is {len} bytes; the limit is {MAX_AUDIO_BYTES}"
        ));
    }
    let mut reader = hound::WavReader::open(&file).map_err(|e| format!("audio: {e}"))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err("audio must be 16-bit PCM".to_owned());
    }
    if !(1..=2).contains(&spec.channels) || !(8_000..=96_000).contains(&spec.sample_rate) {
        return Err("audio must be 1–2 channels at 8–96 kHz".to_owned());
    }
    let pcm: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("audio: {e}"))?;
    Ok(AudioJob {
        pcm: Arc::new(pcm),
        rate: spec.sample_rate,
        channels: spec.channels as u8,
        start_s: f64::from(PRE_ROLL_S) + offset_ms as f64 / 1000.0,
    })
}

/// Validate and compile a request. Everything that can be refused is refused here, at the door,
/// with the reason — the control loop is handed only something it can run.
pub fn prepare(
    p: &proto::SingParams,
    opted_in: bool,
    has_voice: bool,
    show_root: &Path,
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
                .ok_or_else(|| format!("unknown style {name:?} — have ballad, pop, chant, lively")),
        }
    };

    let (perf, score) = match &p.performance {
        Some(json) => {
            let perf = Performance::from_json(json).map_err(|e| e.to_string())?;
            (perf.with_pre_roll(PRE_ROLL_S), None)
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
            (perf, Some((score, part)))
        }
    };
    if !p.mute && (score.is_some() || p.audio.is_some()) && !has_voice {
        return Err(
            "this robot has no voice to sing with — ask for `mute` to move the beak and head only"
                .to_owned(),
        );
    }
    let file = match &p.audio {
        Some(a) => Some(load_audio(&a.path, show_root, a.start_offset_ms)?),
        None => None,
    };
    if file.is_some() && score.is_some() {
        return Err("a piece sings itself; give either `piece` or `audio`, not both".to_owned());
    }
    let mut head_keys: Vec<(f64, [f32; 4])> = Vec::with_capacity(p.head_keys.len());
    for k in &p.head_keys {
        let v = k.offsets.map(|x| x as f32);
        if v.iter()
            .zip(HEAD_ABS_MAX)
            .any(|(x, m)| !x.is_finite() || x.abs() > m)
        {
            return Err(format!("a head keyframe is beyond ±{HEAD_ABS_MAX:?} rad"));
        }
        head_keys.push((k.t_ms as f64 / 1000.0 + f64::from(PRE_ROLL_S), v));
    }
    if head_keys.windows(2).any(|w| w[1].0 < w[0].0) {
        return Err("head keyframes must be in time order".to_owned());
    }
    let mut style = style_by_name(&perf)?;
    if let Some(scale) = p.head_scale {
        if !(0.0..=5.0).contains(&scale) {
            return Err(format!("a head scale of {scale} is outside 0..5"));
        }
        style = style.scaled(1.0, scale as f32);
    }
    let style_name = style.name.clone();
    let mut profile = Profile::default();
    if style.name == "lively" {
        profile.head = singer::profile::HeadProfile::lively();
    }
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
    // A bigger head asks for a bigger budget too, or the soft limit just flattens it; the absolute
    // ceiling above is still the last word.
    if let Some(scale) = p.head_scale.filter(|s| *s > 1.0) {
        for (b, cap) in profile.head.budget.iter_mut().zip(HEAD_ABS_MAX) {
            *b = (*b * scale as f32).min(cap);
        }
    }
    let show = Show::compile(perf, style, profile).map_err(|e| e.to_string())?;
    let result = proto::SingResult {
        accepted: true,
        reason: None,
        name: Some(show.perf.name.clone()),
        duration_s: f64::from(show.duration()) - f64::from(PRE_ROLL_S),
        style: Some(style_name),
    };
    Ok((
        Command::Start(Box::new(Job {
            show,
            score,
            mute: p.mute,
            audio_start_s: f64::from(PRE_ROLL_S),
            file,
            head_keys,
        })),
        result,
    ))
}

/// What the loop should do to the speaker this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioOp {
    Start { frame: usize },
    Pause(bool),
    Seek { frame: usize },
    Stop,
}

/// A running performance.
pub struct Performer {
    player: Player,
    score: Option<(Score, Part)>,
    mute: bool,
    audio_start_s: f64,
    file: Option<AudioJob>,
    head_keys: Vec<(f64, [f32; 4])>,
    /// The show clock is `now − base`, unless it is held.
    base: Instant,
    held: Option<f64>,
    stopping: bool,
    name: String,
    // The file audio's bookkeeping, so the loop is told only what changed.
    file_started: bool,
    file_ended: bool,
    file_stop_sent: bool,
    file_paused_sent: bool,
    pending_seek: bool,
}

impl Performer {
    pub fn new(job: Box<Job>, now: Instant) -> Self {
        let name = job.show.perf.name.clone();
        Self {
            player: Player::new(job.show),
            score: job.score,
            mute: job.mute,
            audio_start_s: job.audio_start_s,
            file: job.file,
            head_keys: job.head_keys,
            base: now,
            held: None,
            stopping: false,
            name,
            file_started: false,
            file_ended: false,
            file_stop_sent: false,
            file_paused_sent: false,
            pending_seek: false,
        }
    }

    /// Wind down: the beak closes and the head settles, then [`Self::tick`] reports done.
    pub fn stop(&mut self) {
        self.stopping = true;
    }

    fn show_time(&self, now: Instant) -> f64 {
        self.held
            .unwrap_or_else(|| now.duration_since(self.base).as_secs_f64())
    }

    /// Apply a control. `Err` is a reason to hand back to the caller.
    pub fn control(&mut self, c: Control, now: Instant) -> Result<(), String> {
        let t = self.show_time(now);
        match c {
            Control::Pause => {
                if self.held.is_none() {
                    self.held = Some(t);
                }
            }
            Control::Resume => {
                if let Some(held) = self.held.take() {
                    self.base = now - std::time::Duration::from_secs_f64(held.max(0.0));
                }
            }
            Control::Seek(song_s) => {
                let target = song_s + self.audio_start_s;
                let end = f64::from(self.player.show().duration());
                if !(0.0..=end).contains(&target) || !song_s.is_finite() {
                    return Err(format!("position {song_s} s is outside the song"));
                }
                if self.held.is_some() {
                    self.held = Some(target);
                } else {
                    self.base = now - std::time::Duration::from_secs_f64(target);
                }
                self.pending_seek = true;
            }
            Control::Volume(_) => {}
        }
        Ok(())
    }

    /// The line to sing and where in it, in beats — `None` before the music starts, after it
    /// ends, when muted, paused, or when the sound is somebody else's.
    pub fn audio(&self, now: Instant) -> Option<(&Score, Part, f64)> {
        if self.mute || self.stopping || self.held.is_some() {
            return None;
        }
        let (score, part) = self.score.as_ref()?;
        let t = self.show_time(now) - self.audio_start_s;
        let beats = t / score.beat_s();
        (t >= 0.0 && beats * score.beat_s() <= score.duration_s()).then_some((score, *part, beats))
    }

    /// Whether the voice should be silent now (stopped, paused, or the music has ended).
    pub fn audio_over(&self, now: Instant) -> bool {
        self.score.is_some() && self.audio(now).is_none()
    }

    /// The file audio, if there is one and it is not muted.
    pub fn file(&self) -> Option<&AudioJob> {
        self.file.as_ref().filter(|_| !self.mute)
    }

    /// The loop saw the writer reach the end of the file.
    pub fn file_ended(&mut self) {
        self.file_started = false;
        self.file_ended = true;
    }

    /// What to do to the speaker this tick. The loop applies each op to its `Sound`.
    pub fn audio_ops(&mut self, now: Instant) -> Vec<AudioOp> {
        let mut ops = Vec::new();
        let Some(f) = self.file() else {
            return ops;
        };
        let (rate, start_s) = (f64::from(f.rate), f.start_s);
        let paused = self.held.is_some();
        if self.stopping {
            if (self.file_started || !self.file_stop_sent) && !self.file_stop_sent {
                ops.push(AudioOp::Stop);
                self.file_stop_sent = true;
                self.file_started = false;
            }
            return ops;
        }
        if self.file_ended {
            // After a seek back into the file it plays again.
            if !self.pending_seek {
                return ops;
            }
            self.file_ended = false;
        }
        let t_file = self.show_time(now) - start_s;
        if self.pending_seek {
            self.pending_seek = false;
            if self.file_started {
                if t_file >= 0.0 {
                    ops.push(AudioOp::Seek {
                        frame: (t_file * rate) as usize,
                    });
                } else {
                    ops.push(AudioOp::Stop);
                    self.file_started = false;
                }
            }
        }
        if !self.file_started && !paused && t_file >= 0.0 {
            ops.push(AudioOp::Start {
                frame: (t_file * rate) as usize,
            });
            self.file_started = true;
            self.file_paused_sent = false;
        } else if self.file_started && paused != self.file_paused_sent {
            ops.push(AudioOp::Pause(paused));
            self.file_paused_sent = paused;
        }
        ops
    }

    fn extra_head(&self, t: f64) -> [f32; 4] {
        let keys = &self.head_keys;
        match keys.len() {
            0 => [0.0; 4],
            _ if t <= keys[0].0 => keys[0].1,
            _ if t >= keys[keys.len() - 1].0 => keys[keys.len() - 1].1,
            _ => {
                let i = keys.partition_point(|k| k.0 <= t);
                let (a, b) = (&keys[i - 1], &keys[i]);
                let f = ((t - a.0) / (b.0 - a.0).max(1e-6)) as f32;
                [0, 1, 2, 3].map(|j| a.1[j] + (b.1[j] - a.1[j]) * f)
            }
        }
    }

    /// One control tick. `None` when the performance is over and the joints are at rest.
    pub fn tick(&mut self, now: Instant, dt: f32) -> Option<(Frame, proto::SingState)> {
        let t = self.show_time(now);
        let clock = if self.stopping {
            Clock::Stopped
        } else if self.held.is_some() {
            Clock::Paused
        } else {
            Clock::Playing(t)
        };
        let mut frame = self.player.tick(dt, clock);
        // A head clip is not the singer's to slew: it was validated at the door and it is added
        // on top, bounded by the absolute ceiling — and only while the show is actually playing.
        if matches!(clock, Clock::Playing(_)) {
            let extra = self.extra_head(t);
            for j in 0..4 {
                frame.head[j] = (frame.head[j] + extra[j]).clamp(-HEAD_ABS_MAX[j], HEAD_ABS_MAX[j]);
            }
        }
        let past_end = t > f64::from(self.player.show().duration());
        if self.player.phase() == Phase::Idle && (self.stopping || past_end) {
            return None;
        }
        let show = self.player.show();
        let phase = match self.player.phase() {
            Phase::Releasing => "settling",
            _ if self.held.is_some() => "paused",
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
                paused: self.held.is_some(),
                audio_s: None,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn root() -> PathBuf {
        std::env::temp_dir()
    }

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
            head_scale: None,
            audio: None,
            head_keys: Vec::new(),
        }
    }

    fn start(p: &proto::SingParams) -> Box<Job> {
        match prepare(p, true, true, &root()).expect("accepted").0 {
            Command::Start(job) => job,
            _ => panic!("a start"),
        }
    }

    #[test]
    fn a_robot_that_has_not_opted_in_refuses_and_says_why() {
        let err = prepare(&params(Some(2)), false, true, &root())
            .err()
            .expect("refused");
        assert!(err.contains("[chorale] accept"), "{err}");
    }

    #[test]
    fn stopping_is_never_refused() {
        let mut p = params(None);
        p.active = false;
        assert!(matches!(
            prepare(&p, false, false, &root()),
            Ok((Command::Stop, _))
        ));
    }

    #[test]
    fn bad_requests_are_refused_at_the_door_with_what_the_robot_has() {
        let e = prepare(&params(Some(99)), true, true, &root())
            .err()
            .expect("refused");
        assert!(e.contains("duck-strut"), "names the catalogue: {e}");
        let mut p = params(Some(2));
        p.part = Some("kazoo".into());
        assert!(
            prepare(&p, true, true, &root())
                .err()
                .expect("refused")
                .contains("soprano")
        );
        let mut p = params(Some(2));
        p.style = Some("opera".into());
        assert!(
            prepare(&p, true, true, &root())
                .err()
                .expect("refused")
                .contains("ballad")
        );
        let mut p = params(None);
        p.performance = Some("{\"nope\": 1}".into());
        assert!(prepare(&p, true, true, &root()).is_err());
    }

    #[test]
    fn a_voiceless_robot_can_still_perform_muted_but_not_sing() {
        let mut p = params(Some(2));
        p.mute = false;
        assert!(
            prepare(&p, true, false, &root())
                .err()
                .expect("refused")
                .contains("mute")
        );
        p.mute = true;
        assert!(prepare(&p, true, false, &root()).is_ok());
    }

    /// The whole path at the loop's rate: lead-in, performance, stop — the beak and head stay
    /// inside their limits at every tick and end at rest.
    #[test]
    fn a_performance_runs_at_fifty_hertz_and_ends_at_rest() {
        let (cmd, result) = prepare(&params(Some(2)), true, true, &root()).expect("accepted");
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

    /// Pause holds the clock and sends the joints to rest; resume carries on from where it was,
    /// and a seek moves the clock — none of them jumps a joint.
    #[test]
    fn pause_resume_and_seek_move_the_clock_and_not_the_joints() {
        let mut perf = Performer::new(start(&params(Some(2))), Instant::now());
        let t0 = Instant::now();
        perf.base = t0;
        let limits = perf.player.show().profile;
        let mut last: Option<Frame> = None;
        let mut now = t0;
        let mut step = |perf: &mut Performer, now: &mut Instant, ticks: u32| {
            let mut state = None;
            for _ in 0..ticks {
                *now += Duration::from_millis(20);
                let (f, st) = perf.tick(*now, 0.02).expect("running");
                if let Some(l) = last {
                    assert!(
                        ((f.mouth - l.mouth) / 0.02).abs() <= limits.mouth.max_vel * 1.02 + 1e-3
                    );
                    for j in 0..4 {
                        assert!(
                            ((f.head[j] - l.head[j]) / 0.02).abs()
                                <= limits.head.max_vel[j] * 1.02 + 1e-3
                        );
                    }
                }
                last = Some(f);
                state = Some(st);
            }
            state.expect("ticks")
        };
        let before = step(&mut perf, &mut now, 500); // 10 s in
        assert!(before.t_s > 8.0 && !before.paused);
        perf.control(Control::Pause, now).unwrap();
        let held = step(&mut perf, &mut now, 200);
        assert!(held.paused && held.phase != "performing");
        assert!(
            (held.t_s - before.t_s).abs() < 0.1,
            "the clock is held: {} vs {}",
            held.t_s,
            before.t_s
        );
        perf.control(Control::Resume, now).unwrap();
        let after = step(&mut perf, &mut now, 50);
        assert!(
            !after.paused && (after.t_s - before.t_s).abs() < 1.5,
            "{} vs {}",
            after.t_s,
            before.t_s
        );
        perf.control(Control::Seek(40.0), now).unwrap();
        let there = step(&mut perf, &mut now, 5);
        assert!((there.t_s - 40.1).abs() < 0.5, "{}", there.t_s);
        assert!(perf.control(Control::Seek(9999.0), now).is_err());
    }

    #[test]
    fn head_clips_add_to_the_performance_and_are_bounded() {
        let mut p = params(Some(2));
        p.head_keys = vec![
            proto::SingHeadKey {
                t_ms: 1000,
                offsets: [0.0, 0.0, 0.0, 0.0],
            },
            proto::SingHeadKey {
                t_ms: 2000,
                offsets: [0.0, 0.0, 0.45, 0.0],
            },
        ];
        let mut perf = Performer::new(start(&p), Instant::now());
        let t0 = Instant::now();
        perf.base = t0;
        let mut yaw = 0.0f32;
        for k in 0..400u32 {
            let (f, _) = perf
                .tick(t0 + Duration::from_millis(20 * u64::from(k)), 0.02)
                .unwrap();
            yaw = yaw.max(f.head[2]);
        }
        assert!(yaw > 0.30 && yaw <= HEAD_ABS_MAX[2] + 1e-6, "{yaw}");
        p.head_keys[1].offsets[2] = 0.9;
        assert!(
            prepare(&p, true, true, &root())
                .err()
                .expect("refused")
                .contains("beyond")
        );
    }

    #[test]
    fn audio_is_confined_to_the_show_library() {
        let mut p = params(None);
        p.performance = Some(singer::from_score(&Score::wistful(), Part::Soprano).to_json());
        p.mute = false;
        p.audio = Some(proto::SingAudio {
            path: "/etc/passwd".into(),
            start_offset_ms: 0,
        });
        let err = prepare(&p, true, true, &root()).err().expect("refused");
        assert!(
            err.contains("outside the show library") || err.contains("audio"),
            "{err}"
        );
    }

    /// The speaker is told to start when the audio's own time comes, to pause and resume with the
    /// clock, to jump on a seek, and to stop — each once.
    #[test]
    fn the_audio_ops_follow_the_clock() {
        let dir = tempfile_dir();
        let wav = dir.join("a.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..80_000 {
            w.write_sample(((i % 100) as i16) * 10).unwrap();
        }
        w.finalize().unwrap();
        let mut p = params(None);
        p.performance = Some(singer::from_score(&Score::wistful(), Part::Soprano).to_json());
        p.mute = false;
        p.audio = Some(proto::SingAudio {
            path: wav.to_string_lossy().into_owned(),
            start_offset_ms: 500,
        });
        let job = match prepare(&p, true, true, &dir).expect("accepted").0 {
            Command::Start(j) => j,
            _ => panic!(),
        };
        let t0 = Instant::now();
        let mut perf = Performer::new(job, t0);
        assert!(perf.audio_ops(t0).is_empty(), "not before its time");
        // pre-roll 1.5 s + offset 0.5 s
        let ops = perf.audio_ops(t0 + Duration::from_millis(2100));
        assert!(
            matches!(ops.as_slice(), [AudioOp::Start { frame }] if (*frame as i64 - 800).abs() < 20),
            "{ops:?}"
        );
        assert!(
            perf.audio_ops(t0 + Duration::from_millis(2200)).is_empty(),
            "once"
        );
        perf.control(Control::Pause, t0 + Duration::from_millis(3000))
            .unwrap();
        assert_eq!(
            perf.audio_ops(t0 + Duration::from_millis(3020)),
            vec![AudioOp::Pause(true)]
        );
        perf.control(Control::Resume, t0 + Duration::from_millis(5000))
            .unwrap();
        assert_eq!(
            perf.audio_ops(t0 + Duration::from_millis(5020)),
            vec![AudioOp::Pause(false)]
        );
        perf.control(Control::Seek(5.0), t0 + Duration::from_millis(5100))
            .unwrap();
        let ops = perf.audio_ops(t0 + Duration::from_millis(5120));
        assert!(
            matches!(ops.as_slice(), [AudioOp::Seek { frame }] if (*frame as i64 - 36_000).abs() < 400),
            "{ops:?}"
        );
        perf.stop();
        assert_eq!(
            perf.audio_ops(t0 + Duration::from_millis(6000)),
            vec![AudioOp::Stop]
        );
        assert!(perf.audio_ops(t0 + Duration::from_millis(6100)).is_empty());
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sing-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
