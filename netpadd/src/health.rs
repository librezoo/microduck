//! What the robot is — its health and its posture — kept fresh for the driving client's report
//! (`pad_map::wire::RobotReport`).
//!
//! **Off the pad loop, on connections of its own.** A health request may take `robotd` up to a
//! second to answer, and the pad loop must never wait a second for anything but a datagram; so
//! two tasks ask, each on its own socket, and publish into a [`watch`] the loop only reads. A
//! task that cannot reach `robotd` says why in the snapshot and tries again — it never ends
//! `netpadd`, because a robot with no health to report still drives.

use std::path::{Path, PathBuf};
use std::time::Duration;

use duck_ipc_proto as proto;
use pad_map::wire::Posture;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;

use crate::robotd::Robotd;

/// How often `robot.health` is asked: as often as the report goes out.
const HEALTH_EVERY: Duration = pad_map::wire::REPORT_EVERY;

/// The rate `robot.state` is subscribed at. Posture changes on a human timescale; a fall is seen
/// within half a second.
const STATE_HZ: u32 = 2;

/// No `robot.state` for this long is a stream that stopped.
const STATE_SILENT: Duration = Duration::from_secs(3);

/// Between attempts to reach `robotd` again.
const RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub health: Option<proto::HealthResult>,
    pub health_error: Option<String>,
    pub posture: Option<Posture>,
    pub posture_error: Option<String>,
}

/// Start both tasks; the receiver always holds the newest snapshot.
pub fn spawn(socket: &Path) -> watch::Receiver<Snapshot> {
    let (tx, rx) = watch::channel(Snapshot {
        health_error: Some("not asked yet".to_owned()),
        posture_error: Some("not asked yet".to_owned()),
        ..Default::default()
    });
    tokio::spawn(poll_health(socket.to_path_buf(), tx.clone()));
    tokio::spawn(follow_state(socket.to_path_buf(), tx));
    rx
}

async fn poll_health(socket: PathBuf, tx: watch::Sender<Snapshot>) {
    let set = |health: Option<proto::HealthResult>, error: Option<String>| {
        tx.send_modify(|s| (s.health, s.health_error) = (health, error));
    };
    loop {
        match Robotd::connect(&socket, HEALTH_EVERY).await {
            Err(e) => set(None, Some(format!("cannot reach robotd: {e}"))),
            Ok(mut robot) => loop {
                match robot.request(&proto::Call::RobotHealth).await {
                    Ok(Some(answer)) => match answer.result_as::<proto::HealthResult>() {
                        Ok(h) => set(Some(h), None),
                        Err(e) => set(None, Some(format!("unreadable robot.health: {e}"))),
                    },
                    Ok(None) => set(None, Some("unreadable robot.health".to_owned())),
                    Err(e) => {
                        set(None, Some(format!("robot.health: {e}")));
                        break;
                    }
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(HEALTH_EVERY).await;
            },
        }
        if tx.is_closed() {
            return;
        }
        tokio::time::sleep(RETRY).await;
    }
}

async fn follow_state(socket: PathBuf, tx: watch::Sender<Snapshot>) {
    loop {
        let why = match subscribe(&socket, &tx).await {
            Ok(()) => "robotd closed the state stream".to_owned(),
            Err(e) => e.to_string(),
        };
        tx.send_modify(|s| (s.posture, s.posture_error) = (None, Some(why)));
        if tx.is_closed() {
            return;
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Follow `robot.state` until it stops; `Ok` when `robotd` closed it.
async fn subscribe(socket: &Path, tx: &watch::Sender<Snapshot>) -> std::io::Result<()> {
    let stream = UnixStream::connect(socket).await?;
    let mut stream = BufReader::new(stream);
    let call = proto::Call::RobotSubscribe(proto::SubscribeParams { hz: Some(STATE_HZ) });
    let mut line = serde_json::to_vec(&proto::Request::call(proto::Id::Number(1), &call))?;
    line.push(b'\n');
    stream.get_mut().write_all(&line).await?;

    let mut text = String::new();
    loop {
        text.clear();
        let n = tokio::time::timeout(STATE_SILENT, stream.read_line(&mut text))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("no robot.state for {} s", STATE_SILENT.as_secs()),
                )
            })??;
        if n == 0 {
            return Ok(());
        }
        if let Some(p) = posture(&text) {
            tx.send_modify(|s| (s.posture, s.posture_error) = (Some(p), None));
        }
        if tx.is_closed() {
            return Ok(());
        }
    }
}

/// The posture in one `robot.state` notification; `None` for any other line — the subscribe
/// acknowledgement, or a state too unlike this build's to read.
///
/// Only the four fields this needs are read, not the whole `RobotState`: a field added to the
/// state, or one an older `robotd` lacks, must not cost the report its posture.
fn posture(line: &str) -> Option<Posture> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("method")?.as_str()? != proto::method::ROBOT_STATE {
        return None;
    }
    let state = v.get("params")?;
    let safety = state.get("safety")?;
    let flag = |k: &str| safety.get(k).and_then(|f| f.as_bool()).unwrap_or(false);
    Some(Posture {
        policy: state.get("policy")?.as_str()?.to_owned(),
        fallen: flag("fallen"),
        limp: flag("limp"),
        picked_up: flag("picked_up"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_posture_is_read_from_a_state_line_and_nothing_else() {
        let state = r#"{"jsonrpc":"2.0","method":"robot.state","params":{"policy":"walk",
            "safety":{"fallen":true,"limp":false,"gravity":[0,0,-1]},"joints":[1,2]}}"#
            .replace('\n', "");
        let p = posture(&state).expect("a posture");
        assert_eq!(p.policy, "walk");
        assert!(
            p.fallen && !p.limp && !p.picked_up,
            "absent picked_up is false"
        );
        assert_eq!(posture(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#), None);
        assert_eq!(posture("not json"), None);
    }
}
