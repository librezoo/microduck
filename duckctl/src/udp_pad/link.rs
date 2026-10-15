//! The link as the sender sees it, read from `netpadd`'s answers (`pad_map::wire::Status`): the
//! round trip, the loss, and whether this sender is the one driving. No I/O and no clock of its
//! own, so every rule is a unit test.
//!
//! Nothing here can slow a send down: what was sent is a fixed ring, and an answer that never comes
//! only makes the figures stale.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use pad_map::wire::{BUSY_EVERY, Status, Verdict};

/// Sends remembered for matching answers. At 100 Hz this is 2.5 s — far longer than any round trip
/// worth reporting on a LAN.
const RING: usize = 256;

/// No answer for this long, after there were some, is the duck not answering. Past `netpadd`'s
/// default `stale_ms`: by then it has stopped the robot.
pub const SILENT: Duration = Duration::from_millis(500);

/// The same, for a sender the duck has refused: it is told so only every [`BUSY_EVERY`], so a
/// silence shorter than a few of those is the refusal standing, not the duck gone.
pub const BUSY_SILENT: Duration = BUSY_EVERY.saturating_mul(3);

/// Sends going out this long with never an answer: a `netpadd` that predates answers, or nothing
/// reaching it.
pub const NEVER: Duration = Duration::from_secs(3);

/// A refusal this recent, with no answer since, is why nothing answers.
const REFUSED_FOR: Duration = Duration::from_secs(2);

/// Loss is the change in the duck's `accepted` count over the change in sends, over this window.
const LOSS_WINDOW: Duration = Duration::from_secs(1);

/// Round trips kept for the trace: more than any terminal is wide.
pub const RTT_SAMPLES: usize = 600;

/// The smoothing of the round trip: a new sample counts for this much.
const RTT_GAIN: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    /// Nothing answered yet, and not for long.
    Waiting,
    /// Answers come back, and this sender has the robot.
    Driving,
    /// Answers come back: another sender has the robot.
    Busy,
    /// There were answers, and there have been none for this long.
    Silent(Duration),
    /// Sends went out for [`NEVER`] and nothing ever answered.
    NeverAnswered,
    /// No answer, and the duck's address said nothing listens on the port.
    Refused,
}

impl State {
    /// The same state for the purpose of saying so: a silence that grows is not news.
    pub fn same_kind(self, other: Self) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }
}

pub struct Link {
    sent: [(u32, Option<Instant>); RING],
    first_sent: Option<Instant>,
    last: Option<(Instant, Verdict)>,
    rtt: Option<Duration>,
    /// Every round trip measured, newest first.
    rtts: VecDeque<Duration>,
    /// Window start: when, and the duck's `accepted` then.
    window: Option<(Instant, u32)>,
    sends_in_window: u32,
    loss: Option<f64>,
    latest: Option<Status>,
    /// `stale` at the first answer: the count shown is this sender's, not the duck's lifetime.
    stale_base: Option<u32>,
    refused: Option<Instant>,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            sent: [(0, None); RING],
            first_sent: None,
            last: None,
            rtt: None,
            rtts: VecDeque::with_capacity(RTT_SAMPLES),
            window: None,
            sends_in_window: 0,
            loss: None,
            latest: None,
            stale_base: None,
            refused: None,
        }
    }
}

impl Link {
    pub fn on_sent(&mut self, seq: u32, now: Instant) {
        self.sent[seq as usize % RING] = (seq, Some(now));
        self.first_sent.get_or_insert(now);
        self.sends_in_window += 1;
    }

    /// No pad, so nothing is sent: a silence that follows is this sender's, not the duck's.
    pub fn paused(&mut self) {
        self.first_sent = None;
        self.last = None;
        self.window = None;
        self.loss = None;
    }

    /// The ICMP "port unreachable" a connected socket reports, on a send or a receive.
    pub fn on_refused(&mut self, at: Instant) {
        self.refused = Some(at);
    }

    pub fn on_status(&mut self, s: Status, at: Instant) {
        self.last = Some((at, s.verdict));
        let (seq, sent) = self.sent[s.seq as usize % RING];
        if let Some(sent) = sent.filter(|_| seq == s.seq) {
            let sample = at.saturating_duration_since(sent);
            if self.rtts.len() == RTT_SAMPLES {
                self.rtts.pop_back();
            }
            self.rtts.push_front(sample);
            self.rtt = Some(match self.rtt {
                None => sample,
                Some(r) => r.mul_f64(1.0 - RTT_GAIN) + sample.mul_f64(RTT_GAIN),
            });
        }
        if s.verdict != Verdict::Driving {
            self.window = None;
            self.loss = None;
            self.latest = Some(s);
            return;
        }
        self.stale_base.get_or_insert(s.stale);
        match self.window {
            None => {
                self.window = Some((at, s.accepted));
                self.sends_in_window = 0;
            }
            Some((start, accepted0)) if at.duration_since(start) >= LOSS_WINDOW => {
                // A restarted netpadd counts from zero again: the window restarts with it.
                let got = s.accepted.wrapping_sub(accepted0);
                if got <= self.sends_in_window.saturating_mul(2) && self.sends_in_window > 0 {
                    let lost = 1.0 - f64::from(got) / f64::from(self.sends_in_window);
                    self.loss = Some(lost.clamp(0.0, 1.0));
                }
                self.window = Some((at, s.accepted));
                self.sends_in_window = 0;
            }
            Some(_) => {}
        }
        self.latest = Some(s);
    }

    pub fn state(&self, now: Instant) -> State {
        let refused = self.refused.is_some_and(|at| {
            now.saturating_duration_since(at) < REFUSED_FOR
                && self.last.is_none_or(|(answered, _)| answered < at)
        });
        if refused {
            return State::Refused;
        }
        match self.last {
            Some((at, verdict)) => {
                let quiet = now.saturating_duration_since(at);
                let limit = if verdict == Verdict::Busy {
                    BUSY_SILENT
                } else {
                    SILENT
                };
                if quiet >= limit {
                    State::Silent(quiet)
                } else if verdict == Verdict::Busy {
                    State::Busy
                } else {
                    State::Driving
                }
            }
            None if self
                .first_sent
                .is_some_and(|at| now.saturating_duration_since(at) >= NEVER) =>
            {
                State::NeverAnswered
            }
            None => State::Waiting,
        }
    }

    pub fn rtt(&self) -> Option<Duration> {
        self.rtt
    }

    /// Every round trip measured, newest first.
    pub fn rtts(&self) -> &VecDeque<Duration> {
        &self.rtts
    }

    pub fn loss(&self) -> Option<f64> {
        self.loss
    }

    /// This sender's datagrams the duck dropped as stale, since its first answer.
    pub fn stale(&self) -> Option<u32> {
        Some(self.latest?.stale.wrapping_sub(self.stale_base?))
    }

    pub fn malformed(&self) -> Option<u32> {
        Some(self.latest?.malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(verdict: Verdict, seq: u32, accepted: u32) -> Status {
        Status {
            verdict,
            seq,
            accepted,
            stale: 0,
            malformed: 0,
        }
    }

    #[test]
    fn an_answer_gives_the_round_trip_of_the_send_it_names() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut l = Link::default();
        l.on_sent(7, ms(0));
        l.on_sent(8, ms(33));
        l.on_status(status(Verdict::Driving, 7, 1), ms(4));
        assert_eq!(l.rtt(), Some(Duration::from_millis(4)));
        assert_eq!(l.state(ms(10)), State::Driving);
        // An answer to a seq long overwritten in the ring gives no sample.
        l.on_status(status(Verdict::Driving, 7 + RING as u32, 2), ms(40));
        assert_eq!(l.rtt(), Some(Duration::from_millis(4)));
    }

    #[test]
    fn loss_is_sends_the_duck_did_not_count_over_a_window() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut l = Link::default();
        l.on_sent(0, ms(0));
        l.on_status(status(Verdict::Driving, 0, 100), ms(1));
        for seq in 1..=10 {
            l.on_sent(seq, ms(100 * u64::from(seq)));
        }
        // Nine of ten arrived.
        l.on_status(status(Verdict::Driving, 10, 109), ms(1_001));
        assert!((l.loss().unwrap() - 0.1).abs() < 1e-9, "{:?}", l.loss());
    }

    #[test]
    fn a_restarted_netpadd_gives_no_loss_figure_rather_than_a_wild_one() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut l = Link::default();
        l.on_sent(0, ms(0));
        l.on_status(status(Verdict::Driving, 0, 5_000), ms(1));
        l.on_sent(1, ms(500));
        l.on_status(status(Verdict::Driving, 1, 1), ms(1_100));
        assert_eq!(l.loss(), None);
    }

    #[test]
    fn busy_silent_and_never_answered_are_told_apart() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut l = Link::default();
        l.on_sent(0, ms(0));
        assert_eq!(l.state(ms(100)), State::Waiting);
        assert_eq!(l.state(ms(3_000)), State::NeverAnswered);
        l.on_status(status(Verdict::Busy, 0, 0), ms(3_001));
        assert_eq!(l.state(ms(3_002)), State::Busy);
        assert_eq!(l.loss(), None, "not driving, so no loss to speak of");
        // Refused, and told so once a second: refused all along, never "gone" in between.
        assert_eq!(l.state(ms(3_900)), State::Busy);
        assert_eq!(
            l.state(ms(3_001) + BUSY_SILENT - Duration::from_millis(1)),
            State::Busy
        );
        assert!(matches!(l.state(ms(3_001) + BUSY_SILENT), State::Silent(_)));
        assert!(State::Silent(SILENT).same_kind(State::Silent(NEVER)));
        l.on_refused(ms(3_700));
        assert_eq!(l.state(ms(3_800)), State::Refused);
        l.on_status(status(Verdict::Driving, 0, 1), ms(3_900));
        assert_eq!(
            l.state(ms(3_901)),
            State::Driving,
            "an answer since outranks it"
        );
        // No pad on this machine: the silence is ours, and starts over.
        l.paused();
        assert_eq!(l.state(ms(9_000)), State::Waiting);
    }
}
