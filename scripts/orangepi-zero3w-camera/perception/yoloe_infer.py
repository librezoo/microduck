#!/usr/bin/env python3
"""YOLOE float reference from shared raw frames; no capture device or JPEG input."""

import argparse
import hashlib
import json
import time
from pathlib import Path

import cv2
import numpy as np
from raw_client import fetch, validate


def letterbox(rgb, size=(384, 640)):
    h, w = rgb.shape[:2]
    oh, ow = size
    scale = min(ow / w, oh / h)
    nw, nh = round(w * scale), round(h * scale)
    left, top = (ow - nw) // 2, (oh - nh) // 2
    canvas = np.full((oh, ow, 3), 114, np.uint8)
    canvas[top : top + nh, left : left + nw] = (
        rgb
        if (nw, nh) == (w, h)
        else cv2.resize(rgb, (nw, nh), interpolation=cv2.INTER_LINEAR)
    )
    tensor = (
        np.ascontiguousarray(canvas.transpose(2, 0, 1)[None], dtype=np.float32) / 255
    )
    return tensor, (scale, left, top, w, h)


def nms(boxes, scores, labels, threshold=0.7, max_det=30):
    order = np.argsort(-scores, kind="stable")[:3000]
    keep = []
    while len(order) and len(keep) < max_det:
        i = order[0]
        keep.append(i)
        other = order[1:]
        lo = np.maximum(boxes[i, :2], boxes[other, :2])
        hi = np.minimum(boxes[i, 2:], boxes[other, 2:])
        intersection = np.maximum(hi - lo, 0).prod(axis=1)
        area = (boxes[:, 2:] - boxes[:, :2]).clip(0).prod(axis=1)
        iou = intersection / np.maximum(area[i] + area[other] - intersection, 1e-9)
        order = other[(labels[other] != labels[i]) | (iou <= threshold)]
    return np.asarray(keep, dtype=np.int64)


def decode(
    prediction, prototypes, classes, transform, size=(384, 640), conf=0.25, iou=0.7
):
    # Static Ultralytics segmentation export: decoded xywh + class scores + mask coefficients.
    nc, nm = len(classes), prototypes.shape[1]
    if (
        prediction.ndim != 3
        or prediction.shape[0] != 1
        or prediction.shape[1] != 4 + nc + nm
    ):
        raise ValueError(f"unexpected YOLOE output: {prediction.shape}")
    rows = prediction[0].T
    labels = rows[:, 4 : 4 + nc].argmax(axis=1)
    scores = rows[np.arange(len(rows)), 4 + labels]
    selected = np.isfinite(rows).all(axis=1) & (scores >= conf)
    rows, labels, scores = rows[selected], labels[selected], scores[selected]
    boxes = np.concatenate(
        (rows[:, :2] - rows[:, 2:4] / 2, rows[:, :2] + rows[:, 2:4] / 2), axis=1
    )
    keep = nms(boxes, scores, labels, iou)
    rows, boxes, scores, labels = rows[keep], boxes[keep], scores[keep], labels[keep]
    scale, left, top, w, h = transform
    ph, pw = prototypes.shape[2:]
    logits = (rows[:, 4 + nc :] @ prototypes[0].reshape(nm, -1)).reshape(-1, ph, pw)
    oh, ow = size
    yy, xx = np.mgrid[:ph, :pw]
    # The pinned Ultralytics CPU path rounds crop edges for fewer than 50 masks.
    # We cap NMS at 30; fractional edges otherwise lose a full prototype pixel.
    masks, detections = [], []
    for box, score, label, logit in zip(boxes, scores, labels, logits):
        bx = np.rint(box * np.array([pw / ow, ph / oh, pw / ow, ph / oh], np.float32))
        crop = (xx >= bx[0]) & (xx < bx[2]) & (yy >= bx[1]) & (yy < bx[3])
        model_mask = (
            cv2.resize(logit * crop, (ow, oh), interpolation=cv2.INTER_LINEAR) > 0
        )
        # Masks are reported at source geometry, never including letterbox pixels.
        nh, nw = round(h * scale), round(w * scale)
        mask = cv2.resize(
            model_mask[top : top + nh, left : left + nw].astype(np.uint8),
            (w, h),
            interpolation=cv2.INTER_NEAREST,
        ).astype(bool)
        original = (box - [left, top, left, top]) / scale
        original = np.clip(original, [0, 0, 0, 0], [w, h, w, h])
        if original[2] <= original[0] or original[3] <= original[1] or not mask.any():
            continue
        detections.append(
            {
                "label": classes[int(label)],
                "class_id": int(label),
                "confidence": float(score),
                "bbox_xyxy": original.tolist(),
                "mask_index": len(masks),
                "mask_area": int(mask.sum()),
            }
        )
        masks.append(mask)
    return detections, np.stack(masks) if masks else np.zeros((0, h, w), dtype=bool)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--manifest", type=Path, required=True)
    p.add_argument("--socket", default="/run/mediad/media.sock")
    p.add_argument(
        "--raw", type=Path, help="saved UYVY frame, with same-stem .json header"
    )
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--confidence", type=float, default=0.25)
    p.add_argument("--threads", type=int, default=2)
    a = p.parse_args()
    manifest = json.loads(a.manifest.read_text())
    model = a.manifest.parent / manifest["onnx"]
    with model.open("rb") as f:
        if hashlib.file_digest(f, "sha256").hexdigest() != manifest["onnx_sha256"]:
            raise ValueError("model/manifest hash mismatch")
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.intra_op_num_threads = a.threads
    session = ort.InferenceSession(
        str(model), options, providers=["CPUExecutionProvider"]
    )
    if session.get_inputs()[0].shape != manifest["input"]["shape"]:
        raise ValueError("model/manifest shape mismatch")
    if a.raw:
        header, data = (
            json.loads(a.raw.with_suffix(".json").read_text()),
            a.raw.read_bytes(),
        )
        validate(header)
    else:
        header, data = fetch(a.socket)
    if len(data) != header["bytes"]:
        raise ValueError("truncated UYVY frame")
    start = time.monotonic()
    packed = np.frombuffer(data, np.uint8).reshape(header["height"], header["width"], 2)
    rgb = cv2.cvtColor(packed, cv2.COLOR_YUV2RGB_UYVY)
    tensor, transform = letterbox(rgb)
    pre_ms = (time.monotonic() - start) * 1000
    start = time.monotonic()
    prediction, prototypes = session.run(None, {session.get_inputs()[0].name: tensor})
    inference_ms = (time.monotonic() - start) * 1000
    start = time.monotonic()
    detections, masks = decode(
        prediction, prototypes, manifest["classes"], transform, conf=a.confidence
    )
    post_ms = (time.monotonic() - start) * 1000
    a.output.mkdir(parents=True, exist_ok=True)
    canvas = cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)
    for detection, mask in zip(detections, masks):
        color = np.array([60, 200, 255], np.uint8)
        canvas[mask] = (canvas[mask] * 0.6 + color * 0.4).astype(np.uint8)
        x1, y1, x2, y2 = map(round, detection["bbox_xyxy"])
        cv2.rectangle(canvas, (x1, y1), (x2, y2), (60, 200, 255), 2)
        cv2.putText(
            canvas,
            f"{detection['label']} {detection['confidence']:.2f}",
            (x1, max(y1 - 5, 20)),
            cv2.FONT_HERSHEY_SIMPLEX,
            0.7,
            (30, 30, 255),
            2,
        )
    cv2.imwrite(str(a.output / "overlay.png"), canvas)
    cv2.imwrite(str(a.output / "source.png"), cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR))
    np.savez_compressed(a.output / "masks.npz", masks=masks)
    result = {
        "frame": header,
        "backend": "onnx-float-cpu",
        "rotate": 0,
        "undistort": False,
        "model_sha256": manifest["onnx_sha256"],
        "preprocess_ms": pre_ms,
        "inference_ms": inference_ms,
        "postprocess_ms": post_ms,
        "detections": detections,
    }
    (a.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
