#!/usr/bin/env python3
"""Compare board NPU holdouts with FP32; these are agreement metrics, not ground-truth mAP."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import onnxruntime as ort
from raw_client import validate
from resident import decode_split, unpack_frame
from yoloe_infer import decode, letterbox


def agreement(reference, masks, candidate, other_masks):
    pairs = []
    for i, a in enumerate(reference):
        box = np.asarray(a["bbox_xyxy"])
        for j, b in enumerate(candidate):
            if a["class_id"] != b["class_id"]:
                continue
            other = np.asarray(b["bbox_xyxy"])
            intersection = np.maximum(
                0, np.minimum(box[2:], other[2:]) - np.maximum(box[:2], other[:2])
            ).prod()
            union = (
                (box[2:] - box[:2]).prod()
                + (other[2:] - other[:2]).prod()
                - intersection
            )
            iou = float(intersection / max(union, 1e-9))
            if iou >= 0.5:
                pairs.append((iou, i, j))
    used_ref, used_candidate, matches = set(), set(), []
    for iou, i, j in sorted(pairs, reverse=True):
        if i in used_ref or j in used_candidate:
            continue
        used_ref.add(i)
        used_candidate.add(j)
        a, b = reference[i], candidate[j]
        union = (masks[i] | other_masks[j]).sum()
        matches.append(
            {
                "label": a["label"],
                "reference_index": i,
                "candidate_index": j,
                "reference_confidence": a["confidence"],
                "candidate_confidence": b["confidence"],
                "box_iou": iou,
                "mask_iou": float((masks[i] & other_masks[j]).sum() / max(union, 1)),
                "confidence_abs_delta": abs(a["confidence"] - b["confidence"]),
            }
        )
    return {
        "reference_count": len(reference),
        "candidate_count": len(candidate),
        "matches": matches,
        "missed_reference": [
            reference[i] for i in range(len(reference)) if i not in used_ref
        ],
        "extra_candidate": [
            candidate[i] for i in range(len(candidate)) if i not in used_candidate
        ],
        "high_confidence_reference_count": sum(
            d["confidence"] >= 0.5 for d in reference
        ),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--float-manifest", type=Path, required=True)
    p.add_argument("--nbg-manifest", type=Path, required=True)
    p.add_argument("--records", type=Path, required=True)
    p.add_argument("--npu-results", type=Path, required=True)
    p.add_argument("--calibration-manifest", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    baseline = json.loads(a.float_manifest.read_text())
    nbg = json.loads(a.nbg_manifest.read_text())
    model = a.float_manifest.parent / baseline["onnx"]
    if hashlib.sha256(model.read_bytes()).hexdigest() != baseline["onnx_sha256"]:
        raise ValueError("float hash mismatch")
    if (
        nbg["source_onnx_sha256"] != baseline["onnx_sha256"]
        or nbg["classes"] != baseline["classes"]
    ):
        raise ValueError("different source model or class order")
    run = json.loads((a.npu_results / "run.json").read_text())
    if not run["frames"]:
        raise ValueError("no NPU records to compare")
    if run["model_sha256"] != nbg["nbg_sha256"]:
        raise ValueError("NPU results belong to another model")
    calibration = json.loads(a.calibration_manifest.read_text())
    held_out = {
        Path(f["image"]).stem for f in calibration["frames"] if f["split"] == "holdout"
    }
    calibrated = {
        Path(f["image"]).stem
        for f in calibration["frames"]
        if f["split"] == "calibration"
    }
    options = ort.SessionOptions()
    options.intra_op_num_threads = 2
    session = ort.InferenceSession(
        str(model), options, providers=["CPUExecutionProvider"]
    )
    reports = []
    for row in run["frames"]:
        path = a.records / row["frame"]
        if path.stem not in held_out or path.stem in calibrated:
            raise ValueError(f"{path.name} is not a disjoint holdout")
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != row["raw_sha256"]:
            raise ValueError("NPU/CPU frame mismatch")
        header = json.loads(path.with_suffix(".json").read_text())
        validate(header)
        x, t = letterbox(unpack_frame(header, raw))
        ref, rm = decode(*session.run(None, {"images": x}), baseline["classes"], t)
        with np.load(a.npu_results / (path.stem + ".npz"), allow_pickle=False) as f:
            got, gm = decode_split(
                [f[f"output{i}"] for i in range(4)], baseline["classes"], t, 0.25
            )
        reports.append(
            {
                "frame": row["frame"],
                "npu_io_ms": row["npu_io_ms"],
                **agreement(ref, rm, got, gm),
            }
        )
    matches = [m for f in reports for m in f["matches"]]
    total = sum(f["reference_count"] for f in reports)
    high = sum(f["high_confidence_reference_count"] for f in reports)
    summary = {
        "frames": len(reports),
        "reference_count": total,
        "candidate_count": sum(f["candidate_count"] for f in reports),
        "matched_iou50": len(matches),
        "reference_retention_iou50": len(matches) / max(total, 1),
        "high_confidence_retention_iou50": sum(
            m["reference_confidence"] >= 0.5 for m in matches
        )
        / max(high, 1),
        "mean_box_iou_matched": float(np.mean([m["box_iou"] for m in matches]))
        if matches
        else None,
        "mean_mask_iou_matched": float(np.mean([m["mask_iou"] for m in matches]))
        if matches
        else None,
        "median_npu_io_ms": float(np.median([r["npu_io_ms"] for r in reports])),
    }
    result = {
        "measurement": "FP32 agreement, not labelled accuracy or mAP",
        "coverage": calibration["status"],
        "nbg_sha256": nbg["nbg_sha256"],
        "float_sha256": baseline["onnx_sha256"],
        "confidence": 0.25,
        "nms_iou": 0.7,
        "summary": summary,
        "frames": reports,
    }
    a.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
