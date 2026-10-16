#!/usr/bin/env python3
"""Record bounded raw samples for model validation/calibration, using the shared frame socket."""

import argparse
import json
import time
from pathlib import Path

from raw_client import fetch


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--count", type=int, default=30)
    p.add_argument("--hz", type=float, default=1)
    p.add_argument("--socket", default="/run/mediad/media.sock")
    a = p.parse_args()
    if not 1 <= a.count <= 1000 or not 0 < a.hz <= 10:
        p.error("count must be 1..1000 and hz must be (0,10]")
    a.output.mkdir(parents=True, exist_ok=False)
    durations = []
    for index in range(a.count):
        started = time.monotonic()
        header, data = fetch(a.socket)
        durations.append((time.monotonic() - started) * 1000)
        stem = a.output / f"frame-{index:04d}"
        stem.with_suffix(".uyvy").write_bytes(data)
        stem.with_suffix(".json").write_text(json.dumps(header) + "\n")
        print(
            json.dumps({"index": index, "fetch_ms": durations[-1], **header}),
            flush=True,
        )
        if index + 1 < a.count:
            time.sleep(max(0, 1 / a.hz - (time.monotonic() - started)))
    (a.output / "recording.json").write_text(
        json.dumps(
            {
                "count": a.count,
                "requested_hz": a.hz,
                "fetch_ms": durations,
                "lens_nominal_degrees": 200,
                "rotate": 0,
                "undistort": False,
                "calibration_status": "unlabelled_smoke_samples_not_accuracy_validation",
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
