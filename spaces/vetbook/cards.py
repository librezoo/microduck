"""What a card says, from a shared vet book.

The book is `vetbook.json` as `updaterd` publishes it (`updater/src/vetbook.rs`): the robot's
`duck_ipc_proto::vetbook::Vetbook`, without diary notes, plus a `share` block naming the robot,
the owner, when, and which card the share drew. Everything here is arithmetic on that — no
network, no browser — so it can be tested on its own.
"""

from __future__ import annotations

import datetime as dt
import hashlib
from dataclasses import dataclass, field

import traits

# The landscape cards a share can draw, 1200 × 630. The story, 1080 × 1920, is offered beside
# whichever was drawn rather than drawn itself: it is a different shape for a different place.
CARDS = ("notebook", "purikura", "profile", "receipt", "passport", "gameboy", "polaroid", "fifa")

# Designs being tried out: drawn only when asked for by name (`?card=`), never by a share.
PROTOTYPES: tuple[str, ...] = ()

# How many palettes each card has, for `traits.palette`.
PALETTES = 4


def _num(value) -> float:
    try:
        return float(value or 0)
    except (TypeError, ValueError):
        return 0.0


def duration(seconds: float) -> str:
    """`148 h`, `35 min`, `40 s` — the largest unit that reads well."""
    if seconds >= 3600 * 10:
        return f"{seconds / 3600:.0f} h"
    if seconds >= 3600:
        return f"{seconds / 3600:.1f} h"
    if seconds >= 60:
        return f"{seconds / 60:.0f} min"
    return f"{seconds:.0f} s"


def distance(metres: float) -> tuple[str, str]:
    """`("18.6", "km")` or `("420", "m")`: the number and its unit, set apart on the cards."""
    if metres >= 1000:
        return f"{metres / 1000:.1f}", "km"
    return f"{metres:.0f}", "m"


def joint_name(joint: str) -> str:
    """`left_knee` → `left knee`; the mouth is a beak."""
    if joint == "mouth":
        return "beak"
    return joint.replace("_", " ")


def stars(value: float, steps: tuple[float, ...]) -> int:
    """One to five: one star, plus one for each step `value` reaches."""
    return min(5, 1 + sum(1 for s in steps if value >= s))


@dataclass
class Card:
    style: str
    robot: str
    owner: str
    dataset: str
    number: str
    birthday: str
    day: int
    awake: str
    awake_hours: float
    walked: str
    walked_unit: str
    falls: int
    cuddles: int
    pets: int
    updates: int
    hottest: int
    servos_replaced: int
    favourite: str | None
    favourite_count: int
    healthy: bool
    spins: int = 0
    songs: int = 0
    naps: int = 0
    carried: str = "0 s"
    carried_s: float = 0.0
    ground_picks: int = 0
    soft_landings: int = 0
    tricks_known: int = 0
    fut: traits.Fut | None = None
    status: traits.Status | None = None
    badges: list = field(default_factory=list)
    facts: list = field(default_factory=list)
    personality: str = ""
    sign: str | None = None
    palette: int = 0
    season: str | None = None
    records: dict = field(default_factory=dict)
    tripped: list[str] = field(default_factory=list)
    memo: str = ""
    energy: int = 1
    cuddliness: int = 1
    clumsiness: int = 1


def card_for(
    book: dict, user: str, duck: str, style: str | None = None, season: str | None = None
) -> Card:
    life = book.get("life") or {}
    share = book.get("share") or {}
    motors = book.get("motors") or []

    opened = int(_num(book.get("opened")))
    shared_at = int(_num(share.get("shared_at"))) or opened
    robot = share.get("robot") or duck
    owner = share.get("owner") or user

    chosen = style if style in CARDS + PROTOTYPES else share.get("card")
    if chosen not in CARDS + PROTOTYPES:
        chosen = CARDS[int(hashlib.sha1(f"{user}/{duck}".encode()).hexdigest(), 16) % len(CARDS)]

    skills = {k: int(_num(v)) for k, v in (life.get("skills") or {}).items()}
    favourite = max(skills, key=lambda k: skills[k]) if skills else None

    tripped = [
        joint_name(m.get("joint", "?"))
        for m in motors
        if sum(int(_num(v)) for v in ((m.get("wear") or {}).get("trips") or {}).values()) > 0
    ]
    retired = sorted(
        (
            (int(_num(r.get("removed"))), joint_name(m.get("joint", "?")), _num((r.get("wear") or {}).get("torqued_s")))
            for m in motors
            for r in (m.get("retired") or [])
        ),
        reverse=True,
    )
    if retired:
        memo = ". ".join(f"{j.capitalize()} replaced after {duration(s)}" for _, j, s in retired[:2]) + "."
    else:
        memo = "No vet visits yet. A very good duck ♡"

    walked, unit = distance(_num(life.get("distance_m")))
    awake_h = _num(life.get("powered_s")) / 3600
    falls = int(_num(life.get("falls")))
    cuddles = int(_num(life.get("pickups")))
    pets = int(_num(life.get("pets")))
    walking_h = _num(life.get("walking_s")) / 3600

    def wear(m, field_):
        return _num((m.get("wear") or {}).get(field_))

    hot = [m for m in motors if wear(m, "hot_s") > 600]
    hardest = max(motors, key=lambda m: wear(m, "charge_as"), default=None)
    busiest = max(motors, key=lambda m: wear(m, "travel_rad"), default=None)
    stats = traits.Stats(
        awake_h=awake_h,
        walking_h=walking_h,
        sitting_h=_num(life.get("sitting_s")) / 3600,
        standing_h=_num(life.get("standing_s")) / 3600,
        in_hands_h=_num(life.get("in_hands_s")) / 3600,
        distance_m=_num(life.get("distance_m")),
        falls=falls,
        soft_landings=int(_num(life.get("soft_landings"))),
        pickups=cuddles,
        pets=pets,
        songs=int(_num(life.get("songs"))),
        singing_h=_num(life.get("singing_s")) / 3600,
        theremin_h=_num(life.get("theremin_s")) / 3600,
        spins=int(_num(life.get("turned_rad")) / (2 * 3.141592653589793)),
        updates=int(_num(life.get("updates"))),
        servos_replaced=sum(len(m.get("retired") or []) for m in motors),
        tripped=tuple(tripped),
        hot_joint=joint_name(max(hot, key=lambda m: wear(m, "hot_s"))["joint"]) if hot else None,
        skills=skills,
        t=life.get("tallies") or {},
        hi=life.get("highs") or {},
        lo=life.get("lows") or {},
        hardest_joint=joint_name(hardest["joint"]) if hardest and wear(hardest, "charge_as") > 0 else None,
        busiest_joint=joint_name(busiest["joint"]) if busiest and wear(busiest, "travel_rad") > 0 else None,
        busiest_turns=int(wear(busiest, "travel_rad") / (2 * 3.141592653589793)) if busiest else 0,
    )
    status = traits.status(stats)

    return Card(
        style=chosen,
        robot=robot,
        owner=owner,
        dataset=f"{user}/{duck}-vetbook",
        number=f"{int(hashlib.sha1(f'{user}/{duck}'.encode()).hexdigest(), 16) % 10000:04d}",
        birthday=dt.datetime.fromtimestamp(opened, dt.timezone.utc).strftime("%Y.%m.%d") if opened else "—",
        day=max(1, (shared_at - opened) // 86400 + 1) if opened else 1,
        awake=duration(_num(life.get("powered_s"))),
        awake_hours=awake_h,
        walked=walked,
        walked_unit=unit,
        falls=falls,
        cuddles=cuddles,
        pets=pets,
        updates=int(_num(life.get("updates"))),
        hottest=round(_num(life.get("hottest_motor_c"))),
        servos_replaced=sum(len(m.get("retired") or []) for m in motors),
        favourite=favourite,
        favourite_count=skills.get(favourite, 0) if favourite else 0,
        healthy=status.healthy,
        tripped=tripped,
        memo=memo,
        # Yaw turned while walking, either way, as whole turns.
        spins=int(_num(life.get("turned_rad")) / (2 * 3.141592653589793)),
        songs=int(_num(life.get("songs"))),
        naps=int(_num(life.get("sit_downs"))),
        carried=duration(_num(life.get("in_hands_s"))),
        carried_s=_num(life.get("in_hands_s")),
        ground_picks=int(_num(life.get("ground_picks"))),
        soft_landings=int(_num(life.get("soft_landings"))),
        tricks_known=len(skills),
        status=status,
        badges=traits.badges(stats),
        fut=traits.fut(stats),
        facts=traits.facts(stats),
        personality=traits.personality(stats),
        sign=traits.star_sign(opened),
        palette=traits.palette(robot, PALETTES),
        season=traits.season(shared_at, season),
        records={
            "top_speed_kmh": _num((life.get("highs") or {}).get("top_speed_mps")) * 3.6,
            "walk_streak_m": _num((life.get("highs") or {}).get("walk_streak_m")),
            "spin_tps": _num((life.get("highs") or {}).get("spin_dps")) / 360,
            "tilt_deg": _num((life.get("highs") or {}).get("tilt_survived_deg")),
            "photos": _num((life.get("tallies") or {}).get("photos")),
            "surface_m2": _num((life.get("tallies") or {}).get("surface_m2")),
            "flashlight_h": _num((life.get("tallies") or {}).get("flashlight_s")) / 3600,
            "sounds": _num((life.get("tallies") or {}).get("sounds")),
        },
        energy=stars(awake_h, (10, 50, 150, 400)),
        cuddliness=stars(cuddles + pets, (10, 50, 150, 400)),
        # Falls per hour of walking, so a duck that walks a lot is allowed to fall a lot — once it
        # has walked for an hour. Before that a rate means nothing, and the count stands in.
        clumsiness=stars(falls / walking_h, (0.5, 1, 2, 4)) if walking_h >= 1 else stars(falls, (5, 20, 50, 120)),
    )
