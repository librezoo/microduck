//! The manifest: the one document that says what a package is.

use serde::{Deserialize, Serialize};

use crate::error::{Problem, Problems};

pub const SCHEMA: &str = "kivi.show/2";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Role {
    pub role_id: String,
    pub model_id: String,
}

/// A track that is a JSON document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackFile {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioRole {
    /// The app has mixed the accompaniment and the duck voice; the robot plays it as is.
    Mix,
    /// Accompaniment only; the robot sings the duck track in its own voice over it. (Later.)
    Backing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioTrack {
    pub file: String,
    pub role: AudioRole,
    /// Only `pcm_s16le` (WAV) today.
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    /// Where the audio starts on the show clock, milliseconds. Negative starts it earlier.
    #[serde(default)]
    pub start_offset_ms: i64,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tracks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<TrackFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<TrackFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<TrackFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub show_id: String,
    pub title: String,
    pub tempo_bpm: f32,
    pub duration_ms: u64,
    pub roles: Vec<Role>,
    /// The pose the show starts from: `standing` (default) or `sitting`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_pose: Option<String>,
    pub tracks: Tracks,
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A show id becomes a directory name, so it is restricted to what is safe as one.
pub fn is_safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Problems> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| {
            let mut p = Problems::default();
            p.push(Problem::new("Incompatible", format!("manifest: {e}")));
            p
        })?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), Problems> {
        let mut p = Problems::default();
        if self.schema != SCHEMA {
            p.push(Problem::new(
                "Incompatible",
                format!("schema {:?}, this robot reads {SCHEMA:?}", self.schema),
            ));
        }
        if !is_safe_id(&self.show_id) {
            p.push(Problem::new(
                "Corrupt",
                "showId must be 1–64 of [A-Za-z0-9_-]",
            ));
        }
        if self.title.trim().is_empty() || self.title.len() > 200 {
            p.push(Problem::new("Corrupt", "title must be 1–200 bytes"));
        }
        if !(20.0..=300.0).contains(&self.tempo_bpm) {
            p.push(Problem::new("Corrupt", "tempoBpm outside 20..300"));
        }
        if self.duration_ms == 0 {
            p.push(Problem::new("Corrupt", "durationMs is zero"));
        }
        if self.duration_ms > crate::MAX_SHOW_MS {
            p.push(Problem::new(
                "TooLarge",
                format!("durationMs beyond {} ms", crate::MAX_SHOW_MS),
            ));
        }
        if self.roles.len() != 1 {
            p.push(Problem::new(
                "Incompatible",
                format!(
                    "{} roles; this robot performs exactly one (several ducks at once is not supported yet)",
                    self.roles.len()
                ),
            ));
        }
        if let Some(pose) = &self.start_pose
            && !matches!(pose.as_str(), "standing" | "sitting")
        {
            p.push(Problem::new("Corrupt", "startPose is standing or sitting"));
        }
        for (name, t) in [
            ("duck", &self.tracks.duck),
            ("motion", &self.tracks.motion),
            ("expression", &self.tracks.expression),
        ] {
            if let Some(t) = t {
                if !is_hex64(&t.sha256) {
                    p.push(Problem::new(
                        "Corrupt",
                        format!("{name}: sha256 is not 64 lower-case hex"),
                    ));
                }
                if t.file.contains('/') || t.file.contains("..") || t.file.is_empty() {
                    p.push(Problem::new(
                        "Corrupt",
                        format!("{name}: file must be a plain name"),
                    ));
                }
            }
        }
        if let Some(a) = &self.tracks.audio {
            if !is_hex64(&a.sha256) {
                p.push(Problem::new(
                    "Corrupt",
                    "audio: sha256 is not 64 lower-case hex",
                ));
            }
            if a.codec != "pcm_s16le" {
                p.push(Problem::new(
                    "Incompatible",
                    format!("audio codec {:?}; only pcm_s16le WAV is supported", a.codec),
                ));
            }
            if !(1..=2).contains(&a.channels) || !(8_000..=96_000).contains(&a.sample_rate) {
                p.push(Problem::new(
                    "Incompatible",
                    "audio must be 1–2 channels at 8–96 kHz",
                ));
            }
            if a.start_offset_ms.unsigned_abs() > crate::MAX_SHOW_MS {
                p.push(Problem::new(
                    "OutOfRange",
                    format!("audio startOffsetMs beyond ±{} ms", crate::MAX_SHOW_MS),
                ));
            }
            if a.role == AudioRole::Backing {
                p.push(Problem::new(
                    "Incompatible",
                    "audio role \"backing\" (the robot mixes its own voice) is not supported yet; send a \"mix\"",
                ));
            }
            if a.file.contains('/') || a.file.contains("..") || a.file.is_empty() {
                p.push(Problem::new("Corrupt", "audio: file must be a plain name"));
            }
        }
        if self.tracks.duck.is_none() && self.tracks.audio.is_none() && self.tracks.motion.is_none()
        {
            p.push(Problem::new("Corrupt", "a show needs at least one track"));
        }
        p.into_result()
    }

    /// Every track as `(name, sha256, bytes)`, for upload bookkeeping.
    pub fn track_hashes(&self) -> Vec<(&'static str, String, u64)> {
        let mut v = Vec::new();
        if let Some(t) = &self.tracks.duck {
            v.push(("duck", t.sha256.clone(), t.bytes));
        }
        if let Some(t) = &self.tracks.motion {
            v.push(("motion", t.sha256.clone(), t.bytes));
        }
        if let Some(t) = &self.tracks.audio {
            v.push(("audio", t.sha256.clone(), t.bytes));
        }
        if let Some(t) = &self.tracks.expression {
            v.push(("expression", t.sha256.clone(), t.bytes));
        }
        v
    }
}
