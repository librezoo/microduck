//! What `duckctl udp-pad` shows while it runs: whether the duck is connected, the pad, and what
//! it sends.
//!
//! Two renderings, chosen by where stdout goes, as `robotctl monitor` chooses:
//!
//!  - **a terminal**: a frame repainted in place, at most every [`REDRAW`], in the monitor's
//!    look — bordered blocks, centred bars for the sticks, a trace drawn to a fixed scale — with
//!    the notices in a log block, since the alternate screen has no scrollback.
//!  - **anything else**: one summary line every [`SUMMARY`], because a log of a session should not
//!    be a screen's worth of escape codes.

use std::collections::VecDeque;
use std::io::IsTerminal;
use std::time::{Duration, Instant};

use duck_ipc_proto as proto;
use pad_map::Buttons;
use pad_map::wire::{Packet, RobotReport};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, RenderDirection, Sparkline};

use super::link::{Link, State};

pub const REDRAW: Duration = Duration::from_millis(100);
pub const SUMMARY: Duration = Duration::from_secs(10);

/// The round trip at the top of the trace. A LAN answers in a few ms; a trace that reaches the
/// top is a link worth looking at.
const RTT_FULL_SCALE_MS: u64 = 50;

/// Notices kept for the log block.
const LOG_LINES: usize = 100;

/// Cells of bar in a stick or trigger cell.
const BAR: usize = 15;

const NAMES: [(Buttons, &str); 13] = [
    (Buttons::A, "A"),
    (Buttons::B, "B"),
    (Buttons::X, "X"),
    (Buttons::Y, "Y"),
    (Buttons::LB, "LB"),
    (Buttons::RB, "RB"),
    (Buttons::START, "START"),
    (Buttons::SELECT, "SELECT"),
    (Buttons::UP, "UP"),
    (Buttons::DOWN, "DOWN"),
    (Buttons::LEFT, "LEFT"),
    (Buttons::RIGHT, "RIGHT"),
    (Buttons::HOME, "HOME"),
];

/// The pad on this machine, as last read.
pub struct Pad<'a> {
    pub name: &'a str,
    /// gilrs' answer, already in words: "battery 70%", "wired", or nothing when it cannot tell.
    pub power: Option<String>,
    /// On battery, and little of it left.
    pub low_battery: bool,
    pub last_input: Option<Duration>,
    pub packet: Packet,
}

/// Why there is no pad to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Too early to say there is none.
    Searching,
    /// No gamepad on this machine, or not the one `--pad` names.
    Unplugged { pad: Option<usize> },
    /// Pads here, none of them this duck's yet: the next press on one claims it.
    Press,
    /// Another `udp-pad` on this machine is claiming a pad; this one is next.
    Queued,
}

/// Everything one frame shows.
pub struct View<'a> {
    pub to: &'a str,
    pub pad: Option<Pad<'a>>,
    /// Why `pad` is `None`, when it is.
    pub wait: Wait,
    pub hz: u32,
    /// Sends in the last second.
    pub rate: Option<u32>,
    pub failed: u64,
    pub link: &'a Link,
    /// The robot as `netpadd` last reported it, and how long ago that was.
    pub robot: Option<(&'a RobotReport, Duration)>,
    pub now: Instant,
}

/// A robot report older than this is shown as stale: `netpadd` sends one a second.
const REPORT_STALE: Duration = Duration::from_secs(3);

/// How bad an alarm is. Red is something broken or about to be; yellow is worth knowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warn,
    Alarm,
}

impl Level {
    pub fn mark(self) -> &'static str {
        match self {
            Self::Warn => "⚠",
            Self::Alarm => "✕",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Warn => Style::new().fg(Color::Yellow),
            Self::Alarm => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        }
    }
}

/// One thing wrong with the robot, keyed so a change of level can be told from a number moving.
#[derive(Debug, Clone, PartialEq)]
pub struct Alarm {
    pub key: &'static str,
    pub level: Level,
    pub text: String,
}

/// What is wrong with the robot, by the same rules `robotctl monitor` colours its power row by:
/// thresholds are `robotd`'s or `proto`'s, never invented here — which is why a servo's
/// temperature is shown but never raises one.
pub fn alarms(r: &RobotReport) -> Vec<Alarm> {
    let mut out = Vec::new();
    let mut push = |key, level, text: String| out.push(Alarm { key, level, text });
    if let Some(h) = &r.health {
        if !h.healthy {
            let reason = h.reason.as_deref().unwrap_or("no reason given");
            if h.degraded {
                push("health", Level::Warn, format!("robot degraded: {reason}"));
            } else {
                push("health", Level::Alarm, format!("robot unhealthy: {reason}"));
            }
        }
        if let Some(b) = &h.battery {
            if b.percent <= proto::Battery::CRITICAL_PCT {
                push(
                    "battery",
                    Level::Alarm,
                    format!("battery critical: {:.0}%", b.percent),
                );
            } else if b.percent <= proto::Battery::LOW_PCT {
                push(
                    "battery",
                    Level::Warn,
                    format!("battery low: {:.0}%", b.percent),
                );
            }
        }
        if !h.bus.missing.is_empty() {
            push(
                "motors",
                Level::Alarm,
                format!("motors not answering: {}", h.bus.describe_missing()),
            );
        } else if h.bus.consecutive_errors > 0 {
            push(
                "motors",
                Level::Warn,
                format!(
                    "motor bus: {} read failures running",
                    h.bus.consecutive_errors
                ),
            );
        }
        if let Some(imu) = &h.imu {
            if imu.missing {
                push("imu", Level::Alarm, "IMU missing".to_owned());
            } else if imu.frozen() {
                push(
                    "imu",
                    Level::Alarm,
                    format!(
                        "orientation frozen: {} stale reads",
                        imu.consecutive_stale_blocks
                    ),
                );
            }
        }
        if let Some(t) = h.cpu_throttle.filter(proto::CpuThrottle::throttled) {
            push(
                "cpu",
                Level::Warn,
                format!("cpu throttled to {} MHz", t.khz / 1000),
            );
        }
    }
    if let Some(p) = &r.posture {
        if p.fallen {
            push("fallen", Level::Alarm, "the robot has fallen".to_owned());
        }
        if p.limp {
            push("limp", Level::Warn, "the robot is limp".to_owned());
        }
    }
    out
}

/// Whether this laptop reaches the duck, in one word — the first thing the frame says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Connected,
    /// The duck answers, and refuses this sender: another client holds it.
    Refused,
    NotConnected,
    /// Nothing to say yet: no pad, or the first answer not due.
    Pending,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Self::Connected => "● CONNECTED",
            Self::Refused => "✕ REFUSED",
            Self::NotConnected => "✕ NOT CONNECTED",
            Self::Pending => "… connecting",
        }
    }

    fn color(self) -> Option<Color> {
        match self {
            Self::Connected => Some(Color::Green),
            Self::Refused => Some(Color::Red),
            Self::NotConnected => Some(Color::Red),
            Self::Pending => None,
        }
    }
}

/// The verdict on the link and why, `None` while there is nothing to say — no notice then.
pub fn verdict(to: &str, state: State) -> Option<(Verdict, String)> {
    Some(match state {
        State::Waiting => return None,
        State::Driving => (Verdict::Connected, "driving the duck".to_owned()),
        State::Busy => (
            Verdict::Refused,
            "another client is driving the duck — only one at a time; this one takes over once \
             that one has been quiet for [netpad] timeout_ms"
                .to_owned(),
        ),
        State::Silent(d) => (
            Verdict::NotConnected,
            format!("the duck stopped answering {} ago; still sending", ms(d)),
        ),
        State::NeverAnswered => (
            Verdict::NotConnected,
            format!("nothing answers from {to} — is netpadd running, and up to date?"),
        ),
        State::Refused => (
            Verdict::NotConnected,
            format!("nothing listening on {to} — is [netpad] enabled on the duck?"),
        ),
    })
}

/// The verdict as the frame shows it, with a reason even when there is no notice to give.
fn duck(v: &View) -> (Verdict, String) {
    verdict(v.to, v.link.state(v.now)).unwrap_or_else(|| {
        let why = if v.pad.is_none() {
            "nothing is sent without a pad"
        } else {
            "waiting for the duck's first answer"
        };
        (Verdict::Pending, why.to_owned())
    })
}

/// The link's figures, while there are any to give.
fn figures(link: &Link) -> Vec<String> {
    let mut f = Vec::new();
    if let Some(rtt) = link.rtt() {
        f.push(format!("rtt {:.1} ms", rtt.as_secs_f64() * 1e3));
    }
    if let Some(loss) = link.loss() {
        f.push(format!("loss {:.1}%", loss * 100.0));
    }
    if let Some(stale) = link.stale() {
        f.push(format!("{stale} stale"));
    }
    if let Some(bad) = link.malformed().filter(|n| *n > 0) {
        f.push(format!("{bad} malformed on its port"));
    }
    f
}

fn ms(d: Duration) -> String {
    if d >= Duration::from_secs(10) {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

fn pad_words(v: &View) -> String {
    match &v.pad {
        None => match v.wait {
            Wait::Searching => "looking for a gamepad…".to_owned(),
            Wait::Unplugged { pad: None } => "no gamepad — plug one in".to_owned(),
            Wait::Unplugged { pad: Some(n) } => format!("no gamepad {n} — plug it in"),
            Wait::Press => "press a button on the pad for this duck".to_owned(),
            Wait::Queued => "another udp-pad is claiming a pad — this one is next".to_owned(),
        },
        Some(p) => match &p.power {
            Some(power) => format!("{} · {power}", p.name),
            None => p.name.to_owned(),
        },
    }
}

/// One line for a log: the verdict first, then the pad and the sends.
pub fn summary(v: &View) -> String {
    let (verdict, why) = duck(v);
    let mut duck = format!("{} {why}", verdict.word());
    if verdict == Verdict::Connected {
        for f in figures(v.link) {
            duck += &format!(" · {f}");
        }
    }
    let rate = v.rate.map_or("—".to_owned(), |r| r.to_string());
    let robot = match v.robot {
        None => "robot: no health".to_owned(),
        Some((r, _)) => {
            let alarms = alarms(r);
            if alarms.is_empty() {
                "robot ok".to_owned()
            } else {
                let said: Vec<_> = alarms
                    .iter()
                    .map(|a| format!("{} {}", a.level.mark(), a.text))
                    .collect();
                format!("robot {}", said.join(" · "))
            }
        }
    };
    format!(
        "{duck} | {robot} | {} | {rate} Hz of {} → {} · {} failed",
        pad_words(v),
        v.hz,
        v.to,
        v.failed
    )
}

/// A bar growing from the middle of `width` cells, for a stick at rest at centre — drawn as
/// `robotctl monitor` draws a pad axis, so a pad reads the same in both.
fn centred_bar(value: f64, width: usize) -> String {
    let half = width / 2;
    let offset = (value.clamp(-1.0, 1.0) * half as f64).round();
    let cells = (offset.abs() as usize).min(half);
    if offset < 0.0 {
        format!(
            "{}{}│{}",
            "·".repeat(half - cells),
            "█".repeat(cells),
            "·".repeat(width - half - 1)
        )
    } else {
        format!(
            "{}│{}{}",
            "·".repeat(half),
            "█".repeat(cells),
            "·".repeat(width - half - 1 - cells)
        )
    }
}

fn filled_bar(value: f64, width: usize) -> String {
    let filled = (value.clamp(0.0, 1.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "·".repeat(width - filled))
}

fn axis(name: &str, bar: String, value: f64) -> [Span<'static>; 3] {
    [
        Span::raw(format!(" {name:<3}")),
        Span::styled(bar, Style::new().fg(Color::Cyan)),
        Span::raw(format!("{value:>+7.2}  ")),
    ]
}

/// Every button, the held ones lit: what is *not* held is as much the answer as what is.
fn buttons(held: Buttons) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (bit, name) in NAMES {
        spans.push(if held.contains(bit) {
            Span::styled(
                format!(" {name} "),
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(format!(" {name} ")).dim()
        });
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// The frame on a terminal, and the notices it keeps.
pub struct Screen {
    terminal: DefaultTerminal,
    log: VecDeque<String>,
}

impl Screen {
    /// The full-screen view when stdout is a terminal; `None` otherwise.
    pub fn open() -> Option<Self> {
        std::io::stdout().is_terminal().then(|| Self {
            terminal: ratatui::init(),
            log: VecDeque::new(),
        })
    }

    pub fn notice(&mut self, line: &str) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.to_owned());
    }

    /// Whether a quit key was pressed. Never waits: raw mode delivers Ctrl-C as a key, so it has
    /// to be read here, and reading must not hold up a send.
    pub fn quit(&mut self) -> bool {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read()
                && key.kind == KeyEventKind::Press
                && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)))
            {
                return true;
            }
        }
        false
    }

    pub fn draw(&mut self, v: &View) {
        let log = &self.log;
        let _ = self.terminal.draw(|frame| render(frame, v, log));
    }
}

impl Drop for Screen {
    /// The terminal back as it was, however the loop ends. A panic is covered by the hook
    /// `ratatui::init` installs.
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn render(frame: &mut ratatui::Frame, v: &View, log: &VecDeque<String>) {
    let [duck_area, robot_area, pad_area, log_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Min(3),
    ])
    .areas(frame.area());
    render_duck(frame, v, duck_area);
    render_robot(frame, v, robot_area);
    render_pad(frame, v, pad_area);

    let block = Block::bordered()
        .title(" log ")
        .title_bottom(Line::from(" q quit ").dim());
    let height = usize::from(block.inner(log_area).height);
    let shown: Vec<Line> = log
        .iter()
        .skip(log.len().saturating_sub(height))
        .map(|l| Line::from(format!(" {l}")))
        .collect();
    frame.render_widget(Paragraph::new(shown).block(block), log_area);
}

/// Is the duck there, and how good is the way to it.
fn render_duck(frame: &mut ratatui::Frame, v: &View, area: Rect) {
    let (verdict, why) = duck(v);
    let mut block = Block::bordered()
        .title(Line::from(vec![
            Span::raw(" duck "),
            Span::styled(
                v.to.to_owned(),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_bottom(Line::from(format!(" rtt ≤{RTT_FULL_SCALE_MS} ms ")).dim());
    if let Some(c) = verdict.color() {
        // The whole block in the verdict's colour: seen from across the room, not read.
        block = block.border_style(Style::new().fg(c));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [head, numbers, trace] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);

    let word = match verdict.color() {
        Some(c) => Span::styled(
            verdict.word(),
            Style::new().fg(c).add_modifier(Modifier::BOLD),
        ),
        None => Span::raw(verdict.word()).dim(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            word,
            Span::raw(format!("  {why}")),
        ])),
        head,
    );
    let numbers_line = if verdict == Verdict::Connected {
        Line::from(format!(" {}", figures(v.link).join(" · ")))
    } else {
        // The figures of a link that is not there would read as a link that is.
        Line::from(" —").dim()
    };
    frame.render_widget(Paragraph::new(numbers_line), numbers);

    let [label, spark] =
        Layout::horizontal([Constraint::Length(10), Constraint::Min(8)]).areas(trace);
    frame.render_widget(Paragraph::new(Line::from(" rtt ms").dim()), label);
    // In µs, floored at one level as the monitor's gap trace is: a perfect link must not draw as
    // an empty row, which is what a trace nobody feeds looks like.
    let full = RTT_FULL_SCALE_MS * 1000;
    let floor = full.div_ceil(8);
    let data: Vec<u64> = v
        .link
        .rtts()
        .iter()
        .map(|d| (d.as_micros() as u64).max(floor))
        .collect();
    frame.render_widget(
        Sparkline::default()
            .data(data)
            .direction(RenderDirection::RightToLeft)
            .max(full)
            .style(Style::new().fg(Color::Cyan)),
        spark,
    );
}

/// A label column, nine wide like the monitor's rows.
fn row(label: &str, mut spans: Vec<Span<'static>>) -> Line<'static> {
    spans.insert(0, Span::raw(format!(" {label:<9}")));
    Line::from(spans)
}

/// `text` in the colour of the worst alarm under `key`, plain when there is none.
fn flagged(text: String, key: &str, alarms: &[Alarm]) -> Span<'static> {
    match alarms.iter().find(|a| a.key == key) {
        Some(a) => Span::styled(text, a.level.style()),
        None => Span::raw(text),
    }
}

/// The robot itself: power, motors, posture, health, and what it last refused.
fn render_robot(frame: &mut ratatui::Frame, v: &View, area: Rect) {
    let mut title = vec![Span::raw(" robot ")];
    let Some((report, age)) = v.robot else {
        let why = if v.link.state(v.now) == State::Busy {
            " only the client driving the duck is sent its health"
        } else {
            " no health from netpadd yet — it sends one a second to the driving client"
        };
        let block = Block::bordered().title(Line::from(title));
        frame.render_widget(Paragraph::new(Line::from(why).dim()).block(block), area);
        return;
    };
    let alarms = alarms(report);
    let worst = alarms.iter().map(|a| a.level).max();
    let stale = age >= REPORT_STALE;
    let h = report.health.as_ref();
    title.push(match h {
        Some(h) if h.healthy => Span::styled("healthy", Style::new().fg(Color::Green).bold()),
        Some(h) if h.degraded => Span::styled("degraded", Level::Warn.style()),
        Some(_) => Span::styled("unhealthy", Level::Alarm.style()),
        None => Span::raw("no health").dim(),
    });
    if stale {
        title.push(Span::styled(
            format!(" · {} s old", age.as_secs()),
            Style::new().fg(Color::Yellow),
        ));
    }
    title.push(Span::raw(" "));
    let border = match (stale, worst) {
        (true, _) => Style::new().fg(Color::Yellow),
        (false, Some(l)) => Style::new().fg(l.style().fg.unwrap_or(Color::Reset)),
        (false, None) if h.is_some() => Style::new().fg(Color::Green),
        _ => Style::new(),
    };
    let block = Block::bordered()
        .border_style(border)
        .title(Line::from(title));

    let dim = |t: String| Span::raw(t).dim();
    let mut lines = Vec::new();
    match h {
        None => {
            let why = report.health_error.as_deref().unwrap_or("not reported");
            lines.push(row(
                "power",
                vec![dim(format!("no health from robotd — {why}"))],
            ));
            lines.push(row("motors", vec![dim("—".to_owned())]));
        }
        Some(h) => {
            let mut power = vec![match &h.battery {
                Some(b) => {
                    let text = format!("batt {:.2} V {:.0}%", b.volts, b.percent);
                    match alarms.iter().find(|a| a.key == "battery") {
                        Some(a) => Span::styled(text, a.level.style()),
                        None => Span::styled(text, Style::new().fg(Color::Green)),
                    }
                }
                // Absent is not measured, never an empty pack.
                None => dim("batt not read yet".to_owned()),
            }];
            if let Some(c) = h.cpu_temp_c {
                power.push(Span::raw(" · "));
                power.push(Span::raw(format!("cpu {c:.0} °C")));
            }
            if let Some(t) = h.cpu_throttle.filter(proto::CpuThrottle::throttled) {
                power.push(flagged(
                    format!(" throttled to {} MHz", t.khz / 1000),
                    "cpu",
                    &alarms,
                ));
            }
            if let Some(l) = &h.control_loop {
                let achieved = l
                    .achieved_hz
                    .map_or("—".to_owned(), |hz| format!("{hz:.1}"));
                power.push(Span::raw(format!(
                    " · loop {achieved}/{:.0} Hz",
                    l.target_hz
                )));
            }
            lines.push(row("power", power));

            let mut motors = Vec::new();
            if let Some(m) = &h.motors {
                motors.push(Span::raw(format!(
                    "hottest {} {:.0} °C · mean {:.0} °C · ",
                    m.hottest, m.max_c, m.mean_c
                )));
            }
            motors.push(match alarms.iter().find(|a| a.key == "motors") {
                Some(a) => Span::styled(a.text.clone(), a.level.style()),
                None => Span::styled("bus ok", Style::new().fg(Color::Green)),
            });
            lines.push(row("motors", motors));
        }
    }

    lines.push(row(
        "pose",
        match &report.posture {
            None => {
                let why = report.posture_error.as_deref().unwrap_or("not reported");
                vec![dim(format!("no state from robotd — {why}"))]
            }
            Some(p) => {
                let mut pose = vec![Span::styled(
                    p.policy.clone(),
                    Style::new().fg(Color::Cyan).bold(),
                )];
                pose.push(Span::raw(" · "));
                pose.push(if p.fallen {
                    Span::styled("FALLEN", Level::Alarm.style())
                } else {
                    Span::raw("upright")
                });
                if p.limp {
                    pose.push(Span::styled(" · limp", Level::Warn.style()));
                }
                if p.picked_up {
                    pose.push(Span::styled(" · held", Style::new().fg(Color::Yellow)));
                }
                if let Some(imu) = h.and_then(|h| h.imu.as_ref()) {
                    pose.push(Span::raw(" · "));
                    pose.push(match alarms.iter().find(|a| a.key == "imu") {
                        Some(a) => Span::styled(a.text.clone(), a.level.style()),
                        None if imu.ready => Span::raw("imu ok"),
                        None => dim("imu settling".to_owned()),
                    });
                }
                pose
            }
        },
    ));

    lines.push(row(
        "health",
        match h {
            None => vec![dim("—".to_owned())],
            Some(h) if h.healthy => vec![Span::styled("healthy", Style::new().fg(Color::Green))],
            Some(_) => match alarms.iter().find(|a| a.key == "health") {
                Some(a) => vec![Span::styled(a.text.clone(), a.level.style())],
                None => vec![dim("not healthy".to_owned())],
            },
        },
    ));

    lines.push(row(
        "refused",
        match &report.refused {
            None => vec![dim("nothing".to_owned())],
            Some(r) => vec![
                Span::styled(
                    format!("{} — {}", r.method, r.reason),
                    Style::new().fg(Color::Yellow),
                ),
                dim(format!(" · {} s ago", (r.ago_ms / 1000) + age.as_secs())),
            ],
        },
    ));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The pad, what it holds, and how fast it goes out.
fn render_pad(frame: &mut ratatui::Frame, v: &View, area: Rect) {
    let mut title = vec![Span::raw(" pad ")];
    match &v.pad {
        Some(p) => {
            title.push(Span::styled(
                p.name.to_owned(),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
            if let Some(power) = &p.power {
                let style = if p.low_battery {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().add_modifier(Modifier::DIM)
                };
                title.push(Span::styled(format!(" · {power}"), style));
            }
            title.push(Span::raw(" "));
        }
        None => title.push(Span::styled(
            format!("· {} ", pad_words(v)),
            Style::new().fg(Color::Yellow),
        )),
    }
    let rate = v.rate.map_or("—".to_owned(), |r| r.to_string());
    let failed = if v.failed > 0 {
        Span::styled(
            format!("· {} failed ", v.failed),
            Style::new().fg(Color::Red),
        )
    } else {
        Span::raw("· 0 failed ").dim()
    };
    let block = Block::bordered()
        .title(Line::from(title))
        .title_bottom(Line::from(vec![
            Span::raw(format!(" sending {rate} Hz of {} ", v.hz)).dim(),
            failed,
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(p) = &v.pad else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(" nothing is sent without a pad: the duck stops, then sits").dim(),
                Line::from(" plug a gamepad into this machine — it is picked up as it arrives")
                    .dim(),
            ]),
            inner,
        );
        return;
    };
    let f = p.packet.frame(Buttons::NONE, Buttons::NONE);
    let [left, right, triggers, held, age] =
        Layout::vertical([Constraint::Length(1); 5]).areas(inner);
    let row = |name: &str, a: [Span<'static>; 3], b: [Span<'static>; 3]| {
        let mut spans = vec![Span::raw(format!(" {name:<6}"))];
        spans.extend(a);
        spans.extend(b);
        Paragraph::new(Line::from(spans))
    };
    frame.render_widget(
        row(
            "left",
            axis("X", centred_bar(f.left_x, BAR), f.left_x),
            axis("Y", centred_bar(f.left_y, BAR), f.left_y),
        ),
        left,
    );
    frame.render_widget(
        row(
            "right",
            axis("X", centred_bar(f.right_x, BAR), f.right_x),
            axis("Y", centred_bar(f.right_y, BAR), f.right_y),
        ),
        right,
    );
    frame.render_widget(
        row(
            "trig",
            axis("LT", filled_bar(f.lt, BAR), f.lt),
            axis("RT", filled_bar(f.rt, BAR), f.rt),
        ),
        triggers,
    );
    frame.render_widget(Paragraph::new(buttons(f.held)), held);
    let input = match p.last_input {
        Some(d) => format!(
            " last input {} ago · values as sent, before the duck's deadzone",
            ms(d)
        ),
        None => " values as sent, before the duck's deadzone".to_owned(),
    };
    frame.render_widget(Paragraph::new(Line::from(input).dim()), age);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Every state that is not driving this robot says NOT, and says why.
    #[test]
    fn not_connected_is_said_as_such_with_the_reason() {
        let to = "duck.local:4210";
        let v = |s| verdict(to, s).unwrap();
        assert_eq!(v(State::Driving).0, Verdict::Connected);
        assert_eq!(v(State::Busy).0, Verdict::Refused);
        for s in [
            State::Silent(Duration::from_secs(2)),
            State::NeverAnswered,
            State::Refused,
        ] {
            let (verdict, why) = v(s);
            assert_eq!(verdict, Verdict::NotConnected, "{s:?}");
            assert!(verdict.word().contains("NOT CONNECTED"));
            assert!(!why.is_empty());
        }
        assert!(
            v(State::Refused)
                .1
                .contains("nothing listening on duck.local:4210")
        );
        assert_eq!(verdict(to, State::Waiting), None);
    }

    #[test]
    fn bars_grow_from_the_middle_and_from_the_left() {
        assert_eq!(centred_bar(0.0, 5), "··│··");
        assert_eq!(centred_bar(1.0, 5), "··│██");
        assert_eq!(centred_bar(-1.0, 5), "██│··");
        assert_eq!(filled_bar(0.5, 4), "██··");
    }

    fn view<'a>(link: &'a Link, pad: Option<Pad<'a>>) -> View<'a> {
        View {
            to: "duck.local:4210",
            pad,
            wait: Wait::Unplugged { pad: None },
            hz: 30,
            rate: Some(30),
            failed: 0,
            link,
            robot: None,
            now: Instant::now(),
        }
    }

    fn screen(v: &View) -> String {
        let mut t = Terminal::new(TestBackend::new(100, 28)).unwrap();
        t.draw(|f| render(f, v, &VecDeque::from(["a notice".to_owned()])))
            .unwrap();
        let buf = t.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn the_frame_shows_the_pad_its_buttons_and_the_log() {
        let link = Link::default();
        let packet = Packet::from_axes(1, 0, [0.0, 0.5, 0.0, 0.0], [0.0, 1.0], Buttons::B);
        let s = screen(&view(
            &link,
            Some(Pad {
                name: "Xbox Wireless Controller",
                power: Some("battery 70%".to_owned()),
                low_battery: false,
                last_input: Some(Duration::from_millis(12)),
                packet,
            }),
        ));
        assert!(s.contains("Xbox Wireless Controller · battery 70%"), "{s}");
        assert!(s.contains("+0.50") && s.contains("+1.00"), "{s}");
        assert!(s.contains(" START ") && s.contains(" B "), "{s}");
        assert!(s.contains("connecting"), "{s}");
        assert!(s.contains("a notice"), "{s}");
    }

    fn sick_robot() -> RobotReport {
        RobotReport {
            health: Some(proto::HealthResult {
                healthy: false,
                reason: Some("motor bus not answering".to_owned()),
                battery: Some(proto::Battery {
                    volts: 6.9,
                    percent: 12.0,
                }),
                cpu_temp_c: Some(71.0),
                motors: Some(proto::MotorThermal {
                    hottest: "left_knee".to_owned(),
                    max_c: 58.0,
                    mean_c: 44.0,
                }),
                bus: proto::BusHealth {
                    missing: vec![10],
                    ..Default::default()
                },
                ..Default::default()
            }),
            posture: Some(pad_map::wire::Posture {
                policy: "stand".to_owned(),
                fallen: true,
                ..Default::default()
            }),
            refused: Some(pad_map::wire::Refusal {
                method: "robot.do".to_owned(),
                reason: "not home yet".to_owned(),
                ago_ms: 2_000,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_sick_robot_raises_red_for_what_is_broken_and_nothing_for_a_warm_servo() {
        let keys: Vec<_> = alarms(&sick_robot())
            .into_iter()
            .map(|a| (a.key, a.level))
            .collect();
        assert_eq!(
            keys,
            [
                ("health", Level::Alarm),
                ("battery", Level::Alarm),
                ("motors", Level::Alarm),
                ("fallen", Level::Alarm),
            ]
        );
        let healthy = RobotReport {
            health: Some(proto::HealthResult {
                healthy: true,
                battery: Some(proto::Battery {
                    volts: 7.8,
                    percent: 80.0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(alarms(&healthy).is_empty());
        // Nothing known is nothing wrong — not an empty battery.
        assert!(alarms(&RobotReport::default()).is_empty());
    }

    #[test]
    fn the_robot_block_shows_power_motors_pose_and_refusals() {
        let link = Link::default();
        let r = sick_robot();
        let mut v = view(&link, None);
        v.robot = Some((&r, Duration::from_millis(200)));
        let s = screen(&v);
        assert!(s.contains("unhealthy"), "{s}");
        assert!(
            s.contains("batt 6.90 V 12%") && s.contains("cpu 71 °C"),
            "{s}"
        );
        assert!(s.contains("hottest left_knee 58 °C"), "{s}");
        assert!(s.contains("motors not answering: 10"), "{s}");
        assert!(s.contains("stand · FALLEN"), "{s}");
        assert!(s.contains("robot.do — not home yet"), "{s}");
    }

    #[test]
    fn no_pad_says_so_in_the_pad_block() {
        let link = Link::default();
        let s = screen(&view(&link, None));
        assert!(s.contains("no gamepad — plug one in"), "{s}");
        assert!(summary(&view(&link, None)).starts_with("… connecting"));
    }

    #[test]
    fn an_unclaimed_instance_asks_for_a_press() {
        let link = Link::default();
        let mut v = view(&link, None);
        v.wait = Wait::Press;
        let s = screen(&v);
        assert!(s.contains("press a button on the pad for this duck"), "{s}");
    }
}
