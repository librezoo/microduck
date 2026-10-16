import json
import socket
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
import resident
from npu_runtime import quantize
from raw_client import fetch, validate
from resident import publish
from yoloe_infer import decode, letterbox, nms


class GeometryTests(unittest.TestCase):
    def test_360p_is_only_padded_without_cpu_resize(self):
        # Capture already matches the useful pixels of the 640x384 model input.
        image = np.full((360, 640, 3), 200, np.uint8)
        with patch(
            "yoloe_infer.cv2.resize", side_effect=AssertionError("unexpected resize")
        ):
            tensor, transform = letterbox(image)
        self.assertEqual(transform, (1, 0, 12, 640, 360))
        np.testing.assert_allclose(tensor[:, :, 12:372], 200 / 255)
        np.testing.assert_allclose(tensor[:, :, :12], 114 / 255)
        np.testing.assert_allclose(tensor[:, :, 372:], 114 / 255)

    def test_1080p_keeps_full_distorted_view_without_rotation(self):
        image = np.full((1080, 1920, 3), 200, np.uint8)
        tensor, transform = letterbox(image)
        self.assertEqual(tensor.shape, (1, 3, 384, 640))
        self.assertEqual(transform, (1 / 3, 0, 12, 1920, 1080))
        np.testing.assert_allclose(tensor[:, :, :12], 114 / 255)
        np.testing.assert_allclose(tensor[:, :, 12:372], 200 / 255)

    def test_nms_is_class_aware_and_deterministic(self):
        boxes = np.array([[0, 0, 10, 10], [0, 0, 10, 10], [0, 0, 10, 10]], np.float32)
        np.testing.assert_array_equal(
            nms(boxes, np.array([0.9, 0.8, 0.7]), np.array([0, 0, 1])), [0, 2]
        )

    def test_boxes_and_masks_undo_letterbox_and_keep_person(self):
        pred = np.zeros((1, 7, 1), np.float32)  # 4 box + 2 classes + 1 coefficient
        pred[0, :, 0] = [320, 192, 320, 180, 0.1, 0.9, 1]
        proto = np.ones((1, 1, 96, 160), np.float32)
        detections, masks = decode(
            pred, proto, ["cup", "person"], (1 / 3, 0, 12, 1920, 1080)
        )
        self.assertEqual(detections[0]["label"], "person")
        np.testing.assert_allclose(detections[0]["bbox_xyxy"], [480, 270, 1440, 810])
        self.assertEqual(masks.shape, (1, 1080, 1920))
        self.assertTrue(masks[0, 540, 960])
        self.assertFalse(masks[0, 0, 0])
        empty, masks = decode(
            pred, proto, ["cup", "person"], (1 / 3, 0, 12, 1920, 1080), conf=0.95
        )
        self.assertEqual(empty, [])
        self.assertEqual(masks.shape, (0, 1080, 1920))


class RuntimeTests(unittest.TestCase):
    def test_camera_disconnect_keeps_the_same_loaded_model(self):
        # A disconnected SSH capture used to cause thousands of systemd/NPU reloads.
        with tempfile.TemporaryDirectory() as d:
            output = Path(d) / "latest.npz"
            header = dict(
                width=2,
                height=2,
                bytes=8,
                format="UYVY",
                rotate=0,
                captured_at_unix_us=1,
            )
            net = MagicMock()
            net.manifest = {"classes": ["person"], "nbg_sha256": "test"}
            network = MagicMock()
            network.return_value.__enter__.return_value = net
            errors = [ConnectionRefusedError("camera offline") for _ in range(12)]
            with (
                patch(
                    "sys.argv",
                    [
                        "resident",
                        "--manifest",
                        "unused",
                        "--library",
                        "unused",
                        "--output",
                        str(output),
                        "--count",
                        "1",
                    ],
                ),
                patch("resident.Network", network),
                patch(
                    "resident.fetch",
                    side_effect=errors + [(header, bytes([128, 128] * 4))],
                ) as fetcher,
                patch(
                    "resident.decode_split",
                    return_value=([], np.zeros((0, 2, 2), bool)),
                ),
                patch("resident.time.sleep"),
                patch("resident.signal.signal"),
                patch("builtins.print"),
            ):
                resident.main()
            self.assertEqual(network.call_count, 1)
            self.assertEqual(net.run.call_count, 1)
            self.assertEqual(fetcher.call_count, 13)
            self.assertTrue(output.exists())

    def test_int8_input_does_not_wrap_unsigned_pixels(self):
        pixels = np.array([-1, 0, 128 / 255, 1, 2], np.float32)
        np.testing.assert_array_equal(
            quantize(pixels, np.int8, 1 / 255, -128), [-128, -128, 0, 127, 127]
        )

    def test_atomic_result_keeps_pixels_boxes_and_masks_together(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "latest.npz"
            rgb = np.zeros((4, 10, 3), np.uint8)
            masks = np.zeros((1, 4, 10), bool)
            masks[0, 1, 9] = True  # width not divisible by eight exercises padding bits
            for frame_id in [1, 2]:
                publish(path, rgb + frame_id, masks, {"frame_id": frame_id})
                with np.load(path, allow_pickle=False) as bundle:
                    self.assertEqual(
                        json.loads(bundle["metadata"].tobytes())["frame_id"], frame_id
                    )
                    self.assertTrue(np.all(bundle["rgb"] == frame_id))
                    np.testing.assert_array_equal(
                        np.unpackbits(bundle["masks_bits"], axis=2, count=10), masks
                    )
            self.assertEqual(list(Path(d).glob("*.tmp")), [])


class ProtocolTests(unittest.TestCase):
    def header(self):
        return dict(
            width=2, height=2, format="UYVY", bytes=8, rotate=0, captured_at_unix_us=1
        )

    def test_refuses_rotation_or_oversized_payload_before_read(self):
        for extra in [
            dict(rotate=90),
            dict(bytes=100),
            dict(width=3),
            dict(width=True),
        ]:
            with self.assertRaises(ValueError):
                validate(self.header() | extra)

    def serve(self, payload, broken_header=None):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        path = str(Path(temp.name) / "camera.sock")
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen()

        def run():
            with server, server.accept()[0] as conn, conn.makefile("rb") as reader:
                for ident, result in [(1, {"api_version": 42}), (2, self.header())]:
                    request = json.loads(reader.readline())
                    self.assertEqual(request["id"], ident)
                    if ident == 2 and broken_header is not None:
                        conn.sendall(broken_header)
                        return
                    conn.sendall(
                        json.dumps(
                            dict(jsonrpc="2.0", id=ident, result=result)
                        ).encode()
                        + b"\n"
                    )
                # Split binary chunks to exercise reads across arbitrary stream boundaries.
                conn.sendall(payload[:3])
                conn.sendall(payload[3:])

        worker = threading.Thread(target=run)
        worker.start()
        self.addCleanup(worker.join, 3)
        return path

    def test_binary_payload_may_contain_newlines_and_is_not_json(self):
        payload = b"\x00\n\xff\r1234"
        header, data = fetch(self.serve(payload))
        self.assertEqual(data, payload)
        self.assertEqual(header, self.header())

    def test_truncated_payload_does_not_become_a_partial_image(self):
        with self.assertRaises(EOFError):
            fetch(self.serve(b"123"))

    def test_disconnect_during_header_is_a_recoverable_capture_error(self):
        # The resident retries EOF without discarding its loaded NPU network.
        for prefix in [b"", b'{"jsonrpc":']:
            with self.subTest(prefix=prefix), self.assertRaises(EOFError):
                fetch(self.serve(b"", broken_header=prefix))


if __name__ == "__main__":
    unittest.main()
