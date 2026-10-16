#!/usr/bin/env python3
"""Freeze YOLOE prompts and export a static float baseline for A733 conversion.

Adapted from libre_semantic_world's GMSL experiment (11bbd72), retaining its
class-order/reload checks and artifact hashes. NMS and mask assembly stay outside ONNX.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path


def sha256(path):
    with Path(path).open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", type=Path, required=True)
    p.add_argument(
        "--classes", type=Path, default=Path(__file__).with_name("household-52.json")
    )
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    model_path, classes_path, output = (
        a.model.resolve(),
        a.classes.resolve(),
        a.output.resolve(),
    )
    classes = json.loads(classes_path.read_text())["classes"]
    if (
        not classes
        or len(classes) != len(set(classes))
        or any(not isinstance(c, str) or not c.strip() for c in classes)
    ):
        raise ValueError("classes must be unique non-empty strings")
    output.mkdir(parents=True, exist_ok=True)
    checkpoint = output / "yoloe-11s-household.pt"
    manifest_path = output / "manifest.json"
    if manifest_path.exists():
        raise FileExistsError("use a new output directory for another vocabulary/build")
    import torch
    import ultralytics
    from ultralytics import YOLOE

    # MobileCLIP resolves its assets relative to cwd; keep downloads outside the repository.
    os.chdir(output)
    if not checkpoint.exists():
        model = YOLOE(str(model_path))
        model.set_classes(classes)
        model.save(checkpoint)
    model = YOLOE(str(checkpoint))
    if [model.names[i] for i in range(len(model.names))] != classes:
        raise ValueError(
            "checkpoint's frozen vocabulary differs from requested class order"
        )
    exported = Path(
        model.export(
            format="onnx",
            imgsz=(384, 640),
            batch=1,
            dynamic=False,
            half=False,
            simplify=False,
            nms=False,
            opset=13,
            device="cpu",
        )
    )
    import onnx

    graph = onnx.load(exported)
    onnx.checker.check_model(graph)

    def shape(value):
        return [d.dim_value for d in value.type.tensor_type.shape.dim]

    manifest = {
        "schema": 1,
        "backend": "onnx-float-baseline",
        "classes": classes,
        "class_count": len(classes),
        "classes_sha256": sha256(classes_path),
        "base_sha256": sha256(model_path),
        "checkpoint_sha256": sha256(checkpoint),
        "onnx": exported.name,
        "onnx_sha256": sha256(exported),
        "ultralytics": ultralytics.__version__,
        "torch": torch.__version__,
        "input": {
            "shape": [1, 3, 384, 640],
            "layout": "NCHW",
            "color": "RGB",
            "scale": 1 / 255,
            "letterbox_value": 114,
        },
        "outputs": [{"name": v.name, "shape": shape(v)} for v in graph.graph.output],
        "camera": {
            "lens_nominal_degrees": 200,
            "rotate": 0,
            "undistort": False,
            "capture_hw": [360, 640],
            "sensor_mode_hw": [1080, 1920],
            "calibrated": False,
        },
        "source_reference": "librezoo/libre_semantic_world@11bbd72",
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
