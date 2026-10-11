//! The motion track: serial action clips.
//!
//! The robot can execute four kinds, because those are the channels the balance policy leaves
//! open while it holds the robot up: a **skill** (a trained policy by name), **head** keyframes,
//! **body** offsets (`robot.pose`), and a **walk** velocity envelope (`robot.move`). A raw
//! joint trajectory for the legs is deliberately not a kind: the policy owns the legs.

use serde::{Deserialize, Serialize};

use crate::caps::{Capabilities, Pose};
use crate::error::{Problem, Problems};

pub const SCHEMA: &str = "kivi.motion/1";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadKey {
    pub t_ms: u64,
    pub neck_pitch: f32,
    pub head_pitch: f32,
    pub head_yaw: f32,
    pub head_roll: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyKey {
    pub t_ms: u64,
    pub z: f32,
    pub roll: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkKey {
    pub t_ms: u64,
    pub vx: f32,
    pub vy: f32,
    pub vyaw: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActionKind {
    Skill {
        skill: String,
    },
    Head {
        keyframes: Vec<HeadKey>,
    },
    Body {
        keyframes: Vec<BodyKey>,
    },
    Walk {
        keyframes: Vec<WalkKey>,
    },
    /// Nothing is commanded: the clip asserts the pose the robot is to be in (`standing` or
    /// `sitting`) for its duration. A "stand naturally" clip between two songs' worth of motion
    /// is this — and on a robot that is sitting it is refused, not turned into a stand-up.
    Hold {
        pose: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub event_id: String,
    /// The app's own name for it; shown in messages, not interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    pub start_ms: u64,
    pub duration_ms: u64,
    #[serde(flatten)]
    pub kind: ActionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionTrack {
    pub schema: String,
    pub actions: Vec<Action>,
}

fn keys_t(kind: &ActionKind) -> Vec<u64> {
    match kind {
        ActionKind::Skill { .. } | ActionKind::Hold { .. } => vec![],
        ActionKind::Head { keyframes } => keyframes.iter().map(|k| k.t_ms).collect(),
        ActionKind::Body { keyframes } => keyframes.iter().map(|k| k.t_ms).collect(),
        ActionKind::Walk { keyframes } => keyframes.iter().map(|k| k.t_ms).collect(),
    }
}

impl MotionTrack {
    /// Reads the track. It does **not** validate: call [`Self::validate`] before trusting any
    /// time in it (`end_ms` and `head_keys` saturate, so they cannot overflow, but they are only
    /// meaningful on a validated track).
    pub fn parse(bytes: &[u8]) -> Result<Self, Problems> {
        serde_json::from_slice(bytes).map_err(|e| {
            let mut p = Problems::default();
            p.push(Problem::new("Incompatible", format!("motion track: {e}")));
            p
        })
    }

    pub fn end_ms(&self) -> u64 {
        self.actions
            .iter()
            .map(|a| a.start_ms.saturating_add(a.duration_ms))
            .max()
            .unwrap_or(0)
    }

    /// Check the whole track against what this robot can do, from `start` pose.
    ///
    /// **The robot's check is the authoritative one.** The app evaluates a motion for the user
    /// (what is reachable, what it will look like) and that is advice; this is what stands
    /// between a package and the motors, and it never clamps — an out-of-range keyframe is
    /// refused, with the event it is in.
    pub fn validate(&self, caps: &Capabilities, start: Pose) -> Result<(), Problems> {
        let mut p = Problems::default();
        if self.schema != SCHEMA {
            p.push(Problem::new(
                "Incompatible",
                format!(
                    "motion schema {:?}, this robot reads {SCHEMA:?}",
                    self.schema
                ),
            ));
        }
        if self.actions.len() > caps.max_events {
            p.push(Problem::new(
                "TooLarge",
                format!(
                    "{} actions; this robot takes {}",
                    self.actions.len(),
                    caps.max_events
                ),
            ));
        }
        let mut pose = start;
        let mut prev_end = 0u64;
        let mut times_ok = true;
        for a in &self.actions {
            // Bound every time before any sum below touches it.
            if a.start_ms > crate::MAX_SHOW_MS
                || a.duration_ms > crate::MAX_SHOW_MS
                || keys_t(&a.kind).iter().any(|t| *t > crate::MAX_SHOW_MS)
            {
                times_ok = false;
                p.push(
                    Problem::new(
                        "OutOfRange",
                        format!("an action's time lies beyond {} ms", crate::MAX_SHOW_MS),
                    )
                    .at(&a.event_id),
                );
                continue;
            }
            if a.duration_ms == 0 {
                p.push(Problem::new("Corrupt", "an action has zero duration").at(&a.event_id));
            }
            if a.start_ms < prev_end {
                p.push(
                    Problem::new(
                        "Overlap",
                        "actions are strictly serial; this one starts before the last ends",
                    )
                    .at(&a.event_id),
                );
            }
            prev_end = prev_end.max(a.start_ms + a.duration_ms);
            let ts = keys_t(&a.kind);
            if ts.windows(2).any(|w| w[1] < w[0]) || ts.iter().any(|t| *t > a.duration_ms) {
                p.push(
                    Problem::new(
                        "Corrupt",
                        "keyframe times must ascend and stay inside the action",
                    )
                    .at(&a.event_id),
                );
            }
            match &a.kind {
                ActionKind::Skill { skill } => match caps.skill(skill) {
                    None => p.push(
                        Problem::new(
                            "ActionUnsupported",
                            format!("this robot has no skill {skill:?}"),
                        )
                        .at(&a.event_id),
                    ),
                    Some(info) => {
                        match pose.after_skill(skill, &info.entry_pose) {
                            Ok(next) => pose = next,
                            Err(need) => p.push(
                                Problem::new(
                                    "PoseMismatch",
                                    format!(
                                        "{skill} needs the robot {need}; it will be {}",
                                        pose.name()
                                    ),
                                )
                                .at(&a.event_id),
                            ),
                        }
                        let typical = info.duration_ms;
                        if typical > 0 && a.duration_ms.saturating_add(200) < typical {
                            p.push(
                                Problem::new(
                                    "TooShort",
                                    format!(
                                        "{skill} takes about {typical} ms; this clip is {} ms",
                                        a.duration_ms
                                    ),
                                )
                                .at(&a.event_id),
                            );
                        }
                    }
                },
                ActionKind::Hold { pose: want } => {
                    let want = Pose::from_name(want);
                    if want == Pose::Unknown {
                        p.push(
                            Problem::new("Corrupt", "a hold names standing or sitting")
                                .at(&a.event_id),
                        );
                    } else if want != pose {
                        p.push(
                            Problem::new(
                                "PoseMismatch",
                                format!(
                                    "the clip holds the robot {}; it will be {}",
                                    want.name(),
                                    pose.name()
                                ),
                            )
                            .at(&a.event_id),
                        );
                    }
                }
                ActionKind::Head { keyframes } => {
                    let lim = caps.head.max_abs;
                    for k in keyframes {
                        let v = [k.neck_pitch, k.head_pitch, k.head_yaw, k.head_roll];
                        if v.iter()
                            .zip(lim)
                            .any(|(x, l)| !x.is_finite() || x.abs() > l)
                        {
                            p.push(
                                Problem::new(
                                    "OutOfRange",
                                    format!("head offset beyond {lim:?} rad"),
                                )
                                .at(&a.event_id),
                            );
                            break;
                        }
                    }
                    if let Some(speed) = max_speed(keyframes.iter().map(|k| {
                        (
                            k.t_ms,
                            [k.neck_pitch, k.head_pitch, k.head_yaw, k.head_roll],
                        )
                    })) && speed.iter().zip(caps.head.max_vel).any(|(v, l)| *v > l)
                    {
                        p.push(
                            Problem::new(
                                "TooFast",
                                "a head keyframe pair moves faster than the head may",
                            )
                            .at(&a.event_id),
                        );
                    }
                }
                ActionKind::Body { keyframes } => {
                    if !pose.can_stand_actions() {
                        p.push(
                            Problem::new(
                                "PoseMismatch",
                                format!(
                                    "body offsets need the robot standing; it will be {}",
                                    pose.name()
                                ),
                            )
                            .at(&a.event_id),
                        );
                    }
                    let b = &caps.body;
                    for k in keyframes {
                        if !(b.z.0..=b.z.1).contains(&k.z)
                            || k.roll.abs() > b.roll
                            || k.pitch.abs() > b.pitch
                        {
                            p.push(
                                Problem::new(
                                    "OutOfRange",
                                    format!(
                                        "body offset outside the trained range (z {:?} m, roll/pitch ±{}/{} rad)",
                                        b.z, b.roll, b.pitch
                                    ),
                                )
                                .at(&a.event_id),
                            );
                            break;
                        }
                    }
                }
                ActionKind::Walk { keyframes } => {
                    if !pose.can_stand_actions() {
                        p.push(
                            Problem::new(
                                "PoseMismatch",
                                format!(
                                    "walking needs the robot standing; it will be {}",
                                    pose.name()
                                ),
                            )
                            .at(&a.event_id),
                        );
                    }
                    let w = &caps.walk;
                    if keyframes.iter().any(|k| {
                        k.vx.abs() > w.max_vx || k.vy.abs() > w.max_vy || k.vyaw.abs() > w.max_vyaw
                    }) {
                        p.push(
                            Problem::new(
                                "OutOfRange",
                                format!(
                                    "walk speed beyond ±{}/{}/{} (m/s, m/s, rad/s)",
                                    w.max_vx, w.max_vy, w.max_vyaw
                                ),
                            )
                            .at(&a.event_id),
                        );
                    }
                }
            }
        }
        // What the robot executes is `head_keys()`, entry and exit anchors included, and it does
        // not slew them: judge those windows too.
        if times_ok {
            let keys = self.head_keys_by_action();
            let mut flagged = std::collections::HashSet::new();
            for w in keys.windows(2) {
                let dt = (w[1].0.saturating_sub(w[0].0)).max(1) as f32 / 1000.0;
                let too_fast =
                    (0..4).any(|j| ((w[1].1[j] - w[0].1[j]) / dt).abs() > caps.head.max_vel[j]);
                // A window whose later key is the next clip's own first key is that clip's
                // entry; the key's owner is the clip to name.
                if too_fast && flagged.insert(w[1].2) {
                    p.push(
                        Problem::new(
                            "TooFast",
                            "a head clip jumps from or to rest faster than the head may",
                        )
                        .at(&self.actions[w[1].2].event_id),
                    );
                }
            }
        }
        p.into_result()
    }

    /// The head keyframes of every `head` action as one absolute-time list, with a return to
    /// zero at the end of each clip so a clip never leaves the head where it stopped.
    pub fn head_keys(&self) -> Vec<(u64, [f32; 4])> {
        self.head_keys_by_action()
            .into_iter()
            .map(|(t, v, _)| (t, v))
            .collect()
    }

    /// `head_keys`, each key also naming the action (index) it belongs to.
    fn head_keys_by_action(&self) -> Vec<(u64, [f32; 4], usize)> {
        let mut out: Vec<(u64, [f32; 4], usize)> = Vec::new();
        for (i, a) in self.actions.iter().enumerate() {
            if let ActionKind::Head { keyframes } = &a.kind {
                if keyframes.is_empty() {
                    continue;
                }
                if out
                    .last()
                    .is_none_or(|l| l.0.saturating_add(1) < a.start_ms)
                {
                    out.push((a.start_ms.saturating_sub(1), [0.0; 4], i));
                }
                for k in keyframes {
                    out.push((
                        a.start_ms.saturating_add(k.t_ms),
                        [k.neck_pitch, k.head_pitch, k.head_yaw, k.head_roll],
                        i,
                    ));
                }
                out.push((a.start_ms.saturating_add(a.duration_ms), [0.0; 4], i));
            }
        }
        out
    }
}

fn max_speed(keys: impl Iterator<Item = (u64, [f32; 4])>) -> Option<[f32; 4]> {
    let v: Vec<_> = keys.collect();
    if v.len() < 2 {
        return None;
    }
    let mut m = [0.0f32; 4];
    for w in v.windows(2) {
        let dt = (w[1].0.saturating_sub(w[0].0)).max(1) as f32 / 1000.0;
        for (j, slot) in m.iter_mut().enumerate() {
            *slot = slot.max(((w[1].1[j] - w[0].1[j]) / dt).abs());
        }
    }
    Some(m)
}
