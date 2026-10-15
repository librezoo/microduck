"""The microduck vet book Space: a shareable card for each duck that has shared its book.

A duck shares with `robotctl vetbook` and `s`: `updaterd` publishes the book to a public dataset
on its owner's account, `<user>/<duck>-vetbook` (`updater/src/vetbook.rs` in the microduck
repository), and hands back a link here. This draws the card from that dataset.

- `/d/<user>/<duck>` is the page to share: the card, buttons to post it, and the Open Graph tags
  that make a pasted link unfurl as the card.
- `/d/<user>/<duck>/card.png` is the card, 1200 × 630. `?card=notebook|purikura|profile`
  overrides the one the share drew.

Cards are HTML (`card.html`) rendered by headless Chromium, so what is drawn is exactly what was
designed. A render is a second or so, so each is cached against the dataset's commit: a new share
is a new commit, and the next request draws it again.

`VETBOOK_LOCAL=<dir>` reads `<dir>/<user>/<duck>-vetbook/vetbook.json` instead of the Hub, for
working on the cards without sharing anything.
"""

from __future__ import annotations

import asyncio
import base64
import os
import re
from collections import OrderedDict
from contextlib import asynccontextmanager
from pathlib import Path

import httpx
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import FileResponse, HTMLResponse, Response
from jinja2 import Environment, FileSystemLoader, select_autoescape
from playwright.async_api import async_playwright

import cards

HERE = Path(__file__).parent
HUB = os.environ.get("HF_ENDPOINT", "https://huggingface.co").rstrip("/")
PUBLIC_URL = os.environ.get("VETBOOK_PUBLIC_URL", "https://pollen-robotics-microduck-vetbook.hf.space").rstrip("/")
LOCAL = os.environ.get("VETBOOK_LOCAL")
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,95}$")
CACHE_SIZE = 256

env = Environment(loader=FileSystemLoader(HERE), autoescape=select_autoescape(["html"]))
STICKER = "data:image/webp;base64," + base64.b64encode((HERE / "sticker.webp").read_bytes()).decode()
# The robot's own pixel head (`robotctl vetbook`'s header), plain and in Game Boy greens.
PIXEL = "data:image/png;base64," + base64.b64encode((HERE / "pixel-head.png").read_bytes()).decode()
PIXEL_GB = "data:image/png;base64," + base64.b64encode((HERE / "pixel-head-gb.png").read_bytes()).decode()

state: dict = {}
pngs: OrderedDict[tuple, bytes] = OrderedDict()
render_lock = asyncio.Semaphore(2)


@asynccontextmanager
async def lifespan(_: FastAPI):
    playwright = await async_playwright().start()
    # `VETBOOK_CHROME=chrome` uses the system's Chrome instead of Playwright's own, for local work.
    state["browser"] = await playwright.chromium.launch(
        args=["--no-sandbox"], channel=os.environ.get("VETBOOK_CHROME") or None
    )
    state["http"] = httpx.AsyncClient(timeout=20, follow_redirects=True)
    yield
    await state["http"].aclose()
    await state["browser"].close()
    await playwright.stop()


app = FastAPI(lifespan=lifespan, docs_url=None, redoc_url=None)


def check(user: str, duck: str) -> None:
    if not (NAME.match(user) and NAME.match(duck)):
        raise HTTPException(404, "no such duck")


async def fetch(user: str, duck: str) -> tuple[dict, str] | None:
    """The shared book and the commit it came from, or `None` when there is none."""
    repo = f"{user}/{duck}-vetbook"
    if LOCAL:
        path = Path(LOCAL) / repo / "vetbook.json"
        if not path.exists():
            return None
        import json

        return json.loads(path.read_text()), str(path.stat().st_mtime_ns)
    http: httpx.AsyncClient = state["http"]
    info = await http.get(f"{HUB}/api/datasets/{repo}")
    if info.status_code != 200:
        return None
    sha = info.json().get("sha") or "main"
    book = await http.get(f"{HUB}/datasets/{repo}/resolve/{sha}/vetbook.json")
    if book.status_code != 200:
        return None
    return book.json(), sha


async def render(card: cards.Card, template: str, width: int, height: int) -> bytes:
    html = env.get_template(template).render(
        c=card,
        sticker=STICKER,
        pixel=PIXEL,
        pixel_gb=PIXEL_GB,
        width=width,
        height=height,
        kind="story" if template == "story.html" else None,
    )
    async with render_lock:
        page = await state["browser"].new_page(viewport={"width": width, "height": height})
        try:
            await page.set_content(html, wait_until="networkidle")
            await page.evaluate("document.fonts.ready.then(() => window.fit && window.fit())")
            return await page.screenshot(type="png", clip={"x": 0, "y": 0, "width": width, "height": height})
        finally:
            await page.close()


async def cached_png(
    user: str, duck: str, style: str | None, season: str | None, story: bool, versioned: bool
) -> Response:
    check(user, duck)
    found = await fetch(user, duck)
    if found is None:
        raise HTTPException(404, "no vet book has been shared for this duck")
    book, sha = found
    c = cards.card_for(book, user, duck, style, season)
    template, width, height = ("story.html", 1080, 1920) if story else (f"card_{c.style}.html", 1200, 630)
    key = (user, duck, sha, template, c.season)
    png = pngs.get(key)
    if png is None:
        png = await render(c, template, width, height)
        pngs[key] = png
        while len(pngs) > CACHE_SIZE:
            pngs.popitem(last=False)
    else:
        pngs.move_to_end(key)
    # A versioned address (`?v=<share time>`) names one share's card, which never changes, so it can
    # be kept; the bare address is always the latest share, so nothing may keep it.
    cache = "public, max-age=86400" if versioned else "no-cache"
    return Response(png, media_type="image/png", headers={"Cache-Control": cache})


@app.get("/d/{user}/{duck}/card.png")
async def card_png(
    user: str, duck: str, card: str | None = None, season: str | None = None, v: str | None = None
):
    return await cached_png(user, duck, card, season, story=False, versioned=v is not None)


@app.get("/d/{user}/{duck}/story.png")
async def story_png(user: str, duck: str, season: str | None = None, v: str | None = None):
    """The same duck as a 1080 × 1920 story, for Instagram and TikTok."""
    return await cached_png(user, duck, None, season, story=True, versioned=v is not None)


@app.get("/d/{user}/{duck}", response_class=HTMLResponse)
async def share_page(user: str, duck: str, request: Request):
    check(user, duck)
    url = f"{PUBLIC_URL}/d/{user}/{duck}"
    found = await fetch(user, duck)
    template = env.get_template("page.html")
    if found is None:
        return HTMLResponse(
            template.render(
                card=None,
                heading=("No vet book", "here yet"),
                title="No vet book here yet",
                description="This microduck has not shared its vet book.",
                url=url,
                image=None,
                message=f"No duck called {duck} has shared a vet book on @{user}'s account.",
            ),
            status_code=404,
        )
    book, _ = found
    c = cards.card_for(book, user, duck, request.query_params.get("card"))
    # Each share gets its own image address, so a browser or a social network that kept the last
    # share's card fetches this one instead of showing the old one under the same link.
    version = (book.get("share") or {}).get("shared_at") or 0
    query = f"?v={version}" + (f"&card={c.style}" if request.query_params.get("card") else "")
    return template.render(
        card=c,
        robot=c.robot,
        owner=c.owner,
        dataset=c.dataset,
        title=f"{c.robot}'s vet book",
        description=f"{c.robot} the microduck: {c.walked} {c.walked_unit} walked, {c.awake} awake, "
        f"{c.cuddles} cuddles, {c.falls} oopsie falls.",
        post_text=f"Meet {c.robot}, my microduck ♡ {c.walked} {c.walked_unit} walked, {c.cuddles} cuddles "
        f"and {c.falls} oopsie falls so far.",
        url=url,
        image=f"{url}/card.png{query}",
        # The same duck as a 1080 × 1920 story, for Instagram and TikTok.
        story=f"{url}/story.png?v={version}",
    )


@app.get("/sticker.webp")
async def sticker():
    return FileResponse(HERE / "sticker.webp", media_type="image/webp")


@app.get("/", response_class=HTMLResponse)
async def index():
    return env.get_template("page.html").render(
        card=None,
        heading=("Microduck", "vet book"),
        title="Microduck vet book",
        description="Cards for microducks: hours awake, distance walked, cuddles and falls, shared from the robot.",
        url=PUBLIC_URL,
        image=None,
        message="Every microduck keeps a vet book: its whole life in numbers. Shared, it becomes a card.",
    )


@app.get("/health")
async def health():
    return {"ok": True}
