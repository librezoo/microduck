//! What the mechanism can do, as the generator needs to know it.
//!
//! **Every number here is a design default, not a measurement.** The beak servo's real velocity
//! and acceleration under its linkage load, the head's safe amplitude with the ToF sensor and the
//! balance policy on top, and the audio-to-beak latency are all things `docs/ideas/
//! singing-performance.md` lists as *to be measured on a robot* (stage H). They are set
//! conservatively — slower and smaller than the joint limits in `duck-control::model` — so that a
//! show compiled with the defaults is feasible on any plausible duck, and so that the first
//! calibration moves them *up*, never down.
//!
//! What is *not* a default: the joint travel itself. The beak's range comes from
//! `duck_control::model::mouth_target` and is applied by `robotd`, not here; this crate works in
//! "fraction open" and radians of head offset.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouthProfile {
    /// Fastest the beak may open or close, in fraction-of-stroke per second. The full stroke is
    /// −5°…+30°, so 5.0/s is 175°/s: a closing in 0.2 s.
    pub max_vel: f32,
    /// Fastest the beak's speed may change, fraction/s². Decides how a fast passage degrades.
    pub max_acc: f32,
}

impl Default for MouthProfile {
    fn default() -> Self {
        Self {
            max_vel: 5.0,
            max_acc: 80.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadProfile {
    /// Largest offset the show may ask of each of `[neck_pitch, head_pitch, head_yaw, head_roll]`,
    /// radians, **on top of wherever the head was told to look**. Starts at the bound the
    /// chorale's sway is already tested to (0.15 rad): the head carries the ToF and the balance
    /// policy has opinions about mass this high up.
    pub budget: [f32; 4],
    pub max_vel: [f32; 4],
    pub max_acc: [f32; 4],
    /// +1 if a positive `head_pitch` raises the beak, −1 if it lowers it. **Unverified**: the
    /// chorale's `head_expression` assumes negative is up ("if hardware says otherwise, flip
    /// `REACH_LIFT`") and nothing in the tree has checked it on a robot. One number to flip.
    pub pitch_up_sign: f32,
}

impl Default for HeadProfile {
    fn default() -> Self {
        Self {
            budget: [0.08, 0.15, 0.15, 0.12],
            max_vel: [0.6, 1.2, 1.2, 1.0],
            max_acc: [6.0, 12.0, 12.0, 10.0],
            pitch_up_sign: -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub mouth: MouthProfile,
    pub head: HeadProfile,
    /// Output sample rate of a compiled show. The control loop's rate; 50 Hz.
    pub rate: f32,
    /// How much *later* than the beak command the sound reaches the room, seconds. **Positive
    /// means the audio lags** (so the beak must be sampled later in the song to match it).
    /// Unmeasured: 0 until a robot is put in front of a camera and a microphone.
    pub audio_latency: f32,
    /// How much *later* than its command the beak physically moves, seconds. Compensation for
    /// this is a *lead* applied by the player, separate from the artistic anticipation the
    /// style asks for. Unmeasured: 0.
    pub mouth_latency: f32,
    /// How much *later* than its command the head physically gets there, seconds — the policy's
    /// tracking of the head command plus the servos. **Measured once, in the MuJoCo twin: 0.21–0.25 s**
    /// (`docs/ideas/singing-performance.md` §8.3), which is longer than a nod and shorter than a
    /// phrase, so it matters for exactly the layers that announce things. The twin's body is not the
    /// robot's, so this is a number to re-measure on hardware, not one to trust.
    pub head_latency: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            mouth: MouthProfile::default(),
            head: HeadProfile::default(),
            rate: 50.0,
            audio_latency: 0.0,
            mouth_latency: 0.0,
            head_latency: 0.0,
        }
    }
}

impl Profile {
    /// How far ahead in the song the beak must be commanded for its *motion* to coincide with
    /// the *sound*: the beak's own lag minus the audio's. Positive means command early.
    pub fn mouth_lead(&self) -> f32 {
        self.mouth_latency - self.audio_latency
    }

    /// How far ahead the head is commanded: its own lag minus the audio's, for the same reason.
    pub fn head_lead(&self) -> f32 {
        self.head_latency - self.audio_latency
    }
}
