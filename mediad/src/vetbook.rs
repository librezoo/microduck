//! The camera's lines in the vet book: photos taken, remote visits, and video streamed.
//!
//! The book is `robotd`'s; this daemon reports its counts through `robot.vetbookTally` once a
//! minute. Counting is an atomic add where the thing happens, so it costs nothing on any path;
//! [`report`] is the only task, and it sleeps sixty seconds at a time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use duck_ipc_proto as proto;
use proto::vetbook::{VetbookTallyParams, key};

static PHOTOS: AtomicU64 = AtomicU64::new(0);
static VISITS: AtomicU64 = AtomicU64::new(0);
static VIDEO_MS: AtomicU64 = AtomicU64::new(0);

const EVERY: Duration = Duration::from_secs(60);

/// A `GET /frame` snapshot was served.
pub fn photo() {
    PHOTOS.fetch_add(1, Ordering::Relaxed);
}

/// A peer with the video is in; drop the guard when it leaves, and its time is counted.
pub fn visit() -> Visit {
    VISITS.fetch_add(1, Ordering::Relaxed);
    Visit(std::time::Instant::now())
}

pub struct Visit(std::time::Instant);

impl Drop for Visit {
    fn drop(&mut self) {
        VIDEO_MS.fetch_add(self.0.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
}

/// Report to `robotd` every minute, for as long as the process runs. Counts that could not be
/// delivered — `robotd` restarting — go back on the counters for the next report.
pub async fn report() {
    let socket = PathBuf::from(proto::socket::ROBOT);
    let mut tick = tokio::time::interval(EVERY);
    tick.tick().await;
    loop {
        tick.tick().await;
        let counts = [
            (&PHOTOS, key::PHOTOS, 1.0),
            (&VISITS, key::REMOTE_VISITS, 1.0),
            (&VIDEO_MS, key::VIDEO_S, 0.001),
        ]
        .map(|(counter, name, scale)| (counter, name, scale, counter.swap(0, Ordering::Relaxed)));
        let mut p = VetbookTallyParams::default();
        for (_, name, scale, n) in &counts {
            if *n > 0 {
                p.tallies.insert((*name).to_owned(), *n as f64 * scale);
            }
        }
        if p.is_empty() {
            continue;
        }
        let socket = socket.clone();
        let sent = tokio::task::spawn_blocking(move || {
            proto::vetbook::report(&socket, &p, Duration::from_secs(2))
        })
        .await;
        if !matches!(sent, Ok(Ok(()))) {
            tracing::debug!("vet book: report not delivered; keeping it");
            for (counter, _, _, n) in counts {
                counter.fetch_add(n, Ordering::Relaxed);
            }
        }
    }
}
