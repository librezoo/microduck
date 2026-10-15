"""Checks for the card logic: `python test_traits.py`. No test framework, so the Space needs none."""

import datetime as dt

import cards
import traits


def book(**life):
    base = {"powered_s": 360_000, "walking_s": 72_000, "distance_m": 500, "falls": 10}
    base.update(life)
    return {"opened": 1_755_000_000, "life": base, "motors": [], "share": {"robot": "duck", "owner": "u", "shared_at": 1_760_000_000}}


def mood(**life):
    return cards.card_for(book(**life), "u", "duck").status.key


def test_moods_in_priority_order():
    assert mood() == "healthy"
    assert mood(powered_s=600) == "egg"
    assert mood(distance_m=12_000) == "globetrotter"
    assert mood(falls=200) == "clumsy"  # 200 falls in 20 h of walking
    assert mood(sitting_s=200_000) == "couch"
    tripped = book()
    tripped["motors"] = [{"joint": "left_knee", "wear": {"trips": {"overload": 1}}}]
    c = cards.card_for(tripped, "u", "duck")
    assert c.status.key == "vet" and not c.healthy and "left knee" in c.status.line


def test_badges_rarest_first_and_records_count():
    b = book(distance_m=50_000, pets=60, tallies={"photos": 150}, highs={"top_speed_mps": 0.5})
    names = [x.key for x in cards.card_for(b, "u", "duck").badges]
    assert names[:2] == ["marathon", "speed"] or names[:2] == ["speed", "marathon"]
    assert "paparazzi" in names and "good_duck" in names and "first_steps" not in names


def test_facts_say_something_true():
    c = cards.card_for(book(distance_m=84_390, highs={"walk_streak_m": 120}), "u", "duck")
    assert "walked 2.0 marathons" in c.facts
    assert "walked 120 m without a single fall" in c.facts


def test_star_sign_and_season():
    assert traits.star_sign(int(dt.datetime(2026, 10, 13, tzinfo=dt.timezone.utc).timestamp())) == "Libra"
    assert traits.star_sign(int(dt.datetime(2026, 1, 3, tzinfo=dt.timezone.utc).timestamp())) == "Capricorn"
    assert traits.season(int(dt.datetime(2026, 10, 31, tzinfo=dt.timezone.utc).timestamp())) == "halloween"
    assert traits.season(int(dt.datetime(2026, 7, 1, tzinfo=dt.timezone.utc).timestamp())) is None
    assert traits.season(0, "winter") == "winter"


def test_a_book_with_nothing_in_it_still_makes_a_card():
    c = cards.card_for({}, "u", "duck")
    assert c.status.key == "egg" and c.badges == [] and c.style in cards.CARDS


if __name__ == "__main__":
    for name, test in list(globals().items()):
        if name.startswith("test_"):
            test()
            print("ok", name)
