#!/usr/bin/env python3
"""Separate boxes, scores and mask coefficients before A733 INT8 conversion.

The decoded YOLOE concat mixes coordinates up to 640 with probabilities up to 1.
ACUITY shares one activation scale across that concat, which erases the scores.
Keeping four outputs preserves independent quantization without changing the float model.
"""

import argparse
import json
from pathlib import Path

import onnx
from export_yoloe import sha256


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    manifest = json.loads(a.manifest.read_text())
    source = a.manifest.parent / manifest["onnx"]
    if sha256(source) != manifest["onnx_sha256"]:
        raise ValueError("float model/manifest hash mismatch")
    graph = onnx.load(source)
    outputs = [v.name for v in graph.graph.output]
    concat = next(n for n in graph.graph.node if outputs[0] in n.output)
    if concat.op_type != "Concat" or len(concat.input) != 3 or len(outputs) != 2:
        raise ValueError(
            "expected the pinned YOLOE segmentation concat + prototype export"
        )
    names = list(concat.input) + [outputs[1]]
    a.output.mkdir(parents=True, exist_ok=False)
    target = a.output / "yoloe52split.onnx"
    onnx.utils.extract_model(
        str(source), str(target), [graph.graph.input[0].name], names, check_model=True
    )
    split = onnx.load(target)
    # ONNX shape inference loses the constant batch through the DFL reshape. The input
    # is explicitly batch one; restore that bound rather than exporting a dynamic graph.
    if [d.dim_value for d in split.graph.input[0].type.tensor_type.shape.dim] != [
        1,
        3,
        384,
        640,
    ]:
        raise ValueError("expected a static batch-one input")
    for value in split.graph.output:
        if value.type.tensor_type.shape.dim[0].dim_value == 0:
            value.type.tensor_type.shape.dim[0].dim_value = 1
    shapes = [
        [d.dim_value for d in v.type.tensor_type.shape.dim] for v in split.graph.output
    ]
    expected = [
        [1, 4, 5040],
        [1, len(manifest["classes"]), 5040],
        [1, 32, 5040],
        [1, 32, 96, 160],
    ]
    if shapes != expected:
        raise ValueError(f"unexpected split output geometry: {shapes}")
    onnx.checker.check_model(split)
    onnx.save(split, target)
    manifest.update(
        {
            "backend": "onnx-float-split-for-acuity",
            "source_onnx_sha256": manifest["onnx_sha256"],
            "onnx": target.name,
            "onnx_sha256": sha256(target),
            "outputs": [
                {"name": name, "shape": shape, "role": role}
                for name, shape, role in zip(
                    names,
                    shapes,
                    ["boxes_xywh", "class_scores", "mask_coefficients", "prototypes"],
                )
            ],
        }
    )
    (a.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest["outputs"], indent=2))


if __name__ == "__main__":
    main()
