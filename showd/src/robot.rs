//! The link to `robotd`: calls, notifications, and the latest state.
//!
//! Everything `showd` does to the robot goes through this trait, so the playback logic and the
//! HTTP layer run in tests against [`FakeRobot`] and on the robot against [`UnixRobot`] with no
//! difference. `robotd` serves one request at a time per connection and a state subscription
//! occupies its connection, so the real link keeps three: one per request (opened as needed),
//! one write-only stream for the 20 Hz intents, and one subscription that feeds the state cache.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use duck_ipc_proto as proto;
use serde_json::Value;

/// A call's answer: the `result` value, or the daemon's message.
pub type CallResult = Result<Value, String>;

#[async_trait::async_trait]
pub trait RobotLink: Send + Sync {
    async fn call(&self, call: proto::Call) -> CallResult;
    /// Fire and forget, in order: a continuous intent (`robot.pose`, `robot.move`).
    fn notify(&self, call: proto::Call);
    /// The newest state, and how old it is.
    fn state(&self) -> Option<(proto::RobotState, Duration)>;
}

const API_VERSION_FALLBACK: u32 = proto::API_VERSION;

fn hello_line() -> String {
    let req = proto::Request::call(
        proto::Id::Number(1),
        &proto::Call::Hello(proto::HelloParams {
            api_version: API_VERSION_FALLBACK,
        }),
    );
    serde_json::to_string(&req).expect("serialises") + "\n"
}

pub struct UnixRobot {
    path: std::path::PathBuf,
    state: Arc<Mutex<Option<(Instant, proto::RobotState)>>>,
    notify_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl UnixRobot {
    /// Start the background tasks: the intent writer and the state subscription.
    pub fn spawn(path: std::path::PathBuf) -> Arc<Self> {
        let state = Arc::new(Mutex::new(None));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let me = Arc::new(Self {
            path: path.clone(),
            state: state.clone(),
            notify_tx: tx,
        });

        // The intent writer: one connection, reopened when it breaks.
        let writer_path = path.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut stream: Option<tokio::net::UnixStream> = None;
            while let Some(line) = rx.recv().await {
                for attempt in 0..2 {
                    if stream.is_none()
                        && let Ok(mut s) = tokio::net::UnixStream::connect(&writer_path).await
                        && s.write_all(hello_line().as_bytes()).await.is_ok()
                    {
                        stream = Some(s);
                    }
                    let Some(s) = stream.as_mut() else { break };
                    if s.write_all(line.as_bytes()).await.is_ok() {
                        break;
                    }
                    stream = None;
                    let _ = attempt;
                }
                // The replies to the hello and to nothing else are never read: drain what arrives
                // so the daemon's side of the socket cannot fill up.
                if let Some(s) = stream.as_mut() {
                    let mut buf = [0u8; 4096];
                    while let Ok(Ok(n)) = tokio::time::timeout(
                        Duration::from_millis(1),
                        tokio::io::AsyncReadExt::read(s, &mut buf),
                    )
                    .await
                    {
                        if n == 0 {
                            stream = None;
                            break;
                        }
                        if n < buf.len() {
                            break;
                        }
                    }
                }
            }
        });

        // The state subscription.
        let sub_path = path;
        let sub_state = state;
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            loop {
                if let Ok(stream) = tokio::net::UnixStream::connect(&sub_path).await {
                    let (r, mut w) = stream.into_split();
                    let sub = proto::Request::call(
                        proto::Id::Number(2),
                        &proto::Call::RobotSubscribe(proto::SubscribeParams { hz: Some(10) }),
                    );
                    let ok = w.write_all(hello_line().as_bytes()).await.is_ok()
                        && w.write_all(
                            (serde_json::to_string(&sub).expect("serialises") + "\n").as_bytes(),
                        )
                        .await
                        .is_ok();
                    if ok {
                        let mut lines = BufReader::new(r).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(req) = serde_json::from_str::<proto::Request>(&line)
                                && let Some(st) = req.as_state()
                            {
                                *sub_state.lock().expect("not poisoned") =
                                    Some((Instant::now(), st));
                            }
                        }
                    }
                }
                *sub_state.lock().expect("not poisoned") = None;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        me
    }
}

#[async_trait::async_trait]
impl RobotLink for UnixRobot {
    async fn call(&self, call: proto::Call) -> CallResult {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let work = async {
            let stream = tokio::net::UnixStream::connect(&self.path)
                .await
                .map_err(|e| format!("cannot reach robotd: {e}"))?;
            let (r, mut w) = stream.into_split();
            let req = proto::Request::call(proto::Id::Number(2), &call);
            w.write_all(hello_line().as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            w.write_all((serde_json::to_string(&req).expect("serialises") + "\n").as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            let mut lines = BufReader::new(r).lines();
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                let Ok(resp) = serde_json::from_str::<proto::Response>(&line) else {
                    continue;
                };
                if resp.id != Some(proto::Id::Number(2)) {
                    continue;
                }
                return match resp.error {
                    Some(e) => Err(e.message),
                    None => Ok(resp.result.unwrap_or(Value::Null)),
                };
            }
            Err("robotd closed the connection".to_owned())
        };
        tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .map_err(|_| "robotd did not answer".to_owned())?
    }

    fn notify(&self, call: proto::Call) {
        let line =
            serde_json::to_string(&proto::Request::notify(&call)).expect("serialises") + "\n";
        let _ = self.notify_tx.send(line);
    }

    fn state(&self) -> Option<(proto::RobotState, Duration)> {
        self.state
            .lock()
            .expect("not poisoned")
            .as_ref()
            .map(|(at, st)| (st.clone(), at.elapsed()))
    }
}

/// A robot in memory, for tests: records what it was asked and answers from a script.
#[derive(Default)]
pub struct FakeRobot {
    pub calls: Mutex<Vec<proto::Call>>,
    pub notes: Mutex<Vec<proto::Call>>,
    pub st: Mutex<Option<proto::RobotState>>,
    /// Answers by method; anything else is `{"accepted": true}`.
    pub answers: Mutex<std::collections::HashMap<String, CallResult>>,
}

#[async_trait::async_trait]
impl RobotLink for FakeRobot {
    async fn call(&self, call: proto::Call) -> CallResult {
        let method = call.method().to_owned();
        self.calls.lock().expect("not poisoned").push(call);
        self.answers
            .lock()
            .expect("not poisoned")
            .get(&method)
            .cloned()
            .unwrap_or_else(|| Ok(serde_json::json!({"accepted": true})))
    }

    fn notify(&self, call: proto::Call) {
        self.notes.lock().expect("not poisoned").push(call);
    }

    fn state(&self) -> Option<(proto::RobotState, Duration)> {
        self.st
            .lock()
            .expect("not poisoned")
            .clone()
            .map(|s| (s, Duration::ZERO))
    }
}
