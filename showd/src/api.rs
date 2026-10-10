//! The HTTP API: `/api/v1`.
//!
//! The first group is the LAN protocol Kivi Studio already speaks (`pair`, `info`, `snapshot`,
//! `songs`, `control`), kept byte-compatible so the app connects to a duck unchanged. The second
//! is the show API beside it: capabilities, the library, per-track upload, commit and play.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use show::{AudioRole, AudioTrack, Manifest, Role, TrackFile, Tracks, sha256_hex};

use crate::app::{ApiError, App, PROTOCOL};
use crate::auth::{Auth, PairError};

#[derive(Clone)]
pub struct Shared {
    pub app: Arc<App>,
    pub auth: Arc<Auth>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": self.message, "code": self.code, "problems": self.problems});
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(body),
        )
            .into_response()
    }
}

type Res<T> = Result<T, ApiError>;

pub fn router(shared: Shared) -> Router {
    let authed = Router::new()
        .route("/snapshot", get(snapshot))
        .route("/control", post(control))
        .route("/songs/{id}", put(put_song))
        .route("/capabilities", get(capabilities))
        .route("/shows", get(list_shows))
        .route("/shows/{id}", delete(delete_show))
        .route("/shows/{id}/manifest", put(put_manifest))
        .route("/shows/{id}/tracks/{name}", put(put_track))
        .route("/shows/{id}/commit", post(commit))
        .route("/shows/{id}/play", post(play))
        .layer(middleware::from_fn_with_state(
            shared.clone(),
            require_token,
        ));
    Router::new()
        .route("/api/v1/info", get(info))
        .route("/api/v1/pair", post(pair))
        .nest("/api/v1", authed)
        .with_state(shared)
}

async fn require_token(
    State(s): State<Shared>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !s.auth.check(token) {
        return Err(ApiError::new(
            401,
            "Unauthorized",
            "pair with this duck first",
        ));
    }
    Ok(next.run(req).await)
}

async fn info(State(s): State<Shared>) -> Json<Value> {
    Json(json!({"protocol": PROTOCOL, "deviceId": s.app.cfg.device_id, "name": s.app.cfg.name}))
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

async fn pair(State(s): State<Shared>, Json(b): Json<PairBody>) -> Res<Json<Value>> {
    match s.auth.pair(&b.code) {
        Ok(token) => Ok(Json(
            json!({"protocol": PROTOCOL, "deviceId": s.app.cfg.device_id, "token": token}),
        )),
        Err(PairError::Locked) => Err(ApiError::new(
            429,
            "Locked",
            "too many wrong codes; wait a minute",
        )),
        Err(PairError::Wrong) => Err(ApiError::new(403, "BadCode", "the pairing code is wrong")),
    }
}

async fn snapshot(State(s): State<Shared>) -> Json<crate::app::Snapshot> {
    Json(s.app.snapshot())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlBody {
    action: String,
    song_id: Option<String>,
    revision: Option<String>,
    position: Option<f64>,
    volume: Option<f64>,
    boot_id: Option<String>,
    #[serde(default)]
    mute: bool,
}

async fn control(
    State(s): State<Shared>,
    Json(b): Json<ControlBody>,
) -> Res<Json<crate::app::Snapshot>> {
    if let Some(boot) = &b.boot_id
        && *boot != s.app.boot_id
    {
        return Err(ApiError::new(
            409,
            "Rebooted",
            "the duck restarted; refresh its library",
        ));
    }
    match b.action.as_str() {
        "play" => {
            let id = b
                .song_id
                .ok_or_else(|| ApiError::new(400, "BadRequest", "play needs a songId"))?;
            s.app.play(&id, b.revision.as_deref(), b.mute, None).await?;
        }
        "pause" => {
            s.app.pause().await?;
        }
        "resume" => {
            s.app.resume().await?;
        }
        "stop" => {
            s.app.stop().await?;
        }
        "seek" => {
            let p = b
                .position
                .filter(|p| p.is_finite() && *p >= 0.0)
                .ok_or_else(|| {
                    ApiError::new(400, "BadRequest", "seek needs a position in seconds")
                })?;
            s.app.seek(p).await?;
        }
        "volume" => {
            let v = b
                .volume
                .filter(|v| (0.0..=100.0).contains(v))
                .ok_or_else(|| ApiError::new(400, "BadRequest", "volume is 0..100"))?;
            s.app.set_volume(v.round() as u8).await?;
        }
        other => {
            return Err(ApiError::new(
                400,
                "BadRequest",
                format!("unknown action {other:?}"),
            ));
        }
    }
    Ok(Json(s.app.snapshot()))
}

async fn capabilities(State(s): State<Shared>) -> Json<show::Capabilities> {
    Json(s.app.capabilities().await)
}

async fn list_shows(State(s): State<Shared>) -> Json<Value> {
    let shows: Vec<Value> = s
        .app
        .store
        .list()
        .into_iter()
        .map(|e| {
            let hashes: serde_json::Map<String, Value> = e
                .manifest
                .track_hashes()
                .into_iter()
                .map(|(n, h, b)| (n.to_owned(), json!({"sha256": h, "bytes": b})))
                .collect();
            json!({"showId": e.manifest.show_id, "title": e.manifest.title, "revision": e.meta.revision, "tracks": hashes, "updatedAt": e.meta.updated_at_ms})
        })
        .collect();
    Json(json!({"shows": shows}))
}

async fn delete_show(State(s): State<Shared>, Path(id): Path<String>) -> Res<Json<Value>> {
    s.app.delete(&id)?;
    Ok(Json(json!({"deleted": id})))
}

fn limit(s: &Shared) -> u64 {
    s.app.cfg.max_show_bytes
}

async fn put_track(
    State(s): State<Shared>,
    Path((id, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Res<Json<Value>> {
    let (sha, bytes) = s
        .app
        .store
        .put_track(&id, &name, body.into_data_stream(), limit(&s))
        .await
        .map_err(ApiError::from_problems)?;
    if let Some(expect) = headers.get("x-sha256").and_then(|v| v.to_str().ok())
        && expect != sha
    {
        return Err(ApiError::new(
            422,
            "Corrupt",
            format!("the track arrived as {sha}, not {expect}"),
        ));
    }
    Ok(Json(json!({"track": name, "sha256": sha, "bytes": bytes})))
}

async fn put_manifest(
    State(s): State<Shared>,
    Path(id): Path<String>,
    body: Body,
) -> Res<Json<Value>> {
    let (sha, bytes) = s
        .app
        .store
        .put_track(&id, "manifest", body.into_data_stream(), 1 << 20)
        .await
        .map_err(ApiError::from_problems)?;
    Ok(Json(json!({"sha256": sha, "bytes": bytes})))
}

async fn commit(State(s): State<Shared>, Path(id): Path<String>) -> Res<Json<Value>> {
    let song = s.app.commit(&id).await?;
    Ok(Json(json!({"stored": true, "song": song})))
}

#[derive(Deserialize, Default)]
struct PlayBody {
    #[serde(default)]
    mute: bool,
    revision: Option<String>,
    head_scale: Option<f64>,
}

async fn play(
    State(s): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<PlayBody>>,
) -> Res<Json<Value>> {
    let b = body.map(|j| j.0).unwrap_or_default();
    let view = s.app.play(&id, b.revision.as_deref(), b.mute, None).await?;
    Ok(Json(json!({"accepted": true, "playback": view})))
}

#[derive(Deserialize)]
struct SongQuery {
    title: Option<String>,
    voice: Option<String>,
}

/// Kivi's original upload: one WAV, `PUT /songs/{id}?title=…`. It becomes a show with only an
/// audio track, so the same library, the same atomic publish and the same player serve it.
async fn put_song(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<SongQuery>,
    headers: HeaderMap,
    body: Body,
) -> Res<Json<crate::app::Snapshot>> {
    if !show::manifest::is_safe_id(&id) {
        return Err(ApiError::new(
            400,
            "BadRequest",
            "the song id must be 1–64 of [A-Za-z0-9_-]",
        ));
    }
    if let Some(base) = headers.get("x-base-revision").and_then(|v| v.to_str().ok())
        && !base.is_empty()
        && let Some(existing) = s.app.store.get(&id)
        && existing.meta.revision != base
    {
        return Err(ApiError::new(
            409,
            "Stale",
            "the song changed on the duck since you last synced",
        ));
    }
    let (sha, bytes) = s
        .app
        .store
        .put_track(&id, "audio", body.into_data_stream(), limit(&s))
        .await
        .map_err(ApiError::from_problems)?;
    let path = s
        .app
        .store
        .shows_dir()
        .parent()
        .expect("a root")
        .join("staging")
        .join(&id)
        .join("audio.wav");
    let spec = hound::WavReader::open(&path)
        .map_err(|e| ApiError::new(422, "Corrupt", format!("not a readable WAV: {e}")))?;
    let (rate, channels, frames) = (
        spec.spec().sample_rate,
        spec.spec().channels,
        spec.duration(),
    );
    let duration_ms = (f64::from(frames) * 1000.0 / f64::from(rate)).ceil() as u64;
    let manifest = Manifest {
        schema: show::manifest::SCHEMA.into(),
        show_id: id.clone(),
        title: q
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| id.clone()),
        tempo_bpm: 120.0,
        duration_ms: duration_ms.max(1),
        roles: vec![Role {
            role_id: "lead".into(),
            model_id: "microduck-v1".into(),
        }],
        start_pose: None,
        tracks: Tracks {
            duck: None,
            motion: None,
            audio: Some(AudioTrack {
                file: "audio.wav".into(),
                role: AudioRole::Mix,
                codec: "pcm_s16le".into(),
                sample_rate: rate,
                channels: channels as u8,
                start_offset_ms: 0,
                sha256: sha,
                bytes,
            }),
            expression: None,
        },
    };
    let _ = (
        q.voice,
        TrackFile {
            file: String::new(),
            sha256: String::new(),
            bytes: 0,
        },
        sha256_hex(b""),
    );
    let json = serde_json::to_vec(&manifest).expect("serialises");
    s.app
        .store
        .put_track(
            &id,
            "manifest",
            futures_util::stream::iter([Ok::<_, std::io::Error>(json)]),
            1 << 20,
        )
        .await
        .map_err(ApiError::from_problems)?;
    s.app.commit(&id).await?;
    Ok(Json(s.app.snapshot()))
}
