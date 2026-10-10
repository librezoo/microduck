#!/usr/bin/env python3
"""A reference client for `showd`: what an app does, in the order it does it.

    push_show.py --host 192.168.1.50:8765 --pair-code ABCD2345 --duck duck.score.json \
        --motion motion.json --audio mix.wav --id my-song --title "My song" --play

It pairs (or takes --token), uploads the tracks that the duck does not already have, uploads the
manifest, commits, and optionally plays and follows the playback. Plain standard library: it is
documentation as much as a tool, and the app's `ShowAdapter` makes the same calls.
"""
import argparse, hashlib, json, sys, time, urllib.error, urllib.request, wave


def call(base, token, method, path, body=None, headers=None):
    req = urllib.request.Request(base + "/api/v1" + path, data=body, method=method)
    if token:
        req.add_header("Authorization", "Bearer " + token)
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    if body is not None and "Content-Type" not in (headers or {}):
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read() or b"{}")
        except Exception:
            return e.code, {"error": str(e)}


def sha(b):
    return hashlib.sha256(b).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", required=True)
    ap.add_argument("--token")
    ap.add_argument("--pair-code")
    ap.add_argument("--duck", help="duck.score.json")
    ap.add_argument("--motion", help="motion.json")
    ap.add_argument("--audio", help="audio.wav (PCM16)")
    ap.add_argument("--id", required=True)
    ap.add_argument("--title", default=None)
    ap.add_argument("--duration-ms", type=int, default=0, help="default: the audio's length")
    ap.add_argument("--tempo", type=float, default=None)
    ap.add_argument("--start-pose", choices=["standing", "sitting"])
    ap.add_argument("--play", action="store_true")
    ap.add_argument("--mute", action="store_true")
    a = ap.parse_args()
    base = "http://" + a.host

    token = a.token
    if not token:
        st, r = call(base, None, "POST", "/pair", json.dumps({"code": a.pair_code}).encode())
        if st != 200:
            sys.exit(f"pairing failed: {r}")
        token = r["token"]
        print("paired; token:", token)

    tracks, blobs = {}, {}
    if a.duck:
        blobs["duck"] = open(a.duck, "rb").read()
        tracks["duck"] = {"file": "duck.score.json", "sha256": sha(blobs["duck"]), "bytes": len(blobs["duck"])}
    if a.motion:
        blobs["motion"] = open(a.motion, "rb").read()
        tracks["motion"] = {"file": "motion.json", "sha256": sha(blobs["motion"]), "bytes": len(blobs["motion"])}
    duration_ms = a.duration_ms
    if a.audio:
        blobs["audio"] = open(a.audio, "rb").read()
        with wave.open(a.audio) as w:
            rate, ch = w.getframerate(), w.getnchannels()
            duration_ms = duration_ms or int(w.getnframes() * 1000 / rate)
        tracks["audio"] = {"file": "audio.wav", "role": "mix", "codec": "pcm_s16le", "sampleRate": rate, "channels": ch,
                           "startOffsetMs": 0, "sha256": sha(blobs["audio"]), "bytes": len(blobs["audio"])}
    tempo = a.tempo or (json.loads(blobs["duck"])["tempoBpm"] if "duck" in blobs else 120.0)
    manifest = {"schema": "kivi.show/2", "showId": a.id, "title": a.title or a.id, "tempoBpm": tempo,
                "durationMs": duration_ms or 1000,
                "roles": [{"roleId": "lead", "modelId": "microduck-v1"}], "tracks": tracks}
    if a.start_pose:
        manifest["startPose"] = a.start_pose

    # Only what the duck does not already have: the library lists every track by hash.
    st, lib = call(base, token, "GET", "/shows")
    have = {}
    for s in lib.get("shows", []):
        if s["showId"] == a.id:
            have = {n: t["sha256"] for n, t in s["tracks"].items()}
    for name, blob in blobs.items():
        if have.get(name) == sha(blob):
            print(f"  {name}: already on the duck")
            continue
        t0 = time.time()
        st, r = call(base, token, "PUT", f"/shows/{a.id}/tracks/{name}", blob,
                     {"Content-Type": "application/octet-stream", "X-Sha256": sha(blob)})
        if st != 200:
            sys.exit(f"upload {name} failed: {st} {r}")
        print(f"  {name}: sent {len(blob)} bytes in {time.time() - t0:.1f} s")
    st, r = call(base, token, "PUT", f"/shows/{a.id}/manifest", json.dumps(manifest).encode())
    if st != 200:
        sys.exit(f"manifest failed: {st} {r}")
    st, r = call(base, token, "POST", f"/shows/{a.id}/commit", b"{}")
    if st != 200:
        print("REFUSED:", r.get("error"))
        for p in r.get("problems", []):
            print("  -", p)
        sys.exit(1)
    print("stored:", r["song"]["title"], r["song"]["revision"][:12])

    if a.play:
        st, r = call(base, token, "POST", f"/shows/{a.id}/play", json.dumps({"mute": a.mute}).encode())
        if st != 200:
            sys.exit(f"play refused: {r}")
        print("playing")
        while True:
            time.sleep(1)
            st, snap = call(base, token, "GET", "/snapshot")
            pb = snap["playback"]
            print(f"  {pb['status']:8} {pb['position']:6.1f}/{pb['duration']:.0f} s", pb.get("error") or "")
            if pb["status"] in ("ended", "stopped"):
                break


main()
