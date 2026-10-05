"""Smoke-test an installed Python mstore v0.3 client against mstore-server (Rust).

Run in a shell where the original Python package is importable:

    cargo run --bin mstore-server -- --endpoint unix:///tmp/mstore-compat.sock
    python scripts/python_compat_smoke.py unix:///tmp/mstore-compat.sock
"""
from __future__ import annotations

import sys

import numpy as np
import mstore

endpoint = sys.argv[1] if len(sys.argv) > 1 else "unix:///tmp/mstore-compat.sock"

with mstore.connect(endpoint=endpoint) as store:
    assert store.ping()["pong"] is True
    image = store.create(shape=(32, 48, 3), dtype=np.uint8)
    image.numpy().fill(23)

    read_token = image.issue("read")
    with store.open(image.object_id, read_token, mode="read", cache=True) as reader:
        array = reader.numpy()
        assert array.shape == (32, 48, 3)
        assert int(array[0, 0, 0]) == 23
        assert array.flags.writeable is False

print("Python v0.3 -> Rust server compatibility: OK")
