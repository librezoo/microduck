"""Resident, single-thread VIPLite network with queried and manifest-checked tensor metadata."""

import ctypes as C
import hashlib
import json
import threading
from pathlib import Path

import numpy as np


class Meta(C.Structure):
    _fields_ = [
        ("dims", C.c_uint32),
        ("sizes", C.c_uint32 * 6),
        ("format", C.c_uint32),
        ("quant", C.c_uint32),
        ("scale", C.c_float),
        ("zero", C.c_int32),
        ("bytes", C.c_uint32),
    ]


def quantize(tensor, dtype, scale, zero):
    if np.issubdtype(dtype, np.integer):
        bounds = np.iinfo(dtype)
        return np.ascontiguousarray(
            np.clip(np.rint(tensor / scale) + zero, bounds.min, bounds.max), dtype=dtype
        )
    return np.ascontiguousarray(tensor, dtype=dtype)


class Network:
    def __init__(self, manifest, library):
        self.handle = None
        self.owner = threading.get_ident()
        self.manifest = json.loads(Path(manifest).read_text())
        nbg = Path(manifest).parent / self.manifest["nbg"]
        with nbg.open("rb") as f:
            if (
                hashlib.file_digest(f, "sha256").hexdigest()
                != self.manifest["nbg_sha256"]
            ):
                raise ValueError("NBG/manifest SHA256 mismatch")
        self.lib = C.CDLL(str(Path(library).resolve()))
        self.lib.md_error.restype = C.c_char_p
        self.lib.md_open.argtypes, self.lib.md_open.restype = [C.c_char_p], C.c_void_p
        self.lib.md_close.argtypes, self.lib.md_close.restype = [C.c_void_p], None
        self.lib.md_info.argtypes = [C.c_void_p, C.c_int, C.POINTER(Meta)]
        self.lib.md_output_count.argtypes = [C.c_void_p]
        self.lib.md_run.argtypes = [
            C.c_void_p,
            C.c_void_p,
            C.c_uint32,
            C.POINTER(C.c_void_p),
            C.POINTER(C.c_uint32),
        ]
        self.handle = self.lib.md_open(str(nbg).encode())
        if not self.handle:
            raise RuntimeError(self.lib.md_error().decode())
        try:
            specs = [self.manifest["quantization"]["input"]] + self.manifest[
                "quantization"
            ]["outputs"]
            if self.lib.md_output_count(self.handle) != len(specs) - 1:
                raise ValueError("NBG output count differs from manifest")
            self.metas, self.arrays = [], []
            formats = {0: np.float32, 1: np.float16, 2: np.uint8, 3: np.int8}
            for i, spec in enumerate(specs):
                meta = Meta()
                self.check(self.lib.md_info(self.handle, i - 1, C.byref(meta)))
                shape = list(reversed(meta.sizes[: meta.dims]))
                dtype = formats[meta.format]
                if shape != spec["shape"] or np.dtype(dtype).name != spec["dtype"]:
                    raise ValueError(
                        f"NBG tensor {i - 1} shape/dtype mismatch: {shape}/{dtype}"
                    )
                if meta.quant == 2:
                    if (
                        not np.isclose(meta.scale, spec["scale"], rtol=1e-6, atol=0)
                        or meta.zero != spec["zero_point"]
                    ):
                        raise ValueError(
                            f"NBG tensor {i - 1} quantization differs from manifest"
                        )
                elif np.issubdtype(dtype, np.integer):
                    raise ValueError("integer tensor without affine quantization")
                array = np.empty(shape, dtype=dtype)
                if array.nbytes != meta.bytes:
                    raise ValueError("NBG tensor byte size mismatch")
                self.metas.append(meta)
                self.arrays.append(array)
            self.pointers = (C.c_void_p * (len(specs) - 1))(
                *(a.ctypes.data for a in self.arrays[1:])
            )
            self.lengths = (C.c_uint32 * (len(specs) - 1))(
                *(a.nbytes for a in self.arrays[1:])
            )
        except BaseException:
            self.close()
            raise

    def check(self, status):
        if status:
            raise RuntimeError(self.lib.md_error().decode())

    def run(self, tensor):
        if threading.get_ident() != self.owner or not self.handle:
            raise RuntimeError(
                "VIPLite must run on its owner thread with an open network"
            )
        if (
            list(tensor.shape) != self.manifest["input"]["shape"]
            or not np.isfinite(tensor).all()
        ):
            raise ValueError("incorrect or non-finite model input")
        meta = self.metas[0]
        encoded = quantize(tensor, self.arrays[0].dtype, meta.scale, meta.zero)
        self.check(
            self.lib.md_run(
                self.handle,
                encoded.ctypes.data,
                encoded.nbytes,
                self.pointers,
                self.lengths,
            )
        )
        # Each result owns its storage; a later NPU call cannot mutate a published frame.
        return [
            (array.astype(np.float32) - meta.zero) * meta.scale
            if meta.quant == 2
            else array.astype(np.float32)
            for array, meta in zip(self.arrays[1:], self.metas[1:])
        ]

    def close(self):
        if self.handle:
            if threading.get_ident() != self.owner:
                raise RuntimeError("close VIPLite on its owner thread")
            self.lib.md_close(self.handle)
            self.handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
