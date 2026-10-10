//! The golden package: a small, valid show that both sides of the integration read.
//!
//! The same files are the contract fixtures for the app's tests. If one of these tests has to
//! change, the app's package writer has to change with it.

use show::{
    AudioRole, AudioTrack, Capabilities, DuckScore, Manifest, MotionTrack, Pose, Role, SkillInfo,
    TrackFile, Tracks, sha256_hex,
};
use singer::{Profile, Show, Style};

const DUCK: &[u8] = include_bytes!("golden/duck.score.json");
const MOTION: &[u8] = include_bytes!("golden/motion.json");

fn caps() -> Capabilities {
    let (head, body, walk) = Capabilities::default_limits();
    Capabilities {
        protocol: 1,
        device_id: "microduck-test".into(),
        model_id: "microduck-v1".into(),
        skills: vec![
            SkillInfo {
                name: "sit_toggle".into(),
                duration_ms: 2600,
                entry_pose: "any".into(),
            },
            SkillInfo {
                name: "roulade".into(),
                duration_ms: 4000,
                entry_pose: "standing".into(),
            },
        ],
        head,
        body,
        walk,
        max_show_bytes: 64 << 20,
        max_events: 2000,
        audio_roles: vec!["mix".into()],
        pose: Pose::Standing,
        battery_percent: Some(80.0),
    }
}

fn manifest(audio: &[u8]) -> Manifest {
    Manifest {
        schema: "kivi.show/2".into(),
        show_id: "golden-0001".into(),
        title: "Golden".into(),
        tempo_bpm: 96.0,
        duration_ms: 16_000,
        roles: vec![Role {
            role_id: "lead".into(),
            model_id: "microduck-v1".into(),
        }],
        start_pose: None,
        tracks: Tracks {
            duck: Some(TrackFile {
                file: "duck.score.json".into(),
                sha256: sha256_hex(DUCK),
                bytes: DUCK.len() as u64,
            }),
            motion: Some(TrackFile {
                file: "motion.json".into(),
                sha256: sha256_hex(MOTION),
                bytes: MOTION.len() as u64,
            }),
            audio: Some(AudioTrack {
                file: "audio.wav".into(),
                role: AudioRole::Mix,
                codec: "pcm_s16le".into(),
                sample_rate: 32_000,
                channels: 1,
                start_offset_ms: 0,
                sha256: sha256_hex(audio),
                bytes: audio.len() as u64,
            }),
            expression: None,
        },
    }
}

#[test]
fn the_golden_manifest_validates_and_round_trips() {
    let m = manifest(b"RIFF....");
    m.validate().expect("valid");
    let json = serde_json::to_vec(&m).unwrap();
    assert_eq!(Manifest::parse(&json).unwrap(), m);
    let text = String::from_utf8(json).unwrap();
    assert!(
        text.contains("\"showId\"") && text.contains("\"tempoBpm\""),
        "camelCase: {text}"
    );
}

#[test]
fn the_golden_duck_track_becomes_a_performance_the_singer_can_compile() {
    let score = DuckScore::parse(DUCK).expect("parses");
    let perf = score.to_performance("golden").expect("performance");
    // n08 and n09 (same pitch, tied) merge into one note, so ten events make nine syllables;
    // n03 and n04 (a new pitch, tied) stay two, the second marked `tie`.
    assert_eq!(perf.syllables.len(), 9);
    assert!(perf.syllables[3].tie);
    // n08 and n09: the same pitch, tied → one long note.
    assert!(
        perf.syllables
            .iter()
            .any(|s| s.duration() > 2.0 * 60.0 / 96.0)
    );
    assert_eq!(perf.sections.len(), 2);
    let show = Show::compile(perf, Style::auto(&score_perf(&score)), Profile::default())
        .expect("compiles");
    assert!(show.mouth.iter().cloned().fold(0.0f32, f32::max) > 0.5);
}

fn score_perf(score: &DuckScore) -> singer::Performance {
    score.to_performance("golden").unwrap()
}

#[test]
fn the_golden_motion_validates_against_a_standing_robot() {
    let motion = MotionTrack::parse(MOTION).expect("parses");
    motion.validate(&caps(), Pose::Standing).expect("valid");
    let keys = motion.head_keys();
    assert!(
        keys.windows(2).all(|w| w[0].0 <= w[1].0),
        "ascending: {keys:?}"
    );
    assert_eq!(
        keys.last().unwrap().1,
        [0.0; 4],
        "the head is returned to zero"
    );
}

#[test]
fn sit_toggle_is_stateful_and_the_check_follows_it() {
    // From sitting, the first sit_toggle stands up and the head clip is then fine standing, but the
    // second sit_toggle sits down and the body offsets that follow are refused.
    let motion = MotionTrack::parse(MOTION).unwrap();
    let err = motion.validate(&caps(), Pose::Sitting).unwrap_err();
    assert!(err.has("PoseMismatch"), "{err}");
    assert!(
        err.0.iter().any(|p| p.event_id.as_deref() == Some("a4")),
        "{err}"
    );
}

#[test]
fn an_unknown_skill_a_big_head_and_an_overlap_are_each_named() {
    let mut caps = caps();
    caps.skills.retain(|s| s.name != "sit_toggle");
    let err = MotionTrack::parse(MOTION)
        .unwrap()
        .validate(&caps, Pose::Standing)
        .unwrap_err();
    assert!(err.has("ActionUnsupported"));

    let mut m = MotionTrack::parse(MOTION).unwrap();
    if let show::ActionKind::Head { keyframes } = &mut m.actions[1].kind {
        keyframes[1].head_yaw = 1.2;
    }
    m.actions[2].start_ms = 6000;
    let err = m.validate(&self::caps(), Pose::Standing).unwrap_err();
    assert!(err.has("OutOfRange") && err.has("Overlap"), "{err}");
}

#[test]
fn a_manifest_that_cannot_be_trusted_is_refused() {
    let mut m = manifest(b"x");
    m.show_id = "../../etc".into();
    assert!(m.validate().unwrap_err().has("Corrupt"));
    let mut m = manifest(b"x");
    m.tracks.audio.as_mut().unwrap().role = AudioRole::Backing;
    assert!(m.validate().unwrap_err().has("Incompatible"));
    let mut m = manifest(b"x");
    m.roles.push(m.roles[0].clone());
    assert!(m.validate().unwrap_err().has("Incompatible"));
}

#[test]
fn overlapping_notes_are_refused_with_the_note() {
    let mut score = DuckScore::parse(DUCK).unwrap();
    score.events[1].start_beat = 0.5;
    let err = score.validate(100).unwrap_err();
    assert!(
        err.0
            .iter()
            .any(|p| p.code == "Overlap" && p.event_id.as_deref() == Some("n02")),
        "{err}"
    );
}
