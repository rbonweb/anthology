#!/usr/bin/env python3
"""Install the pinned rtk binary into the plugin data dir (Task 11).

`rtk.lock` pins a version, the release tag, the GitHub repo and the sha256 of each release
tarball (`rtk-<target>.tar.gz`, holding `rtk` and `LICENSE`). `install` downloads the tarball
for this machine, checks the hash BEFORE touching the contents, extracts only the member named
`rtk`, and places it at `<data>/rtk/<version>/rtk` so a session still on an older plugin
version keeps a matching binary. `reconcile` is what session start calls: it installs when the
binary is missing or does not match the lock, records the path in state.json (which
`bin/rtk` and `bin/rtk-hook` read), and turns any failure into a warning string.

Standard library only, Python 3.9+.

CLI: python3 rtk_install.py --data-dir PATH [--lock PATH] [--dry-run] [--timeout S]
"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import platform
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from pathlib import Path

import state

TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin",
           "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl")
_PLATFORMS = {
    ("darwin", "arm64"): "aarch64-apple-darwin",
    ("darwin", "aarch64"): "aarch64-apple-darwin",
    ("darwin", "x86_64"): "x86_64-apple-darwin",
    ("linux", "x86_64"): "x86_64-unknown-linux-musl",
    ("linux", "amd64"): "x86_64-unknown-linux-musl",
    ("linux", "aarch64"): "aarch64-unknown-linux-musl",
    ("linux", "arm64"): "aarch64-unknown-linux-musl",
}

VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?")
REPO_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SHA256_RE = re.compile(r"[0-9a-f]{64}")

MAX_DOWNLOAD_BYTES = 64 * 1024 * 1024   # the tarball
MAX_BINARY_BYTES = 128 * 1024 * 1024    # the extracted rtk
MAX_TAR_MEMBERS = 64
PRUNE_AFTER_DAYS = 30
STALE_TEMP_SECONDS = 24 * 3600
DEFAULT_TIMEOUT = 30


class InstallError(Exception):
    """Anything that stops rtk from being installed; the message is shown to the user."""


# --- platform and lock ------------------------------------------------------------------

def target(system: str, machine: str) -> str:
    s = (system or "").strip().lower()
    m = (machine or "").strip().lower()
    if s.startswith(("windows", "cygwin", "msys", "mingw")):
        raise InstallError("Windows isn't supported: rtk is installed on macOS and Linux only")
    try:
        return _PLATFORMS[(s, m)]
    except KeyError:
        raise InstallError("unsupported platform %s/%s: rtk is installed on macOS and Linux "
                           "(x86_64, arm64) only" % (system, machine)) from None


def validate_lock(lock) -> dict:
    """Return `lock` when it is exactly the documented shape; raise InstallError otherwise."""
    if not isinstance(lock, dict) or set(lock) != {"version", "tag", "repo", "sha256"}:
        raise InstallError("rtk.lock must be an object with exactly version, tag, repo and sha256")
    version, tag, repo, digests = lock["version"], lock["tag"], lock["repo"], lock["sha256"]
    if not isinstance(version, str) or not VERSION_RE.fullmatch(version):
        raise InstallError("rtk.lock: invalid version %r" % (version,))
    if tag != "rtk--v" + version:
        raise InstallError("rtk.lock: tag must be rtk--v%s" % version)
    if (not isinstance(repo, str) or not REPO_RE.fullmatch(repo)
            or any(part in (".", "..") for part in repo.split("/"))):
        raise InstallError("rtk.lock: invalid repo %r" % (repo,))
    if not isinstance(digests, dict) or set(digests) != set(TARGETS):
        raise InstallError("rtk.lock: sha256 must have exactly the targets " + ", ".join(TARGETS))
    for name, digest in digests.items():
        if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
            raise InstallError("rtk.lock: sha256 for %s must be 64 lowercase hex characters" % name)
    return lock


def load_lock(path) -> dict:
    try:
        raw = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        raise InstallError("cannot read %s: %s" % (path, exc)) from None
    try:
        data = json.loads(raw)
    except ValueError:
        raise InstallError("%s is not valid JSON" % path) from None
    return validate_lock(data)


def release_url(lock: dict, tgt: str) -> str:
    return "https://github.com/%s/releases/download/%s/rtk-%s.tar.gz" % (lock["repo"], lock["tag"], tgt)


# --- verify and fetch -------------------------------------------------------------------

def verify(path, sha256: str) -> bool:
    """True when the file's sha256 equals `sha256` (streamed, constant-time compare)."""
    digest = hashlib.sha256()
    try:
        with open(str(path), "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                digest.update(chunk)
    except OSError:
        return False
    return hmac.compare_digest(digest.hexdigest().encode("ascii"), str(sha256).lower().encode("utf-8"))


class HttpsOnlyRedirect(urllib.request.HTTPRedirectHandler):
    """GitHub release downloads redirect to a CDN; refuse any hop that is not https."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urllib.parse.urlsplit(newurl).scheme.lower() != "https":
            raise urllib.error.URLError("refusing redirect to a non-https URL")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def default_fetch(url: str, dest_path, timeout: float) -> None:
    """Download `url` (https only) to `dest_path`: size-capped, `timeout` per socket operation
    and four times that overall (checked after every receive, so it overshoots by at most one
    socket timeout)."""
    if urllib.parse.urlsplit(url).scheme.lower() != "https":
        raise InstallError("refusing to download over a non-https URL: %s" % url)
    opener = urllib.request.build_opener(HttpsOnlyRedirect)
    request = urllib.request.Request(url, headers={"User-Agent": "anthology-rtk-installer"})
    deadline = time.monotonic() + 4 * timeout
    try:
        with opener.open(request, timeout=timeout) as resp:
            if urllib.parse.urlsplit(resp.geturl()).scheme.lower() != "https":
                raise InstallError("download ended on a non-https URL")
            length = resp.headers.get("Content-Length")
            if length and length.isdigit() and int(length) > MAX_DOWNLOAD_BYTES:
                raise InstallError("download is larger than %d bytes" % MAX_DOWNLOAD_BYTES)
            total = 0
            # read1 returns whatever has arrived instead of waiting for a full chunk, so the
            # deadline is checked after every receive and a trickling server cannot stretch it.
            read = getattr(resp, "read1", None) or resp.read
            with open(str(dest_path), "wb") as out:
                while True:
                    if time.monotonic() > deadline:
                        raise InstallError("download timed out")
                    chunk = read(1 << 14)
                    if not chunk:
                        break
                    total += len(chunk)
                    if total > MAX_DOWNLOAD_BYTES:
                        raise InstallError("download is larger than %d bytes" % MAX_DOWNLOAD_BYTES)
                    out.write(chunk)
    except BaseException:
        try:
            os.unlink(str(dest_path))
        except OSError:
            pass
        raise


# --- install ----------------------------------------------------------------------------

def _is_current(vdir: Path, expected: str) -> bool:
    """The version dir holds an executable regular-file rtk whose recorded tarball hash is `expected`."""
    try:
        if stat.S_ISLNK(os.lstat(str(vdir)).st_mode):
            return False
        if not stat.S_ISREG(os.lstat(str(vdir / "rtk")).st_mode) or not os.access(str(vdir / "rtk"), os.X_OK):
            return False
        recorded = (vdir / ".sha256").read_text(encoding="utf-8").strip()
    except OSError:
        return False
    return hmac.compare_digest(recorded.encode("utf-8"), expected.encode("utf-8"))


def _extract_rtk(archive: Path, dest: Path) -> None:
    """Copy the one regular-file member named exactly `rtk` to `dest`; never extracts anything else."""
    found = False
    try:
        with tarfile.open(str(archive), "r:gz") as tf:
            for count, member in enumerate(tf, 1):
                if count > MAX_TAR_MEMBERS:
                    raise InstallError("tarball has too many members")
                if member.name != "rtk":
                    continue
                if found:
                    raise InstallError("tarball has more than one rtk member")
                if not member.isreg():
                    raise InstallError("the rtk member is not a regular file")
                if member.size > MAX_BINARY_BYTES:
                    raise InstallError("the rtk member is too large")
                found = True
                src = tf.extractfile(member)
                fd = os.open(str(dest), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
                with os.fdopen(fd, "wb") as out:
                    written = 0
                    for chunk in iter(lambda: src.read(1 << 16), b""):
                        written += len(chunk)
                        if written > MAX_BINARY_BYTES:
                            raise InstallError("the rtk member is too large")
                        out.write(chunk)
    except (tarfile.TarError, EOFError, zlib.error) as exc:
        raise InstallError("cannot read the rtk tarball: %s" % exc) from None
    if not found:
        raise InstallError("tarball has no rtk member")


def _touch(path: Path) -> None:
    with open(str(path), "a"):
        pass
    os.utime(str(path), None)


def _download_and_place(root: Path, vdir: Path, url: str, expected: str, fetch, timeout) -> None:
    root.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix=".tmp-", dir=str(root)))  # same filesystem as the target
    try:
        archive = tmp / "archive.tar.gz"
        fetch(url, archive, timeout)
        if not verify(archive, expected):
            raise InstallError("sha256 of %s does not match rtk.lock" % url)
        staged = tmp / "rtk"
        _extract_rtk(archive, staged)
        os.chmod(str(staged), 0o755)
        if vdir.is_symlink():
            raise InstallError("%s is a symlink" % vdir)
        created = not vdir.exists()
        vdir.mkdir(exist_ok=True)
        try:
            (vdir / ".sha256").unlink(missing_ok=True)  # the marker is written last
            os.replace(str(staged), str(vdir / "rtk"))
            marker = tmp / "marker"
            marker.write_text(expected + "\n", encoding="utf-8")
            os.replace(str(marker), str(vdir / ".sha256"))
        except BaseException:
            if created:
                shutil.rmtree(str(vdir), ignore_errors=True)
            raise
    finally:
        shutil.rmtree(str(tmp), ignore_errors=True)


def prune(root, current: str, now: float | None = None) -> list:
    """Delete version dirs directly under `root` unused for 30 days; never `current`, never
    anything that is not a plain directory named like a version. Returns the names removed."""
    now = time.time() if now is None else now
    removed = []
    try:
        entries = list(os.scandir(str(root)))
    except OSError:
        return removed
    for entry in entries:
        if entry.name == current:
            continue
        try:
            if entry.is_symlink() or not entry.is_dir(follow_symlinks=False):
                continue
            if VERSION_RE.fullmatch(entry.name):
                try:
                    stamp = os.lstat(os.path.join(entry.path, ".last-used")).st_mtime
                except OSError:
                    stamp = entry.stat(follow_symlinks=False).st_mtime
                limit = PRUNE_AFTER_DAYS * 86400
            elif entry.name.startswith(".tmp-"):  # left by a crashed install
                stamp = entry.stat(follow_symlinks=False).st_mtime
                limit = STALE_TEMP_SECONDS
            else:
                continue
            if now - stamp > limit:
                shutil.rmtree(entry.path)
                removed.append(entry.name)
        except OSError:
            continue
    return removed


def install(data_dir, lock: dict, fetch, timeout: float = DEFAULT_TIMEOUT, tgt: str | None = None) -> Path:
    """Make `<data>/rtk/<version>/rtk` exist and match `lock`; return its path.

    Fetches only when the binary is missing or its recorded tarball hash differs from the lock.
    Raises (InstallError, or whatever `fetch` raises) on failure, leaving nothing behind.
    """
    validate_lock(lock)
    if tgt is None:
        tgt = target(platform.system(), platform.machine())
    if tgt not in lock["sha256"]:
        raise InstallError("no rtk build for %s" % tgt)
    expected = lock["sha256"][tgt]
    root = Path(data_dir) / "rtk"
    vdir = root / lock["version"]
    if not _is_current(vdir, expected):
        _download_and_place(root, vdir, release_url(lock, tgt), expected, fetch, timeout)
    _touch(vdir / ".last-used")
    prune(root, lock["version"])
    return vdir / "rtk"


def check_installed(data_dir, lock: dict, tgt: str | None = None) -> bool:
    """True when `<data>/rtk/<lock version>/rtk` is an executable regular file whose recorded
    tarball hash (`.sha256`) equals the lock's hash for `tgt` (default: this machine). Read-only:
    never downloads, never raises. (The lock pins the tarball, so the binary itself has no hash
    to compare; the marker is written only after the verified tarball was extracted.)"""
    try:
        validate_lock(lock)
        if tgt is None:
            tgt = target(platform.system(), platform.machine())
        return _is_current(Path(data_dir) / "rtk" / lock["version"], lock["sha256"][tgt])
    except Exception:  # noqa: BLE001
        return False


def reconcile(data_dir, lock: dict, fetch=None, timeout: float = DEFAULT_TIMEOUT,
              system: str | None = None, machine: str | None = None) -> dict:
    """Session-start entry point. Never raises.

    Returns {"rtk_path": str | None, "warning": str | None, "installed": bool}. On success
    rtk_path is the binary and state.json's `rtk_path` is updated (`installed` is True only
    when something was downloaded). On any failure rtk_path is None, warning explains it, and
    state.json and older version dirs are left as they were.
    """
    result = {"rtk_path": None, "warning": None, "installed": False}
    try:
        validate_lock(lock)
        tgt = target(system if system is not None else platform.system(),
                     machine if machine is not None else platform.machine())
        vdir = Path(data_dir) / "rtk" / lock["version"]
        was_current = _is_current(vdir, lock["sha256"][tgt])
        path = install(data_dir, lock, fetch or default_fetch, timeout, tgt=tgt)
        if state.read().get("rtk_path") != str(path):
            state.write(rtk_path=str(path))
        result.update(rtk_path=str(path), installed=not was_current)
    except Exception as exc:  # noqa: BLE001  (a failure here must never break session start)
        result["warning"] = ("rtk is not installed or up to date (%s); commands run without it. "
                             "It is retried at the next session start." % (exc,))
    return result


# --- CLI --------------------------------------------------------------------------------

def main(argv=None) -> int:
    default_lock = Path(__file__).resolve().parent.parent / "rtk.lock"
    parser = argparse.ArgumentParser(description="Install the rtk binary pinned by rtk.lock.")
    parser.add_argument("--data-dir", required=True, help="plugin data dir; rtk goes to <dir>/rtk/<version>/")
    parser.add_argument("--lock", default=str(default_lock), help="path to rtk.lock (default: next to scripts/)")
    parser.add_argument("--dry-run", action="store_true", help="print the plan and fetch nothing")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="network timeout in seconds")
    args = parser.parse_args(argv)
    try:
        lock = load_lock(args.lock)
        tgt = target(platform.system(), platform.machine())
    except InstallError as exc:
        print("rtk_install: %s" % exc, file=sys.stderr)
        return 2
    dest = Path(args.data_dir) / "rtk" / lock["version"] / "rtk"
    if args.dry_run:
        print("target:      %s" % tgt)
        print("url:         %s" % release_url(lock, tgt))
        print("sha256:      %s" % lock["sha256"][tgt])
        print("destination: %s" % dest)
        return 0
    result = reconcile(args.data_dir, lock, default_fetch, args.timeout)
    if result["warning"]:
        print("rtk_install: %s" % result["warning"], file=sys.stderr)
        return 1
    print("%s rtk %s at %s" % ("installed" if result["installed"] else "already installed:",
                              lock["version"], result["rtk_path"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
