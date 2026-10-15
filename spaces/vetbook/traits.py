"""The fun on a card: a mood, badges, fun facts, a personality and a star sign, a palette, a season.

All of it is arithmetic on the shared book (`vetbook.json`); see `cards.py` for the book itself and
`duck_ipc_proto::vetbook::key` in the microduck repository for the record names and units.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import math
from dataclasses import dataclass


def num(value) -> float:
    try:
        v = float(value or 0)
    except (TypeError, ValueError):
        return 0.0
    return v if math.isfinite(v) else 0.0


@dataclass(frozen=True)
class Stats:
    """The numbers the traits are judged on, read once from the book."""

    awake_h: float
    walking_h: float
    sitting_h: float
    standing_h: float
    in_hands_h: float
    distance_m: float
    falls: int
    soft_landings: int
    pickups: int
    pets: int
    songs: int
    singing_h: float
    theremin_h: float
    spins: int
    updates: int
    servos_replaced: int
    tripped: tuple[str, ...]
    hot_joint: str | None
    skills: dict
    t: dict  # tallies
    hi: dict  # highs
    lo: dict  # lows
    hardest_joint: str | None
    busiest_joint: str | None
    busiest_turns: int


# ── mood ─────────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Status:
    key: str
    title: str  # big: "Vet time!"
    line: str  # small: "a servo wants a check-up"
    stamp: str  # on the notebook's stamp: "VET?"
    stamp_sub: str  # under it: "CHECK-UP"
    ink: str  # the stamp's colour
    healthy: bool


def status(s: Stats) -> Status:
    """The first that applies, most urgent first."""
    if s.tripped:
        names = ", ".join(s.tripped[:2]) + (" and more" if len(s.tripped) > 2 else "")
        return Status("vet", "Vet time!", f"{names} wants a check-up", "VET?", "CHECK-UP", "#e88a1a", False)
    if s.hot_joint:
        return Status("hot", "Running hot", f"the {s.hot_joint} gets toasty", "HOT!", "COOL DOWN", "#e8541a", True)
    if s.servos_replaced >= 3:
        return Status("bionic", "Bionic duck", f"{s.servos_replaced} new servos and counting", "BIONIC", "UPGRADED", "#3f8fd8", True)
    if s.awake_h < 1:
        return Status("egg", "Fresh out of the egg", "a brand new duckling", "NEW!", "HELLO WORLD", "#3fae8f", True)
    if s.walking_h >= 1 and s.falls / s.walking_h > 2:
        return Status("clumsy", "Clumsy but happy", "falls down, gets back up", "OOPS!", "STILL SMILING", "#8e6ad8", True)
    if s.distance_m >= 10_000:
        return Status("globetrotter", "Globetrotter", f"{s.distance_m / 1000:.1f} km on its little feet", "WOW!", "FAR AND WIDE", "#2f9a8a", True)
    if s.awake_h > 0 and s.in_hands_h / s.awake_h > 0.25:
        return Status("lap", "Lap duck", "happiest in someone's hands", "CUDDLY", "CERTIFIED", "#e2506e", True)
    if s.songs >= 10 or s.singing_h + s.theremin_h >= 0.5:
        return Status("rockstar", "Rockstar", "sings for anyone who listens", "ENCORE", "ON TOUR", "#8e6ad8", True)
    if s.sitting_h > 1 and s.sitting_h > 2 * s.walking_h:
        return Status("couch", "Couch potato", "sits more than it walks", "COMFY", "NO RUSH", "#b0786a", True)
    if s.awake_h >= 500:
        return Status("old", "Old soul", f"{s.awake_h:.0f} hours of wisdom", "WISE", "VETERAN", "#6a5490", True)
    return Status("healthy", "Feeling great!", "healthy as a duck", "OK!", "HEALTHY ♡", "#e23a4e", True)


# ── badges ───────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Badge:
    key: str
    glyph: str
    name: str
    tier: int  # 1 common … 3 rare: the order a card picks them in
    colour: str


def badges(s: Stats) -> list[Badge]:
    """Every badge earned, rarest first."""
    t, hi, lo = s.t, s.hi, s.lo
    rules = [
        ("marathon", "✦", "Marathon", 3, "#ff7eb0", s.distance_m >= 42_195),
        ("first_steps", "♡", "First steps", 1, "#ff7eb0", 10 <= s.distance_m < 42_195),
        ("stuntduck", "☄", "Stuntduck", 2, "#8e6ad8", s.falls >= 100),
        ("soft", "❀", "Soft landings", 2, "#5cc9a7", s.soft_landings >= 20),
        ("cuddles", "♡", "Cuddle magnet", 2, "#e2506e", s.pickups >= 100),
        ("good_duck", "✿", "Good duck", 1, "#ffa94d", s.pets >= 50),
        ("chorister", "♪", "Chorister", 2, "#9b7bff", s.songs >= 10),
        ("spinner", "↻", "Spinner", 2, "#5ab3f0", s.spins >= 1000),
        ("transplant", "✚", "Transplant survivor", 1, "#e88a1a", s.servos_replaced >= 1),
        ("upgrades", "★", "Upgrade addict", 1, "#3fae8f", s.updates >= 10),
        ("old_timer", "☀", "Old timer", 2, "#ffbf3d", s.awake_h >= 100),
        ("night_owl", "☾", "Night owl", 2, "#6a5490", num(t.get("flashlight_s")) >= 3600),
        ("paparazzi", "◉", "Paparazzi", 2, "#ff7eb0", num(t.get("photos")) >= 100),
        ("influencer", "✧", "Influencer", 3, "#9b7bff", num(t.get("video_s")) >= 36_000 or num(t.get("remote_visits")) >= 50),
        ("explorer", "⌖", "Explorer", 2, "#2f9a8a", num(t.get("surface_m2")) >= 500),
        ("nosy", "◎", "Nosy", 1, "#ffa94d", 0 < num(lo.get("closest_mm")) <= 30),
        ("upside_down", "⇅", "Upside-down champ", 2, "#8e6ad8", num(t.get("upside_down_s")) >= 600),
        ("speed", "⚡", "Speed demon", 3, "#ffbf3d", num(hi.get("top_speed_mps")) >= 0.4),
        ("steady", "✓", "Steady feet", 2, "#3fae8f", num(hi.get("walk_streak_m")) >= 100),
        ("balance", "⚖", "Balance master", 3, "#5ab3f0", num(hi.get("tilt_survived_deg")) >= 45),
        ("tornado", "@", "Tornado", 2, "#5ab3f0", num(hi.get("spin_dps")) >= 720),
        ("chatterbox", "♫", "Chatterbox", 1, "#ff7eb0", num(t.get("sounds")) >= 500),
        ("blabbermouth", "◡", "Blabbermouth", 1, "#ffa94d", num(t.get("beak_flaps")) >= 1000),
        ("fashionista", "✂", "Fashionista", 1, "#e2506e", num(t.get("gait_changes")) >= 5),
        ("muscle", "✊", "Muscle duck", 2, "#e8541a", num(hi.get("peak_current_a")) >= 1.2),
        ("battery", "▮", "Marathon battery", 2, "#3fae8f", num(hi.get("on_one_charge_s")) >= 2 * 3600),
        ("hothead", "♨", "Hothead", 1, "#e8541a", num(t.get("throttled_s")) >= 600),
    ]
    earned = [Badge(k, g, n, tier, c) for k, g, n, tier, c, ok in rules if ok]
    return sorted(earned, key=lambda b: -b.tier)


# ── fun facts ────────────────────────────────────────────────────────────────


def facts(s: Stats) -> list[str]:
    """Short sentences, most remarkable first. A card shows as many as it has room for."""
    out: list[tuple[float, str]] = []  # (how remarkable, sentence)
    d = s.distance_m
    if d >= 42_195:
        out.append((9, f"walked {d / 42_195:.1f} marathons"))
    elif d >= 400:
        out.append((6, f"walked {d / 400:.0f} laps of a running track"))
    elif d >= 50:
        out.append((3, f"walked {d / 50:.0f} swimming-pool lengths"))
    if s.walking_h >= 0.5 and s.falls:
        every = s.walking_h * 60 / s.falls
        out.append((5, f"a tumble every {every:.0f} minutes of walking" if every >= 1.5 else "falls about every minute, bless it"))
    if s.falls >= 10 and s.soft_landings:
        out.append((4, f"lands softly {100 * s.soft_landings / s.falls:.0f}% of the time"))
    if s.awake_h >= 1 and s.pickups:
        out.append((4, f"{s.pickups / s.awake_h:.1f} cuddles an hour"))
    streak = num(s.hi.get("walk_streak_m"))
    if streak >= 20:
        out.append((6, f"walked {streak:.0f} m without a single fall"))
    speed = num(s.hi.get("top_speed_mps"))
    if speed >= 0.1:
        out.append((5, f"top speed {speed * 3.6:.1f} km/h"))
    spin = num(s.hi.get("spin_dps"))
    if spin >= 360:
        out.append((5, f"once spun at {spin / 360:.1f} turns a second"))
    tilt = num(s.hi.get("tilt_survived_deg"))
    if tilt >= 25:
        out.append((6, f"leaned {tilt:.0f}° and did not fall"))
    if num(s.t.get("upside_down_s")) >= 60:
        out.append((4, f"spent {num(s.t['upside_down_s']) / 60:.0f} minutes upside down"))
    if num(s.t.get("photos")) >= 10:
        out.append((4, f"took {num(s.t['photos']):.0f} photos"))
    if num(s.t.get("surface_m2")) >= 20:
        out.append((5, f"its laser has scanned {num(s.t['surface_m2']):,.0f} m² of new surfaces"))
    if 0 < num(s.lo.get("closest_mm")) <= 50:
        out.append((3, f"once put its beak {num(s.lo['closest_mm']):.0f} mm from something"))
    if num(s.t.get("flashlight_s")) >= 600:
        out.append((4, f"kept its flashlight on {num(s.t['flashlight_s']) / 3600:.1f} h"))
    if num(s.t.get("beak_flaps")) >= 100:
        out.append((3, f"flapped its beak {num(s.t['beak_flaps']):,.0f} times"))
    if s.busiest_joint and s.busiest_turns >= 100:
        out.append((3, f"its {s.busiest_joint} has turned {s.busiest_turns:,} times"))
    if s.hardest_joint:
        out.append((2, f"its {s.hardest_joint} works hardest"))
    charge = num(s.hi.get("on_one_charge_s"))
    if charge >= 1800:
        out.append((3, f"ran {charge / 3600:.1f} h on one charge"))
    return [sentence for _, sentence in sorted(out, key=lambda x: -x[0])]


# ── personality and star sign ────────────────────────────────────────────────


def personality(s: Stats) -> str:
    """What it spends most of its time being."""
    performing = s.singing_h + s.theremin_h + 0.02 * sum(int(num(v)) for v in s.skills.values())
    shares = {
        "an adventurer": s.walking_h,
        "a people duck": s.in_hands_h * 1.5,
        "a philosopher": s.standing_h * 0.8,
        "a napper": s.sitting_h,
        "a performer": performing * 3,
    }
    best = max(shares, key=lambda k: shares[k])
    return best if shares[best] > 0 else "a mystery"


SIGNS = [
    ((1, 20), "Capricorn"), ((2, 19), "Aquarius"), ((3, 20), "Pisces"), ((4, 20), "Aries"),
    ((5, 21), "Taurus"), ((6, 21), "Gemini"), ((7, 22), "Cancer"), ((8, 23), "Leo"),
    ((9, 23), "Virgo"), ((10, 23), "Libra"), ((11, 22), "Scorpio"), ((12, 22), "Sagittarius"),
    ((12, 32), "Capricorn"),
]


def star_sign(opened: int) -> str | None:
    """From the day the book opened — its birthday, as far as anyone knows."""
    if not opened:
        return None
    d = dt.datetime.fromtimestamp(opened, dt.timezone.utc)
    for (month, day), name in SIGNS:
        if (d.month, d.day) < (month, day):
            return name
    return "Capricorn"


# ── palette and season ───────────────────────────────────────────────────────


def palette(robot: str, count: int) -> int:
    """Which of a card's palettes this duck wears: fixed by its name, so a duck keeps its colours."""
    return int(hashlib.sha1(robot.encode()).hexdigest(), 16) % count


def season(when: int, override: str | None = None) -> str | None:
    """A seasonal skin for the day the book was shared, or the one asked for."""
    if override in ("halloween", "winter", "valentine", "none"):
        return None if override == "none" else override
    if not when:
        return None
    d = dt.datetime.fromtimestamp(when, dt.timezone.utc)
    if (d.month == 10 and d.day >= 20) or (d.month == 11 and d.day <= 2):
        return "halloween"
    if d.month == 12 or (d.month == 1 and d.day <= 6):
        return "winter"
    if d.month == 2 and 7 <= d.day <= 15:
        return "valentine"
    return None


# ── a football card ──────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Fut:
    overall: int
    position: str
    tier: str  # bronze, silver, gold, icon
    attributes: tuple  # six (code, value), in the card's two columns


def rating(value: float, scale: float) -> int:
    """40 for nothing, approaching 99: `scale` is the value that earns about 77."""
    return min(99, round(40 + 59 * (1 - math.exp(-max(0.0, value) / scale))))


POSITIONS = {
    "an adventurer": "ADV",
    "a people duck": "CUD",
    "a philosopher": "PHI",
    "a napper": "NAP",
    "a performer": "SHO",
    "a mystery": "???",
}


def fut(s: Stats) -> Fut:
    """Six attributes from the book, 40 to 99, and an overall from them."""
    skills_used = sum(int(num(v)) for v in s.skills.values())
    balance = (rating(num(s.hi.get("walk_streak_m")), 80) + rating(num(s.hi.get("tilt_survived_deg")), 20)) / 2
    attrs = (
        ("SPD", rating(num(s.hi.get("top_speed_mps")), 0.25)),
        ("WLK", rating(s.distance_m, 5000)),
        ("HUG", rating(s.pickups + s.pets, 150)),
        ("BAL", round(balance)),
        ("TRK", rating(skills_used, 60)),
        ("STA", rating(s.awake_h, 100)),
    )
    overall = round(sum(v for _, v in attrs) / len(attrs))
    tier = "icon" if s.awake_h >= 500 and overall >= 80 else "gold" if overall >= 75 else "silver" if overall >= 65 else "bronze"
    return Fut(overall, POSITIONS.get(personality(s), "DCK"), tier, attrs)
