use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};
use showd::api::{self, Shared};
use showd::app::{App, Config};
use showd::auth::Auth;
use showd::robot::UnixRobot;
use showd::store::Store;

/// The show service: receives songs from an app, keeps them, and plays them through `robotd`.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Where to listen. 8767, not Kivi Studio's default 8765: on a robot that port belongs to the
    /// live-twin service (`microduck-twin`), and Studio's own device service moved to 8767 for the
    /// same reason. The app takes the port with the address.
    #[arg(long, default_value = "0.0.0.0:8767")]
    listen: SocketAddr,
    /// The library and the pairing state.
    #[arg(long, default_value = "/var/lib/showd")]
    data: PathBuf,
    #[arg(long, default_value = "/run/robotd.sock")]
    robot_socket: PathBuf,
    /// Stable identity. Default: derived from /etc/machine-id.
    #[arg(long)]
    device_id: Option<String>,
    #[arg(long)]
    name: Option<String>,
    /// A fixed pairing code instead of the generated one (tests and bring-up).
    #[arg(long)]
    pair_code: Option<String>,
    /// **Debugging only**: a bearer token that is always accepted. Never on a shipped robot.
    #[arg(long)]
    static_token: Option<String>,
    /// Calibration: how much later than commanded the beak and head move, and the sound arrives.
    /// The defaults are what was measured on the first robot; they are *per robot* numbers.
    #[arg(long, default_value_t = 150.0)]
    mouth_latency_ms: f64,
    #[arg(long, default_value_t = 250.0)]
    head_latency_ms: f64,
    #[arg(long, default_value_t = 0.0)]
    audio_latency_ms: f64,
    /// How big the head moves, 1.0 being each style's own amplitude. (The old restrained styles
    /// were tried at 2.0 on the first robot, sitting: peaks of ~9° yaw, currents unchanged.)
    #[arg(long, default_value_t = 1.0)]
    head_scale: f64,
    /// The performance style: `lively` (big and playful, for a sitting duck), or `ballad`, `pop`,
    /// `chant`; `auto` picks one of those three from the song.
    #[arg(long, default_value = "lively")]
    style: String,
    #[arg(long, default_value_t = 160 << 20)]
    max_show_bytes: u64,
}

fn machine_identity() -> String {
    std::fs::read_to_string("/etc/machine-id")
        .map(|s| {
            Sha256::digest(s.trim().as_bytes())
                .iter()
                .take(4)
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .unwrap_or_else(|_| "00000000".to_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "showd=info".into()),
        )
        .init();
    let a = Args::parse();
    let ident = machine_identity();
    let device_id = a.device_id.unwrap_or_else(|| format!("microduck-{ident}"));
    let name = a
        .name
        .unwrap_or_else(|| format!("MicroDuck-{}", ident[4..].to_uppercase()));

    let store = Store::open(&a.data)
        .with_context(|| format!("cannot open the library at {}", a.data.display()))?;
    store.sweep(24 * 3600 * 1000);
    let auth = Arc::new(Auth::load(
        &a.data,
        a.pair_code,
        a.static_token
            .or_else(|| std::env::var("SHOWD_STATIC_TOKEN").ok()),
    )?);
    let robot = UnixRobot::spawn(a.robot_socket);
    let cfg = Config {
        device_id: device_id.clone(),
        name: name.clone(),
        mouth_latency_ms: a.mouth_latency_ms,
        head_latency_ms: a.head_latency_ms,
        audio_latency_ms: a.audio_latency_ms,
        head_scale: a.head_scale,
        style: (a.style != "auto").then(|| a.style.clone()),
        max_show_bytes: a.max_show_bytes,
        max_events: 2000,
        volume_file: a.data.join("volume"),
    };
    let app = App::new(cfg, store, robot);
    tracing::info!(
        device_id,
        name,
        pairing_code = auth.code(),
        "showd starting"
    );

    // The saved volume goes back onto the robot as soon as it will take it.
    let vol_app = app.clone();
    tokio::spawn(async move {
        for _ in 0..40 {
            if vol_app.robot.state().is_some() {
                vol_app.apply_volume().await;
                return;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });

    let listener = tokio::net::TcpListener::bind(a.listen)
        .await
        .with_context(|| format!("cannot listen on {}", a.listen))?;
    axum::serve(listener, api::router(Shared { app, auth }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
