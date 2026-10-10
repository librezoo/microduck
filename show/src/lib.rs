//! The show package: a manifest, and the tracks it names.
//!
//! ```text
//!   manifest.json          what the package is, and the hash of every track
//!   duck.score.json        the sung notes — the truth the beak, the head and the voice all follow
//!   motion.json            serial action clips (skill / head / body / walk)
//!   audio.wav              what the speaker plays ("mix": the app has already mixed it)
//!   expression.patch.json  optional hand corrections to the performance
//! ```
//!
//! Everything is JSON in camelCase, matching Kivi Studio's own field names, so the two sides read
//! the same documents. The robot **validates** a package and **never repairs** one: a track that
//! does not fit is refused with the reason, because a silently clamped keyframe is a motion nobody
//! agreed to.

pub mod caps;
pub mod duck;
pub mod error;
pub mod manifest;
pub mod motion;

pub use caps::{Capabilities, Pose, SkillInfo};
pub use duck::{DuckScore, NoteEvent, SectionEvent, Voice};
pub use error::{Problem, Problems};
pub use manifest::{AudioRole, AudioTrack, Manifest, Role, TrackFile, Tracks};
pub use motion::{Action, ActionKind, BodyKey, HeadKey, MotionTrack, WalkKey};

/// The tracks a package may carry.
pub const TRACK_NAMES: [&str; 4] = ["duck", "motion", "audio", "expression"];

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
