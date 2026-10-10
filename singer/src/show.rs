//! A compiled performance: the whole song's beak and head tracks, ready to be sampled by song
//! time.

use crate::dsp::sample_at;
use crate::head::{self, HeadStats, Layers};
use crate::mouth::{self, MouthStats};
use crate::perf::{Invalid, Performance};
use crate::profile::Profile;
use crate::style::Style;

/// What the joints should be doing at one instant, in the robot's own conventions.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    /// Beak opening, 0 (closed) to 1 (open): the unit `robot.mouth` takes.
    pub mouth: f32,
    /// Offsets `[neck_pitch, head_pitch, head_yaw, head_roll]` in radians, **added to wherever
    /// the head was told to look** — the same contract as the chorale's sway.
    pub head: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Show {
    pub perf: Performance,
    pub style: Style,
    pub profile: Profile,
    pub mouth: Vec<f32>,
    pub head: Vec<[f32; 4]>,
    pub mouth_stats: MouthStats,
    pub head_stats: HeadStats,
}

impl Show {
    pub fn compile(perf: Performance, style: Style, profile: Profile) -> Result<Self, Invalid> {
        Self::compile_with(perf, style, profile, Layers::ALL)
    }

    pub fn compile_with(
        perf: Performance,
        style: Style,
        profile: Profile,
        layers: Layers,
    ) -> Result<Self, Invalid> {
        perf.validate()?;
        let (mouth, mouth_stats) = mouth::compile(&perf, &style.mouth, &profile, style.seed);
        let (head, head_stats) = head::compile(&perf, &style.head, &profile, style.seed, layers);
        Ok(Self {
            perf,
            style,
            profile,
            mouth,
            head,
            mouth_stats,
            head_stats,
        })
    }

    /// Seconds from the start of the song to the end of the motion, including the settle.
    pub fn duration(&self) -> f32 {
        self.head.len() as f32 / self.profile.rate
    }

    /// The frame at song time `t` (seconds; clamped to the show). The beak is sampled
    /// `profile.mouth_lead()` ahead so its *motion* meets the *sound*; the head likewise by
    /// `profile.head_lead()` — its lag is longer than a nod, so an uncompensated head announces a
    /// phrase after it has begun.
    pub fn sample(&self, t: f64) -> Frame {
        let rate = self.profile.rate;
        let m = sample_at(&self.mouth, rate, t + f64::from(self.profile.mouth_lead()));
        let i = ((t + f64::from(self.profile.head_lead())) * f64::from(rate)).max(0.0);
        let k = i.floor() as usize;
        let head = if self.head.is_empty() {
            [0.0; 4]
        } else if k + 1 >= self.head.len() {
            *self.head.last().expect("non-empty")
        } else {
            let f = (i - k as f64) as f32;
            let (a, b) = (self.head[k], self.head[k + 1]);
            [0, 1, 2, 3].map(|j| a[j] * (1.0 - f) + b[j] * f)
        };
        Frame { mouth: m, head }
    }

    /// Which section of the song `t` is in, for the state report.
    pub fn section_name(&self, t: f64) -> Option<String> {
        self.perf
            .section_at(t as f32)
            .map(|(_, s)| format!("{:?}#{}", s.kind, s.variant).to_lowercase())
    }
}
