//! What this robot says it can do. The robot reports it at run time; nobody maintains a list.

use serde::{Deserialize, Serialize};

/// The poses the executor tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pose {
    Standing,
    Sitting,
    /// Limp, fallen, lifted or running a skill: nothing may be scheduled from here.
    Unknown,
}

impl Pose {
    pub fn name(self) -> &'static str {
        match self {
            Pose::Standing => "standing",
            Pose::Sitting => "sitting",
            Pose::Unknown => "in an unknown pose",
        }
    }

    pub fn from_name(s: &str) -> Pose {
        match s {
            "standing" => Pose::Standing,
            "sitting" => Pose::Sitting,
            _ => Pose::Unknown,
        }
    }

    /// Body offsets and walking need a standing robot.
    pub fn can_stand_actions(self) -> bool {
        self == Pose::Standing
    }

    /// The pose after running `skill`, or the pose it needed. `sit_toggle` is the one stateful
    /// skill: it flips between the two, which is why a show must know which way it will flip.
    pub fn after_skill(self, skill: &str, entry: &str) -> Result<Pose, &'static str> {
        if skill == "sit_toggle" {
            return match self {
                Pose::Standing => Ok(Pose::Sitting),
                Pose::Sitting => Ok(Pose::Standing),
                Pose::Unknown => Err("standing or sitting"),
            };
        }
        let needs = if entry.is_empty() { "standing" } else { entry };
        match (needs, self) {
            ("standing", Pose::Standing) | ("sitting", Pose::Sitting) | ("any", _) => Ok(self),
            ("sitting", _) => Err("sitting"),
            _ => Err("standing"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    /// Typical running time, from the skill's own `manifest.json`; 0 when it does not say.
    pub duration_ms: u64,
    /// `standing`, `sitting` or `any`; empty means standing.
    #[serde(default)]
    pub entry_pose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadLimits {
    /// Largest offset per `[neck_pitch, head_pitch, head_yaw, head_roll]`, radians.
    pub max_abs: [f32; 4],
    pub max_vel: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyLimits {
    pub z: (f32, f32),
    pub roll: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkLimits {
    pub max_vx: f32,
    pub max_vy: f32,
    pub max_vyaw: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub protocol: u32,
    pub device_id: String,
    pub model_id: String,
    pub skills: Vec<SkillInfo>,
    pub head: HeadLimits,
    pub body: BodyLimits,
    pub walk: WalkLimits,
    pub max_show_bytes: u64,
    /// Most notes or actions in one track.
    pub max_events: usize,
    /// Which `audio` roles this robot accepts.
    pub audio_roles: Vec<String>,
    pub pose: Pose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<f32>,
}

impl Capabilities {
    pub fn skill(&self, name: &str) -> Option<&SkillInfo> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Conservative defaults for the limits the robot cannot read off a file.
    ///
    /// **Unmeasured.** The head's are well inside the joints' own travel (the head carries the
    /// ToF and the balance policy has opinions about mass this high up); the body's are the
    /// policy's trained ranges quoted in `robot.pose`; the walk's are a slow stroll. They are
    /// configuration (`showd --limits`), not facts, and they only ever get loosened by a test.
    pub fn default_limits() -> (HeadLimits, BodyLimits, WalkLimits) {
        (
            HeadLimits {
                max_abs: [0.20, 0.30, 0.50, 0.25],
                max_vel: [0.8, 1.2, 1.5, 1.2],
            },
            BodyLimits {
                z: (-0.025, 0.010),
                roll: 0.26,
                pitch: 0.26,
            },
            WalkLimits {
                max_vx: 0.20,
                max_vy: 0.10,
                max_vyaw: 0.8,
            },
        )
    }
}
