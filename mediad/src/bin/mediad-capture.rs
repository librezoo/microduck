//! Camera commissioning with mediad's real backend and media.frame server, without WebRTC.
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use clap::Parser;
    #[derive(Parser)]
    struct Args {
        #[arg(long)]
        config: std::path::PathBuf,
        #[arg(long, default_value = "/dev/video0")]
        device: String,
        #[arg(long, default_value = duck_ipc_proto::socket::MEDIA)]
        frame_socket: std::path::PathBuf,
    }
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let params = mediad::config::load(&args.config, true);
    anyhow::ensure!(
        params.media.source == robotd_params::MediaSource::Camera,
        "mediad-capture requires media.source = 'camera'"
    );
    anyhow::ensure!(
        params.media.camera_backend == robotd_params::CameraBackend::Allwinner,
        "commissioning currently supports the Allwinner ISP profile only"
    );
    let camera = mediad::pipeline::Camera {
        device: args.device,
        backend: params.media.camera_backend,
        exposure: 600,
        analogue_gain: 1024,
    };
    // Bind before opening the camera; never run next to a full mediad serving this socket.
    let (_lock, listener) = mediad::frame::bind(&args.frame_socket).await?;
    let (w, h, fps) = params.media.geometry();
    let (pipeline, frames) = mediad::pipeline::start_capture(&camera, w, h, fps)?;
    let result = tokio::select! {
        result = mediad::frame::serve(listener, frames, 0) => result,
        result = tokio::signal::ctrl_c() => result.map_err(Into::into),
    };
    use gstreamer::prelude::*;
    pipeline.set_state(gstreamer::State::Null)?;
    result
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("mediad-capture needs Linux V4L2; use cargo test on the host")
}
