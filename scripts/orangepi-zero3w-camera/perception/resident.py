#!/usr/bin/env python3
"""Paced NPU perception from mediad raw frames; model and buffers stay resident."""

import argparse
import fcntl
import json
import os
import signal
import time
from pathlib import Path

import cv2
import numpy as np
from npu_runtime import Network
from raw_client import fetch
from yoloe_infer import decode, letterbox


def unpack_frame(header, data):
    if len(data) != header["bytes"]:
        raise ValueError("incomplete raw frame")
    packed = np.frombuffer(data, np.uint8).reshape(header["height"], header["width"], 2)
    return cv2.cvtColor(packed, cv2.COLOR_YUV2RGB_UYVY)


def decode_split(outputs, classes, transform, confidence):
    if len(outputs) != 4:
        raise ValueError("expected boxes, scores, coefficients, prototypes")
    return decode(
        np.concatenate(outputs[:3], axis=1),
        outputs[3],
        classes,
        transform,
        conf=confidence,
    )


def publish(path, rgb, masks, result):
    # One rename publishes pixels, masks and boxes from the same frame. Slow readers get
    # the latest complete result instead of building a latency-inducing frame queue.
    temporary = path.with_suffix(".tmp")
    try:
        with temporary.open("wb") as f:
            np.savez(
                f,
                rgb=rgb,
                masks_bits=np.packbits(masks, axis=2),
                metadata=np.frombuffer(json.dumps(result).encode(), np.uint8),
            )
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--library", type=Path, required=True)
    p.add_argument("--socket", default="/run/mediad/media.sock")
    p.add_argument(
        "--output", type=Path, default=Path("/run/microduck-perception/latest.npz")
    )
    p.add_argument("--hz", type=float, default=5)
    p.add_argument("--confidence", type=float, default=0.25)
    p.add_argument("--count", type=int, default=0, help="zero runs until stopped")
    a = p.parse_args()
    if not 0 < a.hz <= 10 or not 0 < a.confidence < 1 or a.count < 0:
        p.error("hz must be (0,10], confidence (0,1), count >= 0")
    cv2.setNumThreads(1)
    a.output.parent.mkdir(parents=True, exist_ok=True)
    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    with a.output.with_suffix(".lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with Network(a.manifest, a.library) as network:
            frame_id, failures, deadline = 0, 0, time.monotonic()
            last_error_log = 0.0
            while running and (not a.count or frame_id < a.count):
                time.sleep(max(0, deadline - time.monotonic()))
                if not running:
                    break
                start = time.monotonic()
                try:
                    header, data = fetch(a.socket)
                except (OSError, EOFError) as error:
                    # The image stays stamped with its old time, so consumers can mark it stale.
                    # Camera disconnects are recoverable. Keep the model loaded and back off,
                    # rather than making systemd reload it repeatedly while the source is absent.
                    failures += 1
                    now = time.monotonic()
                    if failures == 1 or now - last_error_log >= 30:
                        print(
                            json.dumps(
                                {
                                    "capture_error": str(error),
                                    "consecutive_failures": failures,
                                }
                            ),
                            flush=True,
                        )
                        last_error_log = now
                    deadline = now + min(5, 1 + failures // 3)
                    continue
                failures = 0
                fetched = time.monotonic()
                rgb = unpack_frame(header, data)
                tensor, transform = letterbox(rgb)
                prepared = time.monotonic()
                outputs = network.run(tensor)
                inferred = time.monotonic()
                detections, masks = decode_split(
                    outputs, network.manifest["classes"], transform, a.confidence
                )
                decoded = time.monotonic()
                frame_id += 1
                result = {
                    "frame_id": frame_id,
                    "frame": header,
                    "target_hz": a.hz,
                    "backend": "viplite-npu",
                    "model_sha256": network.manifest["nbg_sha256"],
                    "confidence_threshold": a.confidence,
                    "rotate": 0,
                    "undistort": False,
                    "published_at_unix_ms": time.time_ns() // 1_000_000,
                    "timing_ms": {
                        "fetch": (fetched - start) * 1000,
                        "preprocess": (prepared - fetched) * 1000,
                        "npu_io": (inferred - prepared) * 1000,
                        "postprocess": (decoded - inferred) * 1000,
                        "total_before_publish": (decoded - start) * 1000,
                    },
                    "detections": detections,
                }
                publish(a.output, rgb, masks, result)
                if frame_id == 1 or frame_id % 25 == 0:
                    print(
                        json.dumps(
                            {
                                "frame_id": frame_id,
                                "objects": len(detections),
                                "timing_ms": result["timing_ms"],
                                "cycle_ms": (time.monotonic() - start) * 1000,
                            }
                        ),
                        flush=True,
                    )
                deadline += 1 / a.hz
                # Missed slots are dropped; catch-up bursts waste resources and show old motion.
                now = time.monotonic()
                if deadline < now:
                    deadline += (int((now - deadline) * a.hz) + 1) / a.hz


if __name__ == "__main__":
    main()
