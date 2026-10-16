#!/usr/bin/env python3
"""Run saved raw holdouts through one resident NPU instance; keep tensors for float comparison."""

import argparse
import hashlib
import json
import time
from pathlib import Path

import cv2
import numpy as np
from npu_runtime import Network
from raw_client import validate
from resident import unpack_frame
from yoloe_infer import letterbox


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--library", type=Path, required=True)
    p.add_argument("--records", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    cv2.setNumThreads(1)
    rows = []
    with Network(a.manifest, a.library) as network:
        for path in sorted(a.records.glob("*.uyvy")):
            header = json.loads(path.with_suffix(".json").read_text())
            validate(header)
            raw = path.read_bytes()
            tensor, _ = letterbox(unpack_frame(header, raw))
            start = time.monotonic()
            outputs = network.run(tensor)
            row = {
                "frame": path.name,
                "raw_sha256": hashlib.sha256(raw).hexdigest(),
                "npu_io_ms": (time.monotonic() - start) * 1000,
            }
            np.savez(
                a.output / (path.stem + ".npz"),
                **{f"output{i}": v for i, v in enumerate(outputs)},
            )
            rows.append(row)
            print(json.dumps(row), flush=True)
        if not rows:
            raise ValueError("no raw records")
        (a.output / "run.json").write_text(
            json.dumps(
                {"model_sha256": network.manifest["nbg_sha256"], "frames": rows},
                indent=2,
            )
            + "\n"
        )


if __name__ == "__main__":
    main()
