//! The pad on the wire: one UDP datagram per state, 28 bytes, little-endian.
//! `docs/robot/udp-pad.md` owns the format; this is its only implementation.
//!
//! Every datagram is the pad's *whole* state, never a delta, so a lost one costs nothing — the
//! next replaces it — and nothing ever has to be retransmitted or reordered.
//!
//! [`Status`] is the answer `netpadd` sends back, so a sender can see the link: which datagram it
//! answers, and the receiver's running counts. [`RobotReport`] is the robot's health, sent to the
//! driving client every [`REPORT_EVERY`]. A sender may ignore both entirely.

use crate::{Buttons, PadFrame};
use duck_ipc_proto as proto;

pub const MAGIC: [u8; 4] = *b"DKPD";
pub const VERSION: u8 = 1;
pub const LEN: usize = 28;
pub const DEFAULT_PORT: u16 = 4210;

/// The IP TOS byte both ends set on the pad socket: DSCP EF (46), expedited forwarding. An access
/// point honouring WMM puts it in a high-priority queue (voice under RFC 8325, video under the
/// older `dscp >> 3` mapping), ahead of bulk traffic and batched less. A network that ignores it
/// loses nothing.
pub const TOS: u32 = 46 << 2;

const FULL: f64 = 32767.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Packet {
    /// +1 per datagram, wrapping. The receiver drops anything not newer than what it has, or
    /// more than 1000 ahead of it.
    pub seq: u32,
    /// The sender's own clock, ms, starting anywhere; `netpadd` reads from it how late a datagram
    /// arrived. `0` is no clock.
    pub t_ms: u32,
    /// lx ly rx ry (±32767 is ±1, up positive), lt rt (0..32767 is 0..1).
    pub axes: [i16; 6],
    pub buttons: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    Short(usize),
    Magic,
    Version(u8),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short(n) => write!(f, "{n} bytes, a pad datagram is {LEN}"),
            Self::Magic => write!(f, "not a pad datagram (magic)"),
            Self::Version(v) => write!(f, "pad datagram version {v}, this build reads {VERSION}"),
        }
    }
}

impl Packet {
    pub fn encode(&self) -> [u8; LEN] {
        let mut b = [0u8; LEN];
        b[..4].copy_from_slice(&MAGIC);
        b[4] = VERSION;
        b[6..10].copy_from_slice(&self.seq.to_le_bytes());
        b[10..14].copy_from_slice(&self.t_ms.to_le_bytes());
        for (i, a) in self.axes.iter().enumerate() {
            b[14 + 2 * i..16 + 2 * i].copy_from_slice(&a.to_le_bytes());
        }
        b[26..28].copy_from_slice(&self.buttons.to_le_bytes());
        b
    }

    /// Longer than [`LEN`] is fine — appended fields this build does not know are ignored.
    pub fn decode(b: &[u8]) -> Result<Self, WireError> {
        if b.len() < LEN {
            return Err(WireError::Short(b.len()));
        }
        if b[..4] != MAGIC {
            return Err(WireError::Magic);
        }
        if b[4] != VERSION {
            return Err(WireError::Version(b[4]));
        }
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut axes = [0i16; 6];
        for (i, a) in axes.iter_mut().enumerate() {
            *a = i16::from_le_bytes([b[14 + 2 * i], b[15 + 2 * i]]);
        }
        Ok(Self {
            seq: u32_at(6),
            t_ms: u32_at(10),
            axes,
            buttons: u16::from_le_bytes([b[26], b[27]]),
        })
    }

    pub fn held(&self) -> Buttons {
        Buttons(self.buttons & Buttons::KNOWN)
    }

    pub fn reserved(&self) -> u16 {
        self.buttons & !Buttons::KNOWN
    }

    pub fn frame(&self, pressed: Buttons, released: Buttons) -> PadFrame {
        let stick = |v: i16| (f64::from(v) / FULL).clamp(-1.0, 1.0);
        let trigger = |v: i16| (f64::from(v) / FULL).clamp(0.0, 1.0);
        let a = self.axes;
        PadFrame {
            left_x: stick(a[0]),
            left_y: stick(a[1]),
            right_x: stick(a[2]),
            right_y: stick(a[3]),
            lt: trigger(a[4]),
            rt: trigger(a[5]),
            held: self.held(),
            pressed,
            released,
            attitude: None,
            has_imu: false,
        }
    }

    /// For a sender: sticks `[lx, ly, rx, ry]` in -1..1, triggers `[lt, rt]` in 0..1.
    pub fn from_axes(
        seq: u32,
        t_ms: u32,
        sticks: [f64; 4],
        triggers: [f64; 2],
        held: Buttons,
    ) -> Self {
        let q = |v: f64, lo: f64| (v.clamp(lo, 1.0) * FULL).round() as i16;
        Self {
            seq,
            t_ms,
            axes: [
                q(sticks[0], -1.0),
                q(sticks[1], -1.0),
                q(sticks[2], -1.0),
                q(sticks[3], -1.0),
                q(triggers[0], 0.0),
                q(triggers[1], 0.0),
            ],
            buttons: held.0,
        }
    }
}

pub const STATUS_MAGIC: [u8; 4] = *b"DKST";
pub const STATUS_VERSION: u8 = 1;
/// No longer than a pad datagram, so an answer never amplifies what it answers.
pub const STATUS_LEN: usize = 24;
const _: () = assert!(
    STATUS_LEN <= LEN,
    "an answer must never outgrow what it answers"
);

/// How often a client that does not hold the robot is told so, at most. A sender reads a busy
/// answer as standing for at least this long, so the receiver and every sender share one value.
pub const BUSY_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// What became of the datagram a [`Status`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// From the client that holds the robot — taken, or dropped as stale.
    Driving,
    /// Another client holds the robot; this one is dropped until it has been quiet `timeout_ms`.
    Busy,
    /// A code this build does not know, kept so a reader can say so.
    Other(u8),
}

impl Verdict {
    fn code(self) -> u8 {
        match self {
            Self::Driving => 1,
            Self::Busy => 2,
            Self::Other(c) => c,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => Self::Driving,
            2 => Self::Busy,
            c => Self::Other(c),
        }
    }
}

/// `netpadd`'s answer to one pad datagram. The counters are the receiver's own, wrapping, since it
/// started: a sender reads their change, never their value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub verdict: Verdict,
    /// The `seq` of the datagram answered.
    pub seq: u32,
    /// Datagrams taken from the driving client.
    pub accepted: u32,
    /// Datagrams from the driving client dropped as not newer, too far ahead, or late.
    pub stale: u32,
    /// Datagrams that were not pad state at all, from anyone.
    pub malformed: u32,
}

impl Status {
    pub fn encode(&self) -> [u8; STATUS_LEN] {
        let mut b = [0u8; STATUS_LEN];
        b[..4].copy_from_slice(&STATUS_MAGIC);
        b[4] = STATUS_VERSION;
        b[5] = self.verdict.code();
        b[6..10].copy_from_slice(&self.seq.to_le_bytes());
        b[10..14].copy_from_slice(&self.accepted.to_le_bytes());
        b[14..18].copy_from_slice(&self.stale.to_le_bytes());
        b[18..22].copy_from_slice(&self.malformed.to_le_bytes());
        b
    }

    /// Longer than [`STATUS_LEN`] is fine, as for [`Packet::decode`].
    pub fn decode(b: &[u8]) -> Result<Self, WireError> {
        if b.len() < STATUS_LEN {
            return Err(WireError::Short(b.len()));
        }
        if b[..4] != STATUS_MAGIC {
            return Err(WireError::Magic);
        }
        if b[4] != STATUS_VERSION {
            return Err(WireError::Version(b[4]));
        }
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(Self {
            verdict: Verdict::from_code(b[5]),
            seq: u32_at(6),
            accepted: u32_at(10),
            stale: u32_at(14),
            malformed: u32_at(18),
        })
    }
}

pub const REPORT_MAGIC: [u8; 4] = *b"DKRB";
pub const REPORT_VERSION: u8 = 1;
/// How often the driving client is sent a [`RobotReport`].
pub const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// A refusal is reported for this long after it happened, then dropped.
pub const REFUSAL_KEPT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the robot is, as `netpadd` last heard from `robotd`: `magic`, `version`, then JSON.
///
/// JSON rather than a fixed layout because it carries `robotd`'s own [`proto::HealthResult`]
/// unchanged — a field added there reaches a client without a second format to keep in step —
/// and because it goes once a second to one client, where the bytes do not matter and a reader
/// in any language does. Every field is optional: an absent one is *not known*, never zero.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RobotReport {
    /// `robot.health`, as `robotd` last answered it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<proto::HealthResult>,
    /// Why there is no `health`: `robotd` not answering, or not answering in time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posture: Option<Posture>,
    /// Why there is no `posture`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posture_error: Option<String>,
    /// The last request the robot said no to, within [`REFUSAL_KEPT`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<Refusal>,
}

/// What the robot is doing, from `robot.state`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Posture {
    /// The policy that drove the last tick: `walk`, `stand`, `held`, …
    pub policy: String,
    pub fallen: bool,
    pub limp: bool,
    pub picked_up: bool,
}

/// A request from the pad the robot did not carry out, and its reason.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Refusal {
    /// The method, `robot.do`, `robot.enable`, …
    pub method: String,
    pub reason: String,
    /// How long before this report it happened.
    pub ago_ms: u64,
}

#[derive(Debug)]
pub enum ReportError {
    Wire(WireError),
    Json(serde_json::Error),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(e) => e.fmt(f),
            Self::Json(e) => write!(f, "robot report: {e}"),
        }
    }
}

impl RobotReport {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = REPORT_MAGIC.to_vec();
        b.push(REPORT_VERSION);
        // A struct of options, strings and numbers: serialising it cannot fail.
        b.extend(serde_json::to_vec(self).unwrap_or_default());
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, ReportError> {
        if b.len() < 5 {
            return Err(ReportError::Wire(WireError::Short(b.len())));
        }
        if b[..4] != REPORT_MAGIC {
            return Err(ReportError::Wire(WireError::Magic));
        }
        if b[4] != REPORT_VERSION {
            return Err(ReportError::Wire(WireError::Version(b[4])));
        }
        serde_json::from_slice(&b[5..]).map_err(ReportError::Json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_round_trips() {
        let s = Status {
            verdict: Verdict::Busy,
            seq: 0xDEAD_BEEF,
            accepted: 7,
            stale: 2,
            malformed: 1,
        };
        let b = s.encode();
        assert_eq!(&b[..4], b"DKST");
        assert_eq!(b[5], 2);
        assert_eq!(Status::decode(&b), Ok(s));
        assert_eq!(
            Status::decode(&Packet::default().encode()),
            Err(WireError::Magic),
            "a pad datagram is not a status"
        );
        let mut odd = b;
        odd[5] = 9;
        assert_eq!(Status::decode(&odd).unwrap().verdict, Verdict::Other(9));
    }

    /// Everything a report can carry, filled: still one unfragmented datagram on any LAN.
    fn full_report() -> RobotReport {
        RobotReport {
            health: Some(proto::HealthResult {
                healthy: false,
                degraded: false,
                reason: Some("control loop at 43.9 Hz, below 47.5 Hz".to_owned()),
                battery: Some(proto::Battery {
                    volts: 7.62,
                    percent: 61.0,
                }),
                motors: Some(proto::MotorThermal {
                    hottest: "left_knee".to_owned(),
                    max_c: 58.0,
                    mean_c: 44.0,
                    temps_c: Vec::new(),
                    torque_enabled: None,
                }),
                cpu_temp_c: Some(71.2),
                cpu_throttle: Some(proto::CpuThrottle {
                    level: 1,
                    max_level: 4,
                    khz: 1_416_000,
                    max_khz: 1_800_000,
                }),
                bus: proto::BusHealth {
                    consecutive_errors: 3,
                    startup_failures: 0,
                    missing: (10..25).collect(),
                },
                ..Default::default()
            }),
            posture: Some(Posture {
                policy: "walk".to_owned(),
                fallen: true,
                limp: true,
                picked_up: false,
            }),
            refused: Some(Refusal {
                method: "robot.do".to_owned(),
                reason: "the robot is not home yet; try again in a second".to_owned(),
                ago_ms: 1_234,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_robot_report_round_trips_and_fits_one_datagram() {
        let r = full_report();
        let b = r.encode();
        assert_eq!(&b[..5], b"DKRB\x01");
        assert!(b.len() < 1200, "{} bytes", b.len());
        assert_eq!(RobotReport::decode(&b).unwrap(), r);
        assert!(RobotReport::decode(&Packet::default().encode()).is_err());
        // An empty report is every field unknown, not zero.
        let empty = RobotReport::decode(b"DKRB\x01{}").unwrap();
        assert_eq!(empty, RobotReport::default());
    }

    fn sample() -> Packet {
        Packet {
            seq: 0xDEAD_BEEF,
            t_ms: 42,
            axes: [32767, -32767, 0, 100, 32767, 0],
            buttons: 0b1_0100_0001,
        }
    }

    #[test]
    fn a_packet_round_trips_at_28_bytes() {
        let bytes = sample().encode();
        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[..4], b"DKPD");
        assert_eq!(bytes[4], 1);
        assert_eq!(Packet::decode(&bytes), Ok(sample()));
    }

    #[test]
    fn the_layout_is_little_endian_at_the_documented_offsets() {
        let b = sample().encode();
        assert_eq!(
            u32::from_le_bytes(b[6..10].try_into().unwrap()),
            0xDEAD_BEEF
        );
        assert_eq!(u32::from_le_bytes(b[10..14].try_into().unwrap()), 42);
        assert_eq!(i16::from_le_bytes([b[14], b[15]]), 32767);
        assert_eq!(u16::from_le_bytes([b[26], b[27]]), 0b1_0100_0001);
    }

    #[test]
    fn garbage_is_refused_not_panicked_on() {
        assert_eq!(Packet::decode(&[]), Err(WireError::Short(0)));
        assert_eq!(Packet::decode(&[0u8; 27]), Err(WireError::Short(27)));
        let mut b = sample().encode();
        b[0] = b'X';
        assert_eq!(Packet::decode(&b), Err(WireError::Magic));
        let mut b = sample().encode();
        b[4] = 2;
        assert_eq!(Packet::decode(&b), Err(WireError::Version(2)));
    }

    /// Fields can be appended without a version bump: a v1 reader takes the 28 it knows.
    #[test]
    fn a_longer_v1_packet_is_read_and_its_tail_ignored() {
        let mut long = sample().encode().to_vec();
        long.extend_from_slice(&[9, 9, 9, 9]);
        assert_eq!(Packet::decode(&long), Ok(sample()));
    }

    #[test]
    fn i16_min_clamps_to_minus_one() {
        let p = Packet {
            axes: [i16::MIN, 0, 0, 0, i16::MIN, 0],
            ..Default::default()
        };
        let f = p.frame(Buttons::NONE, Buttons::NONE);
        assert_eq!(f.left_x, -1.0);
        assert_eq!(f.lt, 0.0, "a trigger is never negative");
    }

    #[test]
    fn reserved_bits_are_split_from_buttons() {
        let p = Packet {
            buttons: 0xF001,
            ..Default::default()
        };
        assert_eq!(p.held(), Buttons::A | Buttons::HOME, "bit 12 is Home");
        assert_eq!(p.reserved(), 0xE000);
    }

    #[test]
    fn from_axes_inverts_frame() {
        let p = Packet::from_axes(7, 9, [0.5, -1.0, 0.0, 1.0], [1.0, 0.25], Buttons::START);
        let f = p.frame(Buttons::NONE, Buttons::NONE);
        assert!((f.left_x - 0.5).abs() < 1e-4 && f.left_y == -1.0 && f.right_y == 1.0);
        assert!((f.lt - 1.0).abs() < 1e-4 && (f.rt - 0.25).abs() < 1e-4);
        assert_eq!(f.held, Buttons::START);
        assert!(f.attitude.is_none() && !f.has_imu);
    }
}
