"""Read mediad's existing binary media.frame contract. Never opens a camera device."""

import json
import socket
import time

MAX_HEADER = 4096
MAX_BYTES = 32 * 1024 * 1024


def validate(header):
    w, h = header["width"], header["height"]
    if (
        type(w) is not int
        or type(h) is not int
        or w <= 0
        or h <= 0
        or w % 2
        or header["format"] != "UYVY"
        or header["bytes"] != w * h * 2
        or not 0 < header["bytes"] <= MAX_BYTES
    ):
        raise ValueError("invalid packed UYVY header")
    if header["rotate"] != 0:
        raise ValueError("this experiment requires rotate=0")


def fetch(path="/run/mediad/media.sock", api_version=42, timeout=3.0):
    deadline = time.monotonic() + timeout
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.settimeout(timeout)
        conn.connect(str(path))
        with conn.makefile("rb") as reader:

            def reply(method, ident, params=None):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("raw frame deadline exceeded")
                conn.settimeout(remaining)
                request = {"jsonrpc": "2.0", "id": ident, "method": method}
                if params is not None:
                    request["params"] = params
                conn.sendall(json.dumps(request).encode() + b"\n")
                line = reader.readline(MAX_HEADER + 1)
                if len(line) > MAX_HEADER:
                    raise ValueError("invalid frame response length")
                if not line.endswith(b"\n"):
                    raise EOFError("truncated frame response header")
                response = json.loads(line)
                if response.get("id") != ident or response.get("jsonrpc") != "2.0":
                    raise ValueError("frame response ID/version mismatch")
                if "error" in response:
                    raise RuntimeError(response["error"])
                return response["result"]

            hello = reply("hello", 1, {"api_version": api_version})
            if hello["api_version"] != api_version:
                # Match the daemon policy: report skew, then try the actual required method.
                import warnings

                warnings.warn(
                    f"mediad API {hello['api_version']}, client API {api_version}"
                )
            header = reply("media.frame", 2)
            validate(header)
            chunks, remaining = [], header["bytes"]
            while remaining:
                wait = deadline - time.monotonic()
                if wait <= 0:
                    raise TimeoutError("raw frame deadline exceeded")
                conn.settimeout(wait)
                block = reader.read1(min(remaining, 65536))
                if not block:
                    raise EOFError("truncated raw frame")
                chunks.append(block)
                remaining -= len(block)
            return header, b"".join(chunks)
