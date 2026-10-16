# Orange Pi Zero 3W perception bring-up — 2026-10-15

Board: A733, IMX219 with the user's nominal 200° lens, kernel
`6.6.98-sun60iw2-microduck-xbox2`. No rotation, calibration, or undistortion.
The ISP setup is recorded in [the camera bring-up](orangepi-zero3w-camera.md).
Service ownership remains as described in [architecture](../design/architecture.md);
[the media design](../design/remote-webrtc.md) owns the shared raw-frame mechanism.

## Shared capture

The existing camera serves WebRTC video, snapshots, `media.stream`, and the optional duck
detector. These already share mediad's capture. An additional perception process must use
`media.frame` on `/run/mediad/media.sock`, not open `/dev/video0` or decode preview JPEGs.

`media.camera_backend = "allwinner"` selects the vendor NV12 ISP source and its AE/AWB.
It disables the Rockchip exposure policy, defaults to rotation zero, and does not publish the
unrelated Radxa lens intrinsics. Requested raw frames are packed into the existing UYVY wire
format using the actual plane strides. The native encoder branch retains NV12.
An advisory `/run/lock/microduck-imx219.lock` excludes the standalone capture tools.

`mediad-capture` is a commissioning mode using the same camera adapter, demand-driven frame
rendezvous, and IPC server. Run it **instead of** full mediad. This board lacks `webrtcsink`;
the full WebRTC/encoder path has not been validated on A733.

```sh
cargo build -p mediad --bin mediad-capture
sudo sh scripts/orangepi-zero3w-camera/run-mediad target/debug/mediad-capture \
  --config scripts/orangepi-zero3w-camera/camera.toml
```

The supplied commissioning config selects `360p30`: the sensor remains in its 1920×1080 mode
and the ISP outputs 640×360. This preserves the tested field of view and skips CPU resizing.
The model receives RGB 640×384 with 12 rows of value 114 above and below, normalized by 255.
Both 360p30 and 1080p30 were tested; other Allwinner quality presets currently fail explicitly.
This reduces a packed raw frame from 4,147,200 to 460,800 bytes (89%), but does not reduce sensor
readout cost. The sensor still runs at 30 fps; client demand determines raw-copy frequency.

Board checks: two concurrent clients received the same capture timestamp; a second camera lock
was refused. Thirty 640×360 raw requests at 10 Hz yielded 9.93 Hz, median fetch 56 ms, maximum
67 ms, using the unoptimized development binary. Those are capture/IPC timings, not NPU timings.

## Float reference and samples

`scripts/orangepi-zero3w-camera/perception/` freezes the 52-class vocabulary from
`librezoo/libre_semantic_world@11bbd72` into YOLOE-11s-seg. It retains `person`; this vocabulary
does **not** yet provide separate head, hand, or foot recognition. Model weights, room images,
calibration samples, and generated model binaries belong outside Git.

Use Python 3.12 and `requirements-export.txt` on the development machine. Export with
`export_yoloe.py --model /path/yoloe-11s-seg.pt --output /outside/repo/float`.
The static ONNX input is `[1,3,384,640]`, outputs `[1,88,5040]` and `[1,32,96,160]`;
NMS and mask assembly remain on CPU. The manifest pins hashes, class order and preprocessing.

`record_frames.py --output /outside/repo/records --count 30 --hz 1` runs on the board using only
the Python standard library. Transfer the raw files and headers without JPEG encoding, then run:

```sh
python yoloe_infer.py --manifest /outside/repo/float/manifest.json \
  --raw /outside/repo/records/frame-0029.uyvy --output /outside/repo/result
python prepare_calibration.py --records /outside/repo/records \
  --output /outside/repo/calibration --holdout 5
python -m unittest discover -s scripts/orangepi-zero3w-camera/perception
```

The 640×360 float reference identified chair, desk, door, cup, monitor and bottle; there are also
false positives. Mac CPU inference was approximately 178 ms, not a board throughput claim.
On two recorded frames the custom postprocessor matched pinned Ultralytics CPU class IDs and
boxes (maximum coordinate error <0.0001 px), with identical binary masks. The CPU path rounds
prototype-space crop edges; retaining fractional edges had caused small-object mask differences.

The initial 30 samples come from one scene. Twenty-five calibration frames and five held-out
frames are disjoint files, **not** independent scene coverage or an accuracy benchmark. Broader
rooms, lighting, people, and small objects are needed before accepting deployment quality.

## A733 compiler

The vendor demo ran ten times on VIPLite `2.0.3.2-AW-2024-08-30`, CID `0x1000003b`.
The kernel module's `1.13` string is not the userspace ABI version. Demo timing is not YOLOE timing.

The x86 Linux build uses the official Allwinner `ubuntu-npu:v2.0.10.2` image, ACUITY `6.30.22`,
Vivante IDE `5.11.0`, and `ZIFENG278/ai-sdk@fc90006d0f6569da2f6726c2d8395877686f5aca`.
Setup follows [Radxa's A733 compiler guide](https://docs.radxa.com/cubie/a7a/app-dev/npu-dev/cubie-acuity-env).
The downloaded inner `ubuntu-npu_v2.0.10.2.tar.zip` MD5 matched the vendor's
`170bb054e5fc78cb70d91dd6a2fc9397`. Keep this SDK outside Git too.

Before quantizing, run `split_yoloe_outputs.py` on the float manifest. The four outputs are
decoded boxes, class probabilities, mask coefficients and prototypes. They reproduce the
original float outputs exactly. Without splitting, ACUITY shares a scale of approximately 2.52
between coordinates up to 640 and probabilities below 1, destroying the confidence values.
The split version keeps their quantization ranges independent.

Inside the container, set `ACUITY_PATH=/root/acuity-toolkit-whl-6.30.22/bin`,
`VIV_SDK=/root/Vivante_IDE/VivanteIDE5.11.0/cmdtools`, and source `scripts/pegasus_setup.sh v3`.
Use `pegasus_import.sh MODEL`, then set the generated inputmeta RGB preprocessing scale to
`[1/255,1/255,1/255]`, with zero mean and channel reversal enabled for the image loader.
Place the already letterboxed lossless PNGs and `dataset.txt` beside the model; do not normalize
the PNG byte values themselves. Run `pegasus_quantize.sh MODEL pcq` followed by
`pegasus_export_ovx.sh MODEL pcq`. Target is `VIP9000NANODI_PLUS_PID0X1000003B`.

The split INT8 NBG ran ten times successfully on the board. VIPLite reported approximately
24.1 ms hardware execution and 24.3–25.5 ms wall time per invocation, excluding capture,
preprocessing and CPU postprocessing. Its queried input is INT8, planar RGB 640×384,
scale `0.003921568859368563`, zero point `-128`; do not pass unsigned image bytes directly.

One held-out frame (`frame-0029`) produced 10 float and 12 NPU detections at confidence 0.25.
Door, chair, desk and cup remained recognizable; compared with float, their mask IoUs were
approximately 0.989, 0.942, 0.856 and 0.799. A narrow bottle degraded to about 0.507 and there
were extra overlapping detections. These compare two model outputs, not human annotations.
The current decoded-box INT8 scale is approximately 2.50 pixels, which warrants special
attention for small objects. This build proves board execution, not acceptance of accuracy.

## Resident integration and validation

`vip_bridge.c` is a small checked ABI against the board's installed `libNBGlinker` and
`libVIPhal`. Vendor headers are supplied separately; do not copy them into this repository.
The bridge queries and bounds-checks tensor shapes, sizes, types, scales and zero points.
`npu_runtime.py` checks those against a SHA256-pinned NBG manifest and keeps the network and
buffers resident on one thread. It flushes/invalidates caches through VIPLite for each run.
On the same recorded input, all four outputs matched the vendor `vpm_run` dump exactly.

Install the runtime dependencies on the board and build the bridge:

```sh
sudo apt-get install --no-install-recommends python3-numpy python3-opencv
VIPLITE_INCLUDE=/path/to/sdk/v2.0/inc sh \
  scripts/orangepi-zero3w-camera/perception/build_vip_bridge.sh /outside/repo/libmicroduck-vip.so
```

`perception/nbg-manifest.example.json` records the exact tested build's model hashes, class
order and queried quantization metadata. Copy it beside `yoloe52split-pcq.nb` as
`nbg-manifest.json` for that build. A newly compiled model needs its own NBG hash and tensor
metadata from VIPLite; the runtime rejects a mismatched example instead of guessing scales.

Run capture independently of the SSH session; a foreground SSH capture exits when the laptop
disconnects. The development board uses a transient `microduck-capture-dev` systemd service
running the commissioning command above. `microduck-perception-dev` runs:

```sh
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 python3 \
  scripts/orangepi-zero3w-camera/perception/resident.py \
  --manifest /outside/repo/nbg-manifest.json \
  --library /outside/repo/libmicroduck-vip.so --hz 5
```

`resident.py` consumes only `media.frame`; `/dev/video0` remains owned by the capture service.
Every inference publishes one atomic `/run/microduck-perception/latest.npz` containing raw RGB,
bit-packed binary masks, boxes, capture time, model hash and processing timings. It retains only
the latest result, never a growing frame queue. Empty detections replace previous detections.
Slow work skips scheduling slots rather than producing catch-up bursts. Camera disconnection
keeps the model loaded, backs off to five seconds, and rate-limits diagnostics; protocol/model
errors still fail explicitly. `--count` provides a bounded run for commissioning.

The laptop power-loss test exposed an earlier camera retry policy that reloaded the model after
ten failed frame fetches. That policy was removed. The regression test fails twelve consecutive
fetches and verifies recovery with one model initialization. Capture is now also independent of
SSH, which removes the original cause. These development units are transient: restarting the
board still requires starting them again; this is not a production boot-service installation.

A local preview used for the measurements below consumes the atomic result. Its implementation
and installation are not part of this change.

For repeatable model comparisons, `evaluate_npu_records.py` runs saved raw records on the board
with a single resident network, saving four tensors and a raw-frame hash for each record.
`compare_quantization.py` runs the FP32 model on the same hashed frames, checks that each is in
the held-out split, and performs one-to-one class-aware box matching at IoU ≥ 0.5. Both use
confidence 0.25 and class-aware NMS IoU 0.7.

The five held-out frames produced 48 FP32 and 49 INT8 detections: 39 matched (81.25% reference
retention). All FP32 detections with confidence ≥0.5 were retained. Matched-box mean IoU was
0.8545; matched-mask mean IoU was 0.9087. The median NPU call, including input quantization,
copies and output dequantization, took 34.47 ms. The report is an FP32 agreement measurement,
**not** human-labelled accuracy or mAP; the five adjacent frames do not establish broad coverage.

After building the capture crate with `--config 'profile.dev.package.mediad.opt-level=2'`,
a 30-second live check received 150 unique results at 4.994 Hz. Median capture/IPC was 15.09 ms,
NPU including IO was 35.98 ms, and total processing before publication was 92.78 ms. Maximum
result age seen by the laptop was 197 ms; temperature ranged from 54.7 to 57.7°C. This measures
the tested scene, not a guarantee for arbitrary object counts. All three services had zero
restarts during this check; `/dev/video0` had exactly one owner.

To launch the two development processes from a board checkout without tying them to SSH:

```sh
sudo systemd-run --unit=microduck-capture-dev --property=Restart=on-failure \
  --property=KillSignal=SIGINT /bin/sh "$PWD/scripts/orangepi-zero3w-camera/run-mediad" \
  "$PWD/target/debug/mediad-capture" \
  --config "$PWD/scripts/orangepi-zero3w-camera/camera.toml"
sudo systemd-run --unit=microduck-perception-dev --property=Restart=on-failure \
  --setenv=OPENBLAS_NUM_THREADS=1 --setenv=OMP_NUM_THREADS=1 /usr/bin/python3 \
  "$PWD/scripts/orangepi-zero3w-camera/perception/resident.py" \
  --manifest /outside/repo/nbg-manifest.json --library /outside/repo/libmicroduck-vip.so --hz 5
```

Use `systemctl restart UNIT` for a running unit. Stopping a transient unit may unload it;
if it disappears, rerun its `systemd-run` command. Do not start commissioning capture alongside
the full mediad service. Production installation and boot integration remain separate work.

Remaining accuracy work is broader calibration and held-out scene coverage, small-object
quantization improvements, and separate head/hand/foot recognition. No inference result drives
locomotion in this bring-up.

## Code validation — 2026-10-16

Host Rust tests passed for `mediad` (88) and `robotd-params` (104). Native Linux `mediad`
tests passed (125); the board's `robotd-params` run had one unrelated roller-preset assertion
failure because the installed policy resolves `ground_pick_period` to 5 rather than the test's
expected 3. The installed policy was not changed. Both media binaries built natively on A733,
and the C bridge built with `-Wall -Wextra -Werror`.

The Python suite covers raw-frame truncation, camera reconnect without reloading the NPU,
INT8 clipping, atomic result publication, class-aware NMS, and box/mask coordinate mapping.
