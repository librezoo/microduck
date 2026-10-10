//! The service end to end against a robot in memory: pair, upload, publish, play, control.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use duck_ipc_proto as proto;
use serde_json::{Value, json};
use show::sha256_hex;
use showd::api::{Shared, router};
use showd::app::{App, Config};
use showd::auth::Auth;
use showd::robot::FakeRobot;
use showd::store::Store;
use tower::ServiceExt;

const DUCK: &[u8] = include_bytes!("../../show/tests/golden/duck.score.json");

struct Rig {
    app: Arc<App>,
    robot: Arc<FakeRobot>,
    router: axum::Router,
    token: String,
}

fn temp() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "showd-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A robot state with a `sing` block, built from JSON so the test names only what it cares about.
fn state(policy: &str, sing: Option<Value>, fallen: bool) -> proto::RobotState {
    let mut v = json!({
        "t": 1.0,
        "move": {"requested": [0,0,0], "applied": [0,0,0]},
        "head": [0,0,0,0],
        "policy": policy,
        "safety": {"fallen": fallen, "limp": false},
        "loop": {"hz": 50.0, "missed": 0},
        "joints": [], "targets": []
    });
    if let Some(s) = sing {
        v["sing"] = s;
    }
    serde_json::from_value(v).expect("a state")
}

fn sing_block(t_s: f64, paused: bool) -> Value {
    json!({"name": "x", "phase": if paused {"paused"} else {"performing"}, "t_s": t_s, "duration_s": 20.0,
           "mouth": 0.0, "head": [0,0,0,0], "paused": paused})
}

async fn rig() -> Rig {
    let dir = temp();
    let store = Store::open(&dir).unwrap();
    let robot = Arc::new(FakeRobot::default());
    robot.answers.lock().unwrap().insert(
        "robot.skills".into(),
        Ok(json!({"skills": [{"name": "roulade", "duration": 4.0}], "built_in": ["sit_toggle", "ground_pick"]})),
    );
    *robot.st.lock().unwrap() = Some(state("stand", None, false));
    let cfg = Config {
        device_id: "microduck-test".into(),
        name: "MicroDuck-TEST".into(),
        mouth_latency_ms: 150.0,
        head_latency_ms: 250.0,
        audio_latency_ms: 0.0,
        max_show_bytes: 32 << 20,
        max_events: 2000,
        volume_file: dir.join("volume"),
    };
    let app = App::new(cfg, store, robot.clone());
    let auth = Arc::new(Auth::load(&dir, Some("TESTCODE".into()), None).unwrap());
    let router = router(Shared {
        app: app.clone(),
        auth,
    });
    let mut r = Rig {
        app,
        robot,
        router,
        token: String::new(),
    };
    let (st, body) = r
        .req(
            "POST",
            "/api/v1/pair",
            None,
            Some(json!({"code": "TESTCODE"}).to_string().into_bytes()),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    r.token = body["token"].as_str().unwrap().to_owned();
    r
}

impl Rig {
    async fn req(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        let resp = self
            .router
            .clone()
            .oneshot(b.body(Body::from(body.unwrap_or_default())).unwrap())
            .await
            .unwrap();
        let st = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
            .await
            .unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn authed(&self, method: &str, uri: &str, body: Option<Vec<u8>>) -> (StatusCode, Value) {
        let t = self.token.clone();
        self.req(method, uri, Some(&t), body).await
    }
}

fn wav(seconds: u32, rate: u32) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
    for i in 0..(seconds * rate) {
        w.write_sample(((i % 80) as i16 - 40) * 100).unwrap();
    }
    w.finalize().unwrap();
    cursor.into_inner()
}

fn manifest(id: &str, audio: &[u8], motion: Option<&[u8]>, duration_ms: u64) -> Value {
    let mut tracks = json!({
        "duck": {"file": "duck.score.json", "sha256": sha256_hex(DUCK), "bytes": DUCK.len()},
        "audio": {"file": "audio.wav", "role": "mix", "codec": "pcm_s16le", "sampleRate": 8000, "channels": 1,
                  "sha256": sha256_hex(audio), "bytes": audio.len()}
    });
    if let Some(m) = motion {
        tracks["motion"] =
            json!({"file": "motion.json", "sha256": sha256_hex(m), "bytes": m.len()});
    }
    json!({"schema": "kivi.show/2", "showId": id, "title": "Test show", "tempoBpm": 96, "durationMs": duration_ms,
           "roles": [{"roleId": "lead", "modelId": "microduck-v1"}], "tracks": tracks})
}

async fn upload(r: &Rig, id: &str, audio: &[u8], motion: Option<&[u8]>) -> (StatusCode, Value) {
    for (name, bytes) in [
        ("duck", Some(DUCK)),
        ("audio", Some(audio)),
        ("motion", motion),
    ] {
        if let Some(b) = bytes {
            let (st, body) = r
                .authed(
                    "PUT",
                    &format!("/api/v1/shows/{id}/tracks/{name}"),
                    Some(b.to_vec()),
                )
                .await;
            assert_eq!(st, StatusCode::OK, "{name}: {body}");
            assert_eq!(body["sha256"], sha256_hex(b));
        }
    }
    let m = manifest(id, audio, motion, 16_000);
    let (st, _) = r
        .authed(
            "PUT",
            &format!("/api/v1/shows/{id}/manifest"),
            Some(m.to_string().into_bytes()),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    r.authed(
        "POST",
        &format!("/api/v1/shows/{id}/commit"),
        Some(b"{}".to_vec()),
    )
    .await
}

#[tokio::test]
async fn nothing_but_info_and_pair_is_open() {
    let r = rig().await;
    assert_eq!(
        r.req("GET", "/api/v1/info", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        r.req("GET", "/api/v1/snapshot", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        r.req("GET", "/api/v1/snapshot", Some("wrong"), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (st, body) = r
        .req(
            "POST",
            "/api/v1/pair",
            None,
            Some(json!({"code": "WRONG123"}).to_string().into_bytes()),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["protocol"], 1);
    assert_eq!(snap["deviceId"], "microduck-test");
    assert!(snap["playback"]["status"].is_string());
}

#[tokio::test]
async fn a_show_is_published_whole_and_only_whole() {
    let r = rig().await;
    let audio = wav(2, 8000);
    // Upload the tracks but corrupt one: the manifest says the audio hash is something else.
    for (name, b) in [("duck", DUCK.to_vec()), ("audio", audio.clone())] {
        r.authed("PUT", &format!("/api/v1/shows/s1/tracks/{name}"), Some(b))
            .await;
    }
    let mut m = manifest("s1", &audio, None, 16_000);
    m["tracks"]["audio"]["sha256"] = json!("0".repeat(64));
    r.authed(
        "PUT",
        "/api/v1/shows/s1/manifest",
        Some(m.to_string().into_bytes()),
    )
    .await;
    let (st, body) = r
        .authed("POST", "/api/v1/shows/s1/commit", Some(b"{}".to_vec()))
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "Corrupt");
    let (_, list) = r.authed("GET", "/api/v1/shows", None).await;
    assert_eq!(
        list["shows"].as_array().unwrap().len(),
        0,
        "a failed commit leaves nothing behind"
    );

    let (st, body) = upload(&r, "s1", &audio, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["stored"], true);
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["songs"][0]["id"], "s1");
    assert_eq!(snap["songs"][0]["duration"], 16.0);
}

#[tokio::test]
async fn an_edit_resends_only_what_changed() {
    let r = rig().await;
    let audio = wav(2, 8000);
    assert_eq!(upload(&r, "s2", &audio, None).await.0, StatusCode::OK);
    let (_, before) = r.authed("GET", "/api/v1/shows", None).await;
    // A motion track is added: only it and the manifest travel, the audio is taken from the library.
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"h","type":"head","startMs":0,"durationMs":2000,"keyframes":[{"tMs":0,"neckPitch":0,"headPitch":0,"headYaw":0,"headRoll":0},{"tMs":1000,"neckPitch":0,"headPitch":0,"headYaw":0.3,"headRoll":0}]}]}"#;
    r.authed(
        "PUT",
        "/api/v1/shows/s2/tracks/motion",
        Some(motion.to_vec()),
    )
    .await;
    let m = manifest("s2", &audio, Some(motion), 16_000);
    r.authed(
        "PUT",
        "/api/v1/shows/s2/manifest",
        Some(m.to_string().into_bytes()),
    )
    .await;
    // Neither the duck nor the audio is re-sent: they are in the published copy, by hash.
    let (st, body) = r
        .authed("POST", "/api/v1/shows/s2/commit", Some(b"{}".to_vec()))
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (_, after) = r.authed("GET", "/api/v1/shows", None).await;
    assert_ne!(
        before["shows"][0]["revision"],
        after["shows"][0]["revision"]
    );
    assert!(after["shows"][0]["tracks"]["motion"].is_object());
}

#[tokio::test]
async fn a_motion_the_robot_cannot_do_is_refused_with_the_event() {
    let r = rig().await;
    let audio = wav(1, 8000);
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"bow","type":"skill","skill":"stage-bow","startMs":0,"durationMs":2000}]}"#;
    let (st, body) = upload(&r, "s3", &audio, Some(motion)).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "ActionUnsupported");
    assert_eq!(body["problems"][0]["eventId"], "bow");
}

#[tokio::test]
async fn playing_asks_the_robot_with_the_audio_and_the_head_clips() {
    let r = rig().await;
    let audio = wav(2, 8000);
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"h","type":"head","startMs":1000,"durationMs":2000,"keyframes":[{"tMs":0,"neckPitch":0,"headPitch":0,"headYaw":0,"headRoll":0},{"tMs":1000,"neckPitch":0,"headPitch":0,"headYaw":0.3,"headRoll":0}]}]}"#;
    assert_eq!(
        upload(&r, "s4", &audio, Some(motion)).await.0,
        StatusCode::OK
    );
    let (st, body) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "play", "songId": "s4", "bootId": r.app.boot_id})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let calls = r.robot.calls.lock().unwrap().clone();
    let sing = calls
        .iter()
        .find_map(|c| match c {
            proto::Call::RobotSing(p) => Some(p.clone()),
            _ => None,
        })
        .expect("a robot.sing");
    assert!(sing.active && sing.audio.as_ref().unwrap().path.ends_with("audio.wav"));
    assert_eq!(
        sing.head_keys.first().map(|k| k.t_ms),
        Some(999),
        "the head returns from zero just before the clip"
    );
    assert!(sing.performance.as_deref().unwrap().contains("syllables"));
    assert_eq!(
        (sing.mouth_latency_ms, sing.head_latency_ms),
        (Some(150.0), Some(250.0))
    );
    // The robot reports it is singing: the snapshot says playing, and where.
    *r.robot.st.lock().unwrap() = Some(state("stand", Some(sing_block(3.5, false)), false));
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["playback"]["status"], "playing");
    assert_eq!(snap["playback"]["songId"], "s4");
    assert_eq!(snap["playback"]["position"], 3.5);
    // …and a second play is refused while it runs.
    let (st, body) = r
        .authed("POST", "/api/v1/shows/s4/play", Some(b"{}".to_vec()))
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // Pause and seek reach the robot; stop ends it and the show can then be updated.
    r.authed(
        "POST",
        "/api/v1/control",
        Some(json!({"action": "pause"}).to_string().into_bytes()),
    )
    .await;
    r.authed(
        "POST",
        "/api/v1/control",
        Some(
            json!({"action": "seek", "position": 8.0})
                .to_string()
                .into_bytes(),
        ),
    )
    .await;
    r.authed(
        "POST",
        "/api/v1/control",
        Some(json!({"action": "stop"}).to_string().into_bytes()),
    )
    .await;
    let calls = r.robot.calls.lock().unwrap().clone();
    let controls: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            proto::Call::RobotSingControl(p) => Some(p.action),
            _ => None,
        })
        .collect();
    assert_eq!(
        controls,
        vec![proto::SingAction::Pause, proto::SingAction::Seek]
    );
    assert!(matches!(calls.last(), Some(proto::Call::RobotSing(p)) if !p.active));
    *r.robot.st.lock().unwrap() = Some(state("stand", None, false));
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["playback"]["status"], "stopped");
}

#[tokio::test]
async fn a_song_that_ends_by_itself_is_reported_ended() {
    let r = rig().await;
    assert_eq!(
        upload(&r, "s5", &wav(1, 8000), None).await.0,
        StatusCode::OK
    );
    r.authed("POST", "/api/v1/shows/s5/play", Some(b"{}".to_vec()))
        .await;
    *r.robot.st.lock().unwrap() = Some(state("stand", Some(sing_block(5.0, false)), false));
    assert_eq!(
        r.authed("GET", "/api/v1/snapshot", None).await.1["playback"]["status"],
        "playing"
    );
    *r.robot.st.lock().unwrap() = Some(state("stand", None, false));
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["playback"]["status"], "ended");
}

#[tokio::test]
async fn the_original_kivi_upload_still_works() {
    let r = rig().await;
    let wav = wav(3, 8000);
    let (st, snap) = r
        .authed(
            "PUT",
            "/api/v1/songs/legacy-1?title=Quack&voice=ah",
            Some(wav),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{snap}");
    assert_eq!(snap["songs"][0]["title"], "Quack");
    assert!((snap["songs"][0]["duration"].as_f64().unwrap() - 3.0).abs() < 0.01);
    let (st, _) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "play", "songId": "legacy-1"})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let calls = r.robot.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, proto::Call::RobotSing(p) if p.active && p.audio.is_some()))
    );
    // A stale base revision is refused, and a restarted duck says so.
    r.authed(
        "POST",
        "/api/v1/control",
        Some(json!({"action": "stop"}).to_string().into_bytes()),
    )
    .await;
    *r.robot.st.lock().unwrap() = Some(state("stand", None, false));
    let (st, _) = r
        .req(
            "PUT",
            "/api/v1/songs/legacy-1?title=Quack",
            Some(&r.token),
            Some(wav_bytes_dummy()),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "stop", "bootId": "old"})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
}

fn wav_bytes_dummy() -> Vec<u8> {
    wav(1, 8000)
}

#[tokio::test]
async fn the_volume_is_kept_and_sent_to_the_robot() {
    let r = rig().await;
    let (st, snap) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "volume", "volume": 35})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(snap["volume"], 35);
    assert!(r.robot.calls.lock().unwrap().iter().any(|c| matches!(c, proto::Call::RobotSingControl(p) if p.action == proto::SingAction::Volume && p.volume == Some(35))));
    assert_eq!(
        r.authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "volume", "volume": 300})
                    .to_string()
                    .into_bytes()
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

/// The sequencer: a skill is started when the robot's own song clock reaches it, only from the
/// pose the track assumed, and the show is stopped — with the reason — when that is not so.
#[tokio::test]
async fn the_sequencer_runs_a_skill_from_the_robots_clock_and_checks_the_pose() {
    let r = rig().await;
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"sit","type":"skill","skill":"sit_toggle","startMs":1000,"durationMs":3000},{"eventId":"body","type":"body","startMs":6000,"durationMs":1000,"keyframes":[{"tMs":0,"z":0,"roll":0,"pitch":0},{"tMs":500,"z":-0.01,"roll":0.1,"pitch":0}]}]}"#;
    // From standing, the body offsets after the sit are refused at upload.
    let (st, body) = upload(&r, "q1", &wav(1, 8000), Some(motion)).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["problems"][0]["eventId"], "body");

    // Sit and stand again is fine.
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"sit","type":"skill","skill":"sit_toggle","startMs":1000,"durationMs":3000},{"eventId":"rise","type":"skill","skill":"sit_toggle","startMs":6000,"durationMs":3000}]}"#;
    assert_eq!(
        upload(&r, "q2", &wav(1, 8000), Some(motion)).await.0,
        StatusCode::OK
    );
    r.authed("POST", "/api/v1/shows/q2/play", Some(b"{}".to_vec()))
        .await;
    let did = |r: &Rig| {
        r.robot
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, proto::Call::RobotDo(_)))
            .count()
    };
    *r.robot.st.lock().unwrap() = Some(state("stand", Some(sing_block(0.5, false)), false));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(did(&r), 0, "not before its time");
    *r.robot.st.lock().unwrap() = Some(state("stand", Some(sing_block(1.2, false)), false));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(did(&r), 1, "started when the clock reached it");
    // The robot sits; the clock runs on to the stand-up; it is issued once, from a sitting pose.
    *r.robot.st.lock().unwrap() = Some(state("sit", Some(sing_block(4.5, false)), false));
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    *r.robot.st.lock().unwrap() = Some(state("sit", Some(sing_block(6.2, false)), false));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(did(&r), 2);
    // The robot is lifted mid-show: the show is stopped and says why.
    let mut lifted = state("stand", Some(sing_block(7.0, false)), false);
    lifted.safety.picked_up = true;
    *r.robot.st.lock().unwrap() = Some(lifted);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, snap) = r.authed("GET", "/api/v1/snapshot", None).await;
    assert_eq!(snap["playback"]["status"], "stopped");
    assert!(
        snap["playback"]["error"]
            .as_str()
            .unwrap()
            .contains("RobotUnsafe"),
        "{snap}"
    );
    assert!(
        matches!(r.robot.calls.lock().unwrap().last(), Some(proto::Call::RobotSing(p)) if !p.active)
    );
}

#[tokio::test]
async fn pause_and_seek_are_refused_inside_an_action_and_across_a_pose_change() {
    let r = rig().await;
    let motion = br#"{"schema":"kivi.motion/1","actions":[{"eventId":"sit","type":"skill","skill":"sit_toggle","startMs":2000,"durationMs":3000}]}"#;
    assert_eq!(
        upload(&r, "q3", &wav(1, 8000), Some(motion)).await.0,
        StatusCode::OK
    );
    r.authed("POST", "/api/v1/shows/q3/play", Some(b"{}".to_vec()))
        .await;
    *r.robot.st.lock().unwrap() = Some(state("stand", Some(sing_block(1.0, false)), false));
    r.authed("GET", "/api/v1/snapshot", None).await;
    // Before the action: pause is fine; a seek past the sit would skip the pose change.
    let (st, _) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(json!({"action": "pause"}).to_string().into_bytes()),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "seek", "position": 8.0})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "PoseMismatch");
    let (st, body) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(
                json!({"action": "seek", "position": 3.0})
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // During the action pause is refused too.
    *r.robot.st.lock().unwrap() = Some(state("sit", Some(sing_block(3.0, false)), false));
    r.authed("GET", "/api/v1/snapshot", None).await;
    let (st, body) = r
        .authed(
            "POST",
            "/api/v1/control",
            Some(json!({"action": "pause"}).to_string().into_bytes()),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "Busy");
}
