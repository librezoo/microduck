#!/usr/bin/env python3
"""Create lossless, letterboxed calibration images and a disjoint holdout from raw records."""

import argparse
import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from raw_client import validate
from yoloe_infer import letterbox


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--records", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--holdout", type=int, default=5)
    a = p.parse_args()
    sources = sorted(a.records.glob("*.uyvy"))
    if not 1 <= a.holdout < len(sources):
        p.error("need at least one calibration and one holdout frame")
    a.output.mkdir(parents=True, exist_ok=False)
    rows = []
    for i, source in enumerate(sources):
        header = json.loads(source.with_suffix(".json").read_text())
        validate(header)
        data = source.read_bytes()
        if len(data) != header["bytes"]:
            raise ValueError(f"truncated frame: {source}")
        rgb = cv2.cvtColor(
            np.frombuffer(data, np.uint8).reshape(header["height"], header["width"], 2),
            cv2.COLOR_YUV2RGB_UYVY,
        )
        tensor, _ = letterbox(rgb)
        image = np.rint(tensor[0].transpose(1, 2, 0) * 255).astype(np.uint8)
        target = a.output / (source.stem + ".png")
        if not cv2.imwrite(str(target), cv2.cvtColor(image, cv2.COLOR_RGB2BGR)):
            raise OSError(f"cannot write {target}")
        rows.append(
            {
                "image": target.name,
                "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
                "split": "calibration" if i < len(sources) - a.holdout else "holdout",
                "captured_at_unix_us": header["captured_at_unix_us"],
            }
        )
    # Relative to the model directory containing this calibration directory.
    for split, filename in [("calibration", "dataset.txt"), ("holdout", "holdout.txt")]:
        (a.output / filename).write_text(
            "".join(
                f"{a.output.name}/{r['image']}\n" for r in rows if r["split"] == split
            )
        )
    (a.output / "manifest.json").write_text(
        json.dumps(
            {
                "status": "single_scene_smoke_only",
                "input_rgb_hw": [384, 640],
                "normalization": "RGB / 255, once in ACUITY inputmeta",
                "frames": rows,
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
