"""Shared helpers for the tests that drive the REAL rtk_install with a fake download.

Session start, /anthology:setup and doctor each call rtk_install; their other tests use a fake
module, so each also has one test built on these helpers. Those tests are what stops the callers
and the installer from drifting apart: a renamed function or a changed return shape fails them.
"""
import hashlib
import io
import json
import tarfile
from pathlib import Path

LINUX = ("Linux", "x86_64")
TARGET = "x86_64-unknown-linux-musl"
ALL_TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin",
               "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl")
VERSION = "1.2.1-anthology.1"
RTK_BODY = b"#!/bin/sh\necho fake-rtk\n"


def tarball(body=RTK_BODY):
    """An in-memory rtk release tarball: `rtk` and `LICENSE`."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tf:
        for name, payload in (("rtk", body), ("LICENSE", b"license\n")):
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            info.mode = 0o644
            tf.addfile(info, io.BytesIO(payload))
    return buf.getvalue()


def write_lock(path, blob, version=VERSION):
    """Write a valid rtk.lock whose linux/x86_64 hash is `blob`'s; returns the parsed lock."""
    digests = {t: hashlib.sha256(t.encode()).hexdigest() for t in ALL_TARGETS}
    digests[TARGET] = hashlib.sha256(blob).hexdigest()
    lock = {"version": version, "tag": "rtk--v" + version, "repo": "example/anthology", "sha256": digests}
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    return lock


class FakeFetch:
    """Stands in for rtk_install.default_fetch(url, dest, timeout): writes `blob`, records calls."""

    def __init__(self, blob):
        self.blob, self.calls = blob, []

    def __call__(self, url, dest, timeout):
        self.calls.append((url, str(dest), timeout))
        Path(dest).write_bytes(self.blob)
