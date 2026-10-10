//! The service's brain: the library, the robot, and what is playing.
//!
//! HTTP is a thin layer over this (`api.rs`); nothing here knows about it, so the playback logic
//! is tested against a [`FakeRobot`] and runs unchanged against `robotd`.
//!
//! ## What `showd` does and does not do about motion
//!
//! The beak, the head and the voice are `robotd`'s: it compiles the performance and runs it on its
//! own 50 Hz clock, and so do the `head` clips of a motion track. What `showd` sequences itself are
//! the **coarse** actions — a skill, a body offset, a walk — because those are `robot.do`,
//! `robot.pose` and `robot.move` calls at the right moment, timed from the robot's own song clock
//! (`sing.t_s`), never from showd's.
//!
//! Every one of them is checked twice: the library validated the whole track at upload, and the
//! sequencer checks the *real* pose before it starts each skill. A pose that is not the one the
//! track assumed stops the show and says why; nothing is retried and nothing is guessed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use duck_ipc_proto as proto;
use serde::Serialize;
use show::{
    Action, ActionKind, Capabilities, DuckScore, MotionTrack, Pose, Problem, Problems, SkillInfo,
};

use crate::robot::RobotLink;
use crate::store::{Entry, Store};

pub const PROTOCOL: u32 = 1;

#[derive(Debug, Clone)]
pub struct Config {
    pub device_id: String,
    pub name: String,
    pub mouth_latency_ms: f64,
    pub head_latency_ms: f64,
    pub audio_latency_ms: f64,
    /// Scale of every head amplitude (1.0 = the style's own).
    pub head_scale: f64,
    pub max_show_bytes: u64,
    pub max_events: usize,
    pub volume_file: std::path::PathBuf,
}

/// An error an HTTP caller sees: a status, a stable code, a sentence, and the details.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub problems: Vec<Problem>,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            problems: Vec::new(),
        }
    }

    pub fn from_problems(p: Problems) -> Self {
        let first = p.0.first().map(|x| x.code).unwrap_or("Corrupt");
        let status = match first {
            "Missing" => 409,
            "TooLarge" => 413,
            _ => 422,
        };
        Self {
            status,
            code: first,
            message: p.to_string(),
            problems: p.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    pub voice: String,
    pub revision: String,
    pub duration: f64,
    pub bytes: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackView {
    pub song_id: Option<String>,
    pub revision: Option<String>,
    pub status: &'static str,
    pub position: f64,
    pub duration: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub protocol: u32,
    pub device_id: String,
    pub name: String,
    pub boot_id: String,
    pub sequence: u64,
    pub version: u64,
    pub songs: Vec<Song>,
    pub volume: u8,
    pub playback: PlaybackView,
}

/// A coarse action with the pose it needs and leaves.
#[derive(Debug, Clone)]
struct Step {
    action: Action,
    start_s: f64,
    end_s: f64,
    before: Pose,
    after: Pose,
}

#[derive(Debug, Clone)]
struct Timeline {
    steps: Vec<Step>,
}

impl Timeline {
    fn build(motion: &MotionTrack, start: Pose) -> Result<Self, Problems> {
        let mut pose = start;
        let mut steps = Vec::new();
        let mut p = Problems::default();
        for a in &motion.actions {
            let before = pose;
            match &a.kind {
                ActionKind::Head { .. } => continue,
                ActionKind::Skill { skill } => match pose.after_skill(skill, "") {
                    Ok(next) => pose = next,
                    Err(_) if skill == "sit_toggle" => {
                        p.push(
                            Problem::new(
                                "PoseMismatch",
                                "the robot is in no pose sit_toggle can start from",
                            )
                            .at(&a.event_id),
                        );
                    }
                    Err(need) => {
                        // Skills other than sit_toggle: the manifest says what they need, which
                        // the library already checked; here only the pose bookkeeping matters.
                        let _ = need;
                    }
                },
                ActionKind::Body { .. } | ActionKind::Walk { .. } => {}
            }
            steps.push(Step {
                start_s: a.start_ms as f64 / 1000.0,
                end_s: (a.start_ms + a.duration_ms) as f64 / 1000.0,
                before,
                after: pose,
                action: a.clone(),
            });
        }
        p.into_result()?;
        Ok(Self { steps })
    }

    /// The pose the robot should be in at song time `t`.
    fn pose_at(&self, start: Pose, t: f64) -> Pose {
        let mut pose = start;
        for s in &self.steps {
            if t >= s.end_s {
                pose = s.after;
            }
        }
        pose
    }

    /// The coarse action running at `t`, if any.
    fn active_at(&self, t: f64) -> Option<&Step> {
        self.steps.iter().find(|s| t >= s.start_s && t < s.end_s)
    }
}

struct Current {
    show_id: String,
    revision: String,
    duration_s: f64,
    started: Instant,
    stop_requested: bool,
    seen_sing: bool,
    cancel: Arc<AtomicBool>,
    timeline: Option<Timeline>,
    start_pose: Pose,
    last_position: f64,
}

struct Inner {
    cur: Option<Current>,
    /// What the last performance came to, until the next one starts.
    last: Option<PlaybackView>,
    volume: u8,
}

pub struct App {
    pub cfg: Config,
    pub store: Store,
    pub robot: Arc<dyn RobotLink>,
    pub boot_id: String,
    inner: Mutex<Inner>,
    sequence: AtomicU64,
    version: AtomicU64,
    last_key: Mutex<String>,
}

fn pose_of(st: &proto::RobotState) -> Pose {
    if st.safety.fallen || st.safety.limp || st.safety.picked_up {
        return Pose::Unknown;
    }
    let p = st.policy.as_str();
    if p.contains("sit") {
        Pose::Sitting
    } else if p.contains("stand") || p.contains("walk") {
        Pose::Standing
    } else {
        Pose::Unknown
    }
}

impl App {
    pub fn new(cfg: Config, store: Store, robot: Arc<dyn RobotLink>) -> Arc<Self> {
        let volume = std::fs::read_to_string(&cfg.volume_file)
            .ok()
            .and_then(|v| v.trim().parse::<u8>().ok())
            .filter(|v| *v <= 100)
            .unwrap_or(70);
        let boot_id = {
            let mut b = [0u8; 8];
            getrandom::fill(&mut b).expect("randomness");
            b.iter().map(|x| format!("{x:02x}")).collect()
        };
        Arc::new(Self {
            cfg,
            store,
            robot,
            boot_id,
            inner: Mutex::new(Inner {
                cur: None,
                last: None,
                volume,
            }),
            sequence: AtomicU64::new(1),
            version: AtomicU64::new(1),
            last_key: Mutex::new(String::new()),
        })
    }

    pub fn volume(&self) -> u8 {
        self.inner.lock().expect("not poisoned").volume
    }

    /// Re-apply the saved volume to the robot. Called at start, and whenever the link comes back.
    pub async fn apply_volume(&self) {
        let v = self.volume();
        let _ = self
            .robot
            .call(proto::Call::RobotSingControl(proto::SingControlParams {
                action: proto::SingAction::Volume,
                position_s: None,
                volume: Some(v),
            }))
            .await;
    }

    pub async fn capabilities(&self) -> Capabilities {
        let (head, body, walk) = Capabilities::default_limits();
        let mut skills = Vec::new();
        if let Ok(v) = self.robot.call(proto::Call::RobotSkills).await
            && let Ok(r) = serde_json::from_value::<proto::SkillsResult>(v)
        {
            for s in r.skills {
                skills.push(SkillInfo {
                    duration_ms: (s.duration.unwrap_or(0.0) * 1000.0) as u64,
                    name: s.name,
                    entry_pose: "standing".into(),
                });
            }
            for b in r.built_in {
                let entry = if b == "sit_toggle" { "any" } else { "standing" };
                if !skills.iter().any(|s| s.name == b) {
                    skills.push(SkillInfo {
                        name: b,
                        duration_ms: 0,
                        entry_pose: entry.into(),
                    });
                }
            }
        }
        let st = self.robot.state();
        Capabilities {
            protocol: PROTOCOL,
            device_id: self.cfg.device_id.clone(),
            model_id: "microduck-v1".into(),
            skills,
            head,
            body,
            walk,
            max_show_bytes: self.cfg.max_show_bytes,
            max_events: self.cfg.max_events,
            audio_roles: vec!["mix".into()],
            pose: st
                .as_ref()
                .map(|(s, _)| pose_of(s))
                .unwrap_or(Pose::Unknown),
            battery_percent: None,
        }
    }

    // ── what the library holds ───────────────────────────────────────────────

    fn song_of(e: &Entry) -> Song {
        Song {
            id: e.manifest.show_id.clone(),
            title: e.manifest.title.clone(),
            voice: "duck".into(),
            revision: e.meta.revision.clone(),
            duration: e.manifest.duration_ms as f64 / 1000.0,
            bytes: e.meta.total_bytes,
            updated_at: e.meta.updated_at_ms,
        }
    }

    pub async fn commit(&self, id: &str) -> Result<Song, ApiError> {
        // Not while it is playing: the files under a playing show must not change.
        self.refresh();
        if self.is_current(id) {
            return Err(ApiError::new(
                409,
                "Busy",
                "this show is playing; stop it before updating it",
            ));
        }
        let caps = self.capabilities().await;
        let pose = caps.pose;
        let entry = self
            .store
            .commit(id, &caps, pose)
            .map_err(ApiError::from_problems)?;
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(Self::song_of(&entry))
    }

    fn is_current(&self, id: &str) -> bool {
        self.inner
            .lock()
            .expect("not poisoned")
            .cur
            .as_ref()
            .is_some_and(|c| c.show_id == id)
    }

    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.refresh();
        if self.is_current(id) {
            return Err(ApiError::new(
                409,
                "Busy",
                "this show is playing; stop it first",
            ));
        }
        if !self.store.delete(id) {
            return Err(ApiError::new(404, "NotFound", "no such show"));
        }
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    // ── playing ──────────────────────────────────────────────────────────────

    pub async fn play(
        self: &Arc<Self>,
        id: &str,
        revision: Option<&str>,
        mute: bool,
        head_scale: Option<f64>,
    ) -> Result<PlaybackView, ApiError> {
        let entry = self
            .store
            .get(id)
            .ok_or_else(|| ApiError::new(404, "NotFound", "no such show"))?;
        if let Some(r) = revision
            && !r.is_empty()
            && r != entry.meta.revision
        {
            return Err(ApiError::new(
                409,
                "Stale",
                "the show on the robot is not the revision you asked for",
            ));
        }
        let (view, running) = self.refresh();
        if running {
            return Err(ApiError::new(
                409,
                "Busy",
                format!("{:?} is already playing; stop it first", view.song_id),
            ));
        }
        let caps = self.capabilities().await;
        if caps.pose == Pose::Unknown && self.robot.state().is_some() {
            // Not an error for a show that only sings; a motion track decides below.
        }
        let manifest = &entry.manifest;
        let duration_s = (manifest.duration_ms as f32 / 1000.0).max(0.5);

        // The motion track, checked against the pose the robot is *actually* in.
        let start_pose = manifest
            .start_pose
            .as_deref()
            .map(Pose::from_name)
            .unwrap_or(caps.pose);
        let mut motion: Option<MotionTrack> = None;
        if manifest.tracks.motion.is_some() {
            let bytes = std::fs::read(entry.dir.join("motion.json"))
                .map_err(|e| ApiError::new(500, "Io", e.to_string()))?;
            let m = MotionTrack::parse(&bytes).map_err(ApiError::from_problems)?;
            if m.actions
                .iter()
                .any(|a| !matches!(a.kind, ActionKind::Head { .. }))
            {
                if caps.pose == Pose::Unknown {
                    return Err(ApiError::new(
                        409,
                        "PoseMismatch",
                        "the robot is not standing or sitting (limp, fallen, lifted or not ready)",
                    ));
                }
                if let Some(declared) = manifest.start_pose.as_deref().map(Pose::from_name)
                    && declared != caps.pose
                {
                    return Err(ApiError::new(
                        409,
                        "PoseMismatch",
                        format!(
                            "the show starts {}; the robot is {}",
                            declared.name(),
                            caps.pose.name()
                        ),
                    ));
                }
            }
            m.validate(&caps, start_pose)
                .map_err(ApiError::from_problems)?;
            motion = Some(m);
        }

        // The performance: from the duck track, or a silent one for audio only.
        let perf_json = match &manifest.tracks.duck {
            Some(_) => {
                let bytes = std::fs::read(entry.dir.join("duck.score.json"))
                    .map_err(|e| ApiError::new(500, "Io", e.to_string()))?;
                let min =
                    duration_s.max(motion.as_ref().map_or(0.0, |m| m.end_ms() as f32 / 1000.0));
                DuckScore::parse(&bytes)
                    .and_then(|s| s.to_performance_for(&manifest.title, min))
                    .map_err(ApiError::from_problems)?
                    .to_json()
            }
            None => silent_performance(
                &manifest.title,
                duration_s.max(motion.as_ref().map_or(0.0, |m| m.end_ms() as f32 / 1000.0)),
                manifest.tempo_bpm,
            ),
        };
        let head_keys: Vec<proto::SingHeadKey> = motion
            .as_ref()
            .map(|m| {
                m.head_keys()
                    .into_iter()
                    .map(|(t_ms, v)| proto::SingHeadKey {
                        t_ms,
                        offsets: v.map(f64::from),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let audio = entry
            .audio_path()
            .zip(manifest.tracks.audio.as_ref())
            .map(|(p, a)| proto::SingAudio {
                path: p.to_string_lossy().into_owned(),
                start_offset_ms: a.start_offset_ms,
            });
        let params = proto::SingParams {
            active: true,
            piece: None,
            part: None,
            style: None,
            mute,
            performance: Some(perf_json),
            mouth_latency_ms: Some(self.cfg.mouth_latency_ms),
            head_latency_ms: Some(self.cfg.head_latency_ms),
            audio_latency_ms: Some(self.cfg.audio_latency_ms),
            head_scale: Some(head_scale.unwrap_or(self.cfg.head_scale)),
            audio,
            head_keys,
        };
        let timeline = match &motion {
            Some(m) => Some(Timeline::build(m, start_pose).map_err(ApiError::from_problems)?),
            None => None,
        };
        let answer = self
            .robot
            .call(proto::Call::RobotSing(params))
            .await
            .map_err(|e| ApiError::new(502, "RobotUnavailable", e))?;
        let result: proto::SingResult = serde_json::from_value(answer)
            .map_err(|e| ApiError::new(502, "RobotUnavailable", e.to_string()))?;
        if !result.accepted {
            let reason = result
                .reason
                .unwrap_or_else(|| "the robot refused".to_owned());
            let code = if reason.contains("opted in") {
                "NotEnabled"
            } else if reason.contains("has the beak") {
                "Busy"
            } else {
                "Refused"
            };
            return Err(ApiError::new(409, code, reason));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut inner = self.inner.lock().expect("not poisoned");
            inner.last = None;
            inner.cur = Some(Current {
                show_id: id.to_owned(),
                revision: entry.meta.revision.clone(),
                duration_s: f64::from(duration_s),
                started: Instant::now(),
                stop_requested: false,
                seen_sing: false,
                cancel: cancel.clone(),
                timeline: timeline.clone(),
                start_pose,
                last_position: 0.0,
            });
        }
        self.version.fetch_add(1, Ordering::Relaxed);
        if let Some(tl) = timeline.filter(|t| !t.steps.is_empty()) {
            let app = self.clone();
            tokio::spawn(async move { app.sequence(tl, cancel).await });
        }
        Ok(self.refresh().0)
    }

    pub async fn stop(&self) -> Result<PlaybackView, ApiError> {
        let had = {
            let mut inner = self.inner.lock().expect("not poisoned");
            match inner.cur.as_mut() {
                Some(c) => {
                    c.stop_requested = true;
                    c.cancel.store(true, Ordering::Relaxed);
                    true
                }
                None => false,
            }
        };
        let _ = self.robot.call(proto::Call::RobotSing(stop_params())).await;
        if had {
            // Whatever the sequencer was driving, let go of it.
            self.robot.notify(proto::Call::RobotPose(proto::PoseParams {
                active: false,
                ..Default::default()
            }));
            self.robot.notify(proto::Call::RobotMove(proto::MoveParams {
                vx: 0.0,
                vy: 0.0,
                vyaw: 0.0,
            }));
        }
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(self.refresh().0)
    }

    pub async fn pause(&self) -> Result<PlaybackView, ApiError> {
        self.require_boundary("pause")?;
        self.sing_control(proto::SingAction::Pause, None).await?;
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(self.refresh().0)
    }

    pub async fn resume(&self) -> Result<PlaybackView, ApiError> {
        self.sing_control(proto::SingAction::Resume, None).await?;
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(self.refresh().0)
    }

    pub async fn seek(&self, to: f64) -> Result<PlaybackView, ApiError> {
        {
            let inner = self.inner.lock().expect("not poisoned");
            let Some(cur) = inner.cur.as_ref() else {
                return Err(ApiError::new(409, "NotPlaying", "nothing is playing"));
            };
            if let Some(tl) = &cur.timeline {
                let now = cur.last_position;
                if let Some(step) = tl.active_at(to) {
                    return Err(ApiError::new(
                        409,
                        "PoseMismatch",
                        format!(
                            "position {to:.1} s is inside the action {:?}; seek to its start or end",
                            step.action.event_id
                        ),
                    ));
                }
                if tl.pose_at(cur.start_pose, to) != tl.pose_at(cur.start_pose, now) {
                    return Err(ApiError::new(
                        409,
                        "PoseMismatch",
                        "seeking there would skip an action that changes the robot's pose (a sit or a stand)",
                    ));
                }
            }
        }
        self.require_boundary("seek")?;
        self.sing_control(proto::SingAction::Seek, Some(to)).await?;
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(self.refresh().0)
    }

    pub async fn set_volume(&self, v: u8) -> Result<(), ApiError> {
        if v > 100 {
            return Err(ApiError::new(400, "BadRequest", "volume is 0..100"));
        }
        self.inner.lock().expect("not poisoned").volume = v;
        let _ = std::fs::write(&self.cfg.volume_file, v.to_string());
        self.apply_volume().await;
        self.version.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Pause and seek are only allowed between coarse actions: a skill cannot be frozen halfway
    /// without freezing the policy that is balancing it.
    fn require_boundary(&self, what: &str) -> Result<(), ApiError> {
        let inner = self.inner.lock().expect("not poisoned");
        let Some(cur) = inner.cur.as_ref() else {
            return Err(ApiError::new(409, "NotPlaying", "nothing is playing"));
        };
        if let Some(tl) = &cur.timeline
            && let Some(step) = tl.active_at(cur.last_position)
        {
            return Err(ApiError::new(
                409,
                "Busy",
                format!(
                    "cannot {what} during the action {:?}; try again when it ends",
                    step.action.event_id
                ),
            ));
        }
        Ok(())
    }

    async fn sing_control(
        &self,
        action: proto::SingAction,
        position_s: Option<f64>,
    ) -> Result<(), ApiError> {
        let v = self
            .robot
            .call(proto::Call::RobotSingControl(proto::SingControlParams {
                action,
                position_s,
                volume: None,
            }))
            .await
            .map_err(|e| ApiError::new(502, "RobotUnavailable", e))?;
        let r: proto::SingResult = serde_json::from_value(v)
            .map_err(|e| ApiError::new(502, "RobotUnavailable", e.to_string()))?;
        if r.accepted {
            Ok(())
        } else {
            Err(ApiError::new(409, "Refused", r.reason.unwrap_or_default()))
        }
    }

    // ── state ────────────────────────────────────────────────────────────────

    /// The playback view now, updating the bookkeeping from the robot's state. The bool says
    /// whether a show is still running.
    pub fn refresh(&self) -> (PlaybackView, bool) {
        let st = self.robot.state();
        let mut inner = self.inner.lock().expect("not poisoned");
        let Some(cur) = inner.cur.as_mut() else {
            let v = inner.last.clone().unwrap_or(PlaybackView {
                song_id: None,
                revision: None,
                status: "stopped",
                position: 0.0,
                duration: 0.0,
                error: None,
            });
            return (v, false);
        };
        let sing = st.as_ref().and_then(|(s, _)| s.sing.clone());
        let mut view = PlaybackView {
            song_id: Some(cur.show_id.clone()),
            revision: Some(cur.revision.clone()),
            status: "playing",
            position: cur.last_position,
            duration: cur.duration_s,
            error: None,
        };
        match sing {
            Some(s) => {
                cur.seen_sing = true;
                cur.last_position = s.t_s.max(0.0);
                view.position = cur.last_position;
                view.status = if s.paused {
                    "paused"
                } else if s.phase == "settling" {
                    if cur.stop_requested {
                        "stopped"
                    } else {
                        "playing"
                    }
                } else {
                    "playing"
                };
            }
            None => {
                let robot_known = st.is_some();
                if cur.seen_sing
                    || cur.stop_requested
                    || (robot_known && cur.started.elapsed() > Duration::from_secs(6))
                    || (!robot_known && cur.started.elapsed() > Duration::from_secs(2))
                {
                    let status = if cur.stop_requested {
                        "stopped"
                    } else {
                        "ended"
                    };
                    view.status = status;
                    if status == "ended" {
                        view.position = cur.duration_s;
                    }
                    cur.cancel.store(true, Ordering::Relaxed);
                    inner.last = Some(view.clone());
                    inner.cur = None;
                    self.version.fetch_add(1, Ordering::Relaxed);
                    return (view, false);
                }
            }
        }
        (view, true)
    }

    pub fn snapshot(&self) -> Snapshot {
        let (playback, _) = self.refresh();
        let songs: Vec<Song> = self.store.list().iter().map(Self::song_of).collect();
        // A change of what is on the robot or of what it is doing moves `version`; a poll that
        // sees the same thing does not.
        let key = format!(
            "{}|{:?}|{}|{:?}",
            songs.len(),
            playback.song_id,
            playback.status,
            playback.revision
        );
        {
            let mut last = self.last_key.lock().expect("not poisoned");
            if *last != key {
                *last = key;
                self.version.fetch_add(1, Ordering::Relaxed);
            }
        }
        Snapshot {
            protocol: PROTOCOL,
            device_id: self.cfg.device_id.clone(),
            name: self.cfg.name.clone(),
            boot_id: self.boot_id.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            version: self.version.load(Ordering::Relaxed),
            songs,
            volume: self.volume(),
            playback,
        }
    }

    // ── the sequencer ────────────────────────────────────────────────────────

    async fn abort(&self, why: &str, code: &'static str) {
        tracing::warn!(code, why, "show aborted");
        let _ = self.robot.call(proto::Call::RobotSing(stop_params())).await;
        self.robot.notify(proto::Call::RobotPose(proto::PoseParams {
            active: false,
            ..Default::default()
        }));
        self.robot.notify(proto::Call::RobotMove(proto::MoveParams {
            vx: 0.0,
            vy: 0.0,
            vyaw: 0.0,
        }));
        self.robot.notify(proto::Call::RobotStop);
        let mut inner = self.inner.lock().expect("not poisoned");
        if let Some(cur) = inner.cur.as_mut() {
            cur.stop_requested = true;
            let view = PlaybackView {
                song_id: Some(cur.show_id.clone()),
                revision: Some(cur.revision.clone()),
                status: "stopped",
                position: cur.last_position,
                duration: cur.duration_s,
                error: Some(format!("{code}: {why}")),
            };
            inner.last = Some(view);
            inner.cur = None;
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Run the coarse actions of a track against the robot's own song clock.
    async fn sequence(self: Arc<Self>, tl: Timeline, cancel: Arc<AtomicBool>) {
        enum Running {
            Skill {
                deadline: Instant,
                started: Instant,
                after: Pose,
            },
            Hold,
        }
        let mut next = 0usize;
        let mut running: Option<(usize, Running)> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tick.tick().await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let Some((st, age)) = self.robot.state() else {
                continue;
            };
            if age > Duration::from_secs(2) {
                continue;
            }
            if st.safety.fallen || st.safety.limp || st.safety.picked_up {
                self.abort("the robot fell, went limp or was lifted", "RobotUnsafe")
                    .await;
                return;
            }
            let Some(sing) = st.sing.clone() else {
                // Not started yet, or over: the playback bookkeeping decides which.
                if running.is_none() && next >= tl.steps.len() {
                    return;
                }
                continue;
            };
            let t = sing.t_s;
            if sing.paused {
                continue;
            }
            // Finish or drive what is running.
            if let Some((i, what)) = running.take() {
                let step = &tl.steps[i];
                match what {
                    Running::Skill {
                        deadline,
                        started,
                        after,
                    } => {
                        let pose = pose_of(&st);
                        if pose == after
                            && started.elapsed() > Duration::from_millis(800)
                            && t >= step.start_s
                        {
                            // Done: the pose it was to leave the robot in is the pose it is in.
                        } else if Instant::now() > deadline {
                            self.abort(
                                &format!("{:?} did not finish", step.action.event_id),
                                "SkillTimeout",
                            )
                            .await;
                            return;
                        } else {
                            running = Some((
                                i,
                                Running::Skill {
                                    deadline,
                                    started,
                                    after,
                                },
                            ));
                        }
                    }
                    Running::Hold => {
                        if t >= step.end_s {
                            self.finish_hold(step);
                        } else {
                            self.drive(step, t);
                            running = Some((i, Running::Hold));
                        }
                    }
                }
            }
            if running.is_none() && next < tl.steps.len() && t >= tl.steps[next].start_s {
                let step = &tl.steps[next];
                match &step.action.kind {
                    ActionKind::Skill { skill } => {
                        let pose = pose_of(&st);
                        if pose != step.before {
                            self.abort(
                                &format!(
                                    "{skill} needs the robot {}; it is {}",
                                    step.before.name(),
                                    pose.name()
                                ),
                                "PoseMismatch",
                            )
                            .await;
                            return;
                        }
                        let r = self
                            .robot
                            .call(proto::Call::RobotDo(proto::DoParams {
                                skill: skill.clone(),
                            }))
                            .await;
                        let ok = r
                            .as_ref()
                            .ok()
                            .and_then(|v| v.get("accepted"))
                            .and_then(|a| a.as_bool())
                            .unwrap_or(false);
                        if !ok {
                            self.abort(
                                &format!("the robot refused {skill}: {r:?}"),
                                "SkillRefused",
                            )
                            .await;
                            return;
                        }
                        let typical = ((step.end_s - step.start_s) * 1000.0) as u64;
                        running = Some((
                            next,
                            Running::Skill {
                                deadline: Instant::now()
                                    + Duration::from_millis(typical.max(2000) + 6000),
                                started: Instant::now(),
                                after: step.after,
                            },
                        ));
                    }
                    ActionKind::Body { .. } | ActionKind::Walk { .. } => {
                        if pose_of(&st) != Pose::Standing {
                            self.abort(
                                "walking and body offsets need a standing robot",
                                "PoseMismatch",
                            )
                            .await;
                            return;
                        }
                        running = Some((next, Running::Hold));
                    }
                    ActionKind::Head { .. } => {}
                }
                next += 1;
            }
            if running.is_none() && next >= tl.steps.len() {
                return;
            }
        }
    }

    /// One 20 Hz intent for a body or walk action at song time `t`.
    fn drive(&self, step: &Step, t: f64) {
        let local_ms = ((t - step.start_s) * 1000.0).max(0.0);
        match &step.action.kind {
            ActionKind::Body { keyframes } => {
                let v = interp(
                    keyframes.iter().map(|k| {
                        (
                            k.t_ms as f64,
                            [f64::from(k.z), f64::from(k.roll), f64::from(k.pitch)],
                        )
                    }),
                    local_ms,
                );
                self.robot.notify(proto::Call::RobotPose(proto::PoseParams {
                    z: v[0],
                    roll: v[1],
                    pitch: v[2],
                    active: true,
                }));
            }
            ActionKind::Walk { keyframes } => {
                let v = interp(
                    keyframes.iter().map(|k| {
                        (
                            k.t_ms as f64,
                            [f64::from(k.vx), f64::from(k.vy), f64::from(k.vyaw)],
                        )
                    }),
                    local_ms,
                );
                self.robot.notify(proto::Call::RobotMove(proto::MoveParams {
                    vx: v[0],
                    vy: v[1],
                    vyaw: v[2],
                }));
            }
            _ => {}
        }
    }

    fn finish_hold(&self, step: &Step) {
        match &step.action.kind {
            ActionKind::Body { .. } => {
                self.robot.notify(proto::Call::RobotPose(proto::PoseParams {
                    active: false,
                    ..Default::default()
                }));
            }
            ActionKind::Walk { .. } => {
                self.robot.notify(proto::Call::RobotMove(proto::MoveParams {
                    vx: 0.0,
                    vy: 0.0,
                    vyaw: 0.0,
                }));
            }
            _ => {}
        }
    }
}

fn interp<const N: usize>(keys: impl Iterator<Item = (f64, [f64; N])>, t: f64) -> [f64; N] {
    let k: Vec<_> = keys.collect();
    match k.len() {
        0 => [0.0; N],
        _ if t <= k[0].0 => k[0].1,
        _ if t >= k[k.len() - 1].0 => k[k.len() - 1].1,
        _ => {
            let i = k.partition_point(|x| x.0 <= t);
            let (a, b) = (&k[i - 1], &k[i]);
            let f = (t - a.0) / (b.0 - a.0).max(1e-6);
            let mut out = [0.0; N];
            for (j, slot) in out.iter_mut().enumerate() {
                *slot = a.1[j] + (b.1[j] - a.1[j]) * f;
            }
            out
        }
    }
}

pub fn stop_params() -> proto::SingParams {
    proto::SingParams {
        active: false,
        piece: None,
        part: None,
        style: None,
        mute: false,
        performance: None,
        mouth_latency_ms: None,
        head_latency_ms: None,
        audio_latency_ms: None,
        head_scale: None,
        audio: None,
        head_keys: Vec::new(),
    }
}

/// A performance with nothing sung in it: an audio-only show keeps the clock, the lead-in and the
/// idle drift, and has no beak to move.
pub fn silent_performance(name: &str, duration_s: f32, bpm: f32) -> String {
    let p = singer::Performance {
        name: name.to_owned(),
        bpm,
        source: "audio-only".into(),
        duration: duration_s.max(0.5),
        syllables: Vec::new(),
        phrases: Vec::new(),
        sections: vec![singer::Section {
            kind: singer::SectionKind::Verse,
            start: 0.0,
            end: duration_s.max(0.5),
            intensity: 0.4,
            variant: 0,
        }],
        beats: Vec::new(),
    };
    p.to_json()
}
