//! Singing performance generation for the duck.
//!
//! ```text
//!   score / analysed audio / hand edits
//!                │
//!                ▼
//!        [`Performance`]   events on the song clock: syllables, phrases, sections, beats
//!                │          (+ [`Style`] and [`Profile`])
//!                ▼
//!        [`Show::compile`]  mouth track + head track, feasible by construction
//!                │
//!                ▼
//!        [`Player`]         song clock → frames; seek/pause/stop never jump a joint
//!                │
//!                ▼
//!             robotd         owns the clock, the joints and the arbitration
//! ```
//!
//! Pure computation: nothing here reads a clock, a file or a socket. Design and evidence:
//! `docs/ideas/singing-performance.md`.

pub mod baselines;
pub mod dsp;
pub mod from_score;
pub mod head;
pub mod mechanism;
pub mod metrics;
pub mod mouth;
pub mod perf;
pub mod player;
pub mod profile;
pub mod show;
pub mod style;

pub use from_score::from_score;
pub use head::Layers;
pub use mechanism::Mechanism;
pub use perf::{Beat, Invalid, Onset, Performance, Phrase, Section, SectionKind, Syllable, Vowel};
pub use player::{Clock, Phase, Player};
pub use profile::{HeadProfile, MouthProfile, Profile};
pub use show::{Frame, Show};
pub use style::{HeadStyle, MouthStyle, Style};
