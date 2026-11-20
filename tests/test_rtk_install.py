"""Tests for the rtk wiring in the anthology plugin (Task 11).

Covers scripts/rtk_install.py (target mapping, lock validation, verified per-version install,
reconcile, prune, default fetch), hooks/hooks.json, and the POSIX sh wrappers bin/rtk and
bin/rtk-hook. No network: every fetch is a fake that writes an in-memory tarball.
"""
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
PLUGIN = ROOT / "plugins" / "anthology"
sys.path.insert(0, str(PLUGIN / "scripts"))

import rtk_install  # noqa: E402
import state  # noqa: E402

TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin",
           "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]
HOST = "x86_64-unknown-linux-musl"
RTK_BODY = b"#!/bin/sh\necho fake-rtk\n"


def tarball(members=None):
    """A .tar.gz as bytes. `members` is a list of (name, kind, payload); default rtk + LICENSE."""
    if members is None:
        members = [("rtk", "file", RTK_BODY), ("LICENSE", "file", b"license text\n")]
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tf:
        for name, kind, payload in members:
            info = tarfile.TarInfo(name)
            if kind == "file":
                info.size = len(payload)
                info.mode = 0o644
                tf.addfile(info, io.BytesIO(payload))
            elif kind == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = payload
                tf.addfile(info)
            elif kind == "hardlink":
                info.type = tarfile.LNKTYPE
                info.linkname = payload
                tf.addfile(info)
            else:
                raise AssertionError(kind)
    return buf.getvalue()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def make_lock(version="1.2.1-anthology.1", blob=None, repo="rbonweb/anthology"):
    """A valid lock whose HOST target hashes `blob`; other targets get distinct fake hashes."""
    blob = tarball() if blob is None else blob
    digests = {t: sha(t.encode()) for t in TARGETS}
    digests[HOST] = sha(blob)
    return {"version": version, "tag": "rtk--v" + version, "repo": repo, "sha256": digests}


class FakeFetch:
    """fetch(url, dest, timeout): writes the blob registered for the URL; records every call."""

    def __init__(self, blobs=None, error=None):
        self.blobs = blobs or {}
        self.error = error
        self.calls = []

    def __call__(self, url, dest, timeout):
        self.calls.append((url, str(dest), timeout))
        if self.error:
            raise self.error
        Path(dest).write_bytes(self.blobs[url])


def url_for(lock, target=HOST):
    return "https://github.com/%s/releases/download/%s/rtk-%s.tar.gz" % (
        lock["repo"], lock["tag"], target)


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="t11-"))
        self.addCleanup(shutil.rmtree, str(self.tmp), True)
        self.data = self.tmp / "data"
        self.cfg = self.tmp / "config"
        patcher = mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(self.cfg),
                                               "HOME": str(self.tmp / "home")})
        patcher.start()
        self.addCleanup(patcher.stop)

    def entries(self, path):
        return sorted(os.listdir(str(path))) if Path(path).exists() else []

    def fetch_for(self, lock, blob):
        return FakeFetch({url_for(lock): blob})


class TargetTest(unittest.TestCase):
    def test_supported_platforms(self):
        cases = [("Darwin", "arm64", "aarch64-apple-darwin"),
                 ("Darwin", "aarch64", "aarch64-apple-darwin"),
                 ("Darwin", "x86_64", "x86_64-apple-darwin"),
                 ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
                 ("Linux", "amd64", "x86_64-unknown-linux-musl"),
                 ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
                 ("Linux", "arm64", "aarch64-unknown-linux-musl")]
        for system, machine, want in cases:
            self.assertEqual(rtk_install.target(system, machine), want, (system, machine))

    def test_windows_is_refused_in_any_case(self):
        for system in ("Windows", "windows", "WINDOWS", "Windows_NT"):
            with self.assertRaises(Exception) as cm:
                rtk_install.target(system, "AMD64")
            self.assertIn("Windows isn't supported", str(cm.exception))

    def test_everything_else_is_refused(self):
        for system, machine in (("FreeBSD", "amd64"), ("Linux", "riscv64"), ("Darwin", "ppc"), ("", "")):
            with self.assertRaises(Exception):
                rtk_install.target(system, machine)


class LoadLockTest(Base):
    def load(self, obj):
        path = self.tmp / "rtk.lock"
        path.write_text(obj if isinstance(obj, str) else json.dumps(obj), encoding="utf-8")
        return rtk_install.load_lock(path)

    def test_valid_lock_round_trips(self):
        lock = make_lock()
        self.assertEqual(self.load(lock), lock)

    def test_release_without_prerelease_is_valid(self):
        self.assertEqual(self.load(make_lock("2.0.0"))["tag"], "rtk--v2.0.0")

    def test_rejects_invalid_locks(self):
        def mutate(fn):
            lock = make_lock()
            fn(lock)
            return lock
        bad = {
            "uppercase hex": mutate(lambda l: l["sha256"].update({HOST: "A" * 64})),
            "short hex": mutate(lambda l: l["sha256"].update({HOST: "a" * 63})),
            "non-hex": mutate(lambda l: l["sha256"].update({HOST: "g" * 64})),
            "trailing newline": mutate(lambda l: l["sha256"].update({HOST: "a" * 64 + "\n"})),
            "wrong tag": mutate(lambda l: l.update(tag="rtk--v9.9.9")),
            "no tag prefix": mutate(lambda l: l.update(tag="v1.2.1-anthology.1")),
            "extra target": mutate(lambda l: l["sha256"].update({"x86_64-pc-windows-msvc": "a" * 64})),
            "missing target": mutate(lambda l: l["sha256"].pop("x86_64-apple-darwin")),
            "bad version": mutate(lambda l: l.update(version="1.2", tag="rtk--v1.2")),
            "path version": mutate(lambda l: l.update(version="../1.0.0", tag="rtk--v../1.0.0")),
            "unsafe repo": mutate(lambda l: l.update(repo="a/b/../../c")),
            "repo with space": mutate(lambda l: l.update(repo="a b/c")),
            "repo with query": mutate(lambda l: l.update(repo="a/b?x=1")),
            "repo no slash": mutate(lambda l: l.update(repo="rbonweb")),
            "unknown key": mutate(lambda l: l.update(extra="x")),
            "missing key": mutate(lambda l: l.pop("repo")),
            "sha not a map": mutate(lambda l: l.update(sha256=[])),
            "not an object": [],
        }
        for name, lock in bad.items():
            with self.subTest(name):
                with self.assertRaises(Exception):
                    self.load(lock)

    def test_rejects_non_json(self):
        with self.assertRaises(Exception):
            self.load("not json")


class VerifyTest(Base):
    def test_verify(self):
        path = self.tmp / "blob"
        path.write_bytes(b"hello")
        self.assertIs(rtk_install.verify(path, sha(b"hello")), True)
        self.assertIs(rtk_install.verify(path, sha(b"other")), False)
        self.assertIs(rtk_install.verify(self.tmp / "missing", sha(b"hello")), False)


class InstallTest(Base):
    def install(self, lock, fetch, **kw):
        return rtk_install.install(self.data, lock, fetch, tgt=HOST, **kw)

    def test_good_hash_installs_per_version(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        path = self.install(lock, fetch)
        self.assertEqual(path, self.data / "rtk" / "1.2.1-anthology.1" / "rtk")
        self.assertEqual(path.read_bytes(), RTK_BODY)
        self.assertEqual(os.stat(str(path)).st_mode & 0o777, 0o755)
        self.assertEqual(fetch.calls[0][0], url_for(lock))
        vdir = path.parent
        self.assertEqual((vdir / ".sha256").read_text().strip(), lock["sha256"][HOST])
        self.assertEqual(self.entries(vdir), [".last-used", ".sha256", "rtk"])
        self.assertEqual(self.entries(self.data / "rtk"), ["1.2.1-anthology.1"])

    def test_bad_hash_leaves_nothing_behind(self):
        lock = make_lock()
        lock["sha256"][HOST] = "0" * 64
        fetch = self.fetch_for(lock, tarball())
        with self.assertRaises(Exception):
            self.install(lock, fetch)
        self.assertEqual(len(fetch.calls), 1)
        self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_second_run_does_not_fetch(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        first = self.install(lock, fetch)
        second = self.install(lock, fetch)
        self.assertEqual(first, second)
        self.assertEqual(len(fetch.calls), 1)

    def test_reinstalls_when_marker_disagrees_with_lock(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        path = self.install(lock, fetch)
        (path.parent / ".sha256").write_text("f" * 64 + "\n")
        self.install(lock, fetch)
        self.assertEqual(len(fetch.calls), 2)
        self.assertEqual((path.parent / ".sha256").read_text().strip(), lock["sha256"][HOST])

    def test_reinstalls_when_binary_is_not_executable(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        path = self.install(lock, fetch)
        os.chmod(str(path), 0o644)
        self.install(lock, fetch)
        self.assertEqual(len(fetch.calls), 2)
        self.assertTrue(os.access(str(path), os.X_OK))

    def test_rejected_tarballs_leave_nothing_behind(self):
        cases = {
            "symlink rtk": [("rtk", "symlink", "/bin/sh")],
            "hardlink rtk": [("rtk", "hardlink", "LICENSE"), ("LICENSE", "file", b"x")],
            "dotdot member": [("../rtk", "file", RTK_BODY), ("LICENSE", "file", b"x")],
            "nested member": [("bin/rtk", "file", RTK_BODY)],
            "absolute member": [("/rtk", "file", RTK_BODY)],
            "no rtk member": [("LICENSE", "file", b"x")],
            "duplicate rtk": [("rtk", "file", RTK_BODY), ("rtk", "file", b"other")],
        }
        for name, members in cases.items():
            with self.subTest(name):
                blob = tarball(members)
                lock = make_lock(blob=blob)
                with self.assertRaises(Exception):
                    self.install(lock, self.fetch_for(lock, blob))
                self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_oversized_member_is_rejected(self):
        blob = tarball([("rtk", "file", b"x" * 2048)])
        lock = make_lock(blob=blob)
        with mock.patch.object(rtk_install, "MAX_BINARY_BYTES", 1024):
            with self.assertRaises(Exception):
                self.install(lock, self.fetch_for(lock, blob))
        self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_garbage_that_hashes_correctly_is_rejected(self):
        blob = b"this is not a tarball"
        lock = make_lock(blob=blob)
        with self.assertRaises(Exception):
            self.install(lock, self.fetch_for(lock, blob))
        self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_failed_fetch_leaves_nothing_behind(self):
        lock = make_lock()
        with self.assertRaises(Exception):
            self.install(lock, FakeFetch(error=OSError("boom")))
        self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_the_hash_is_checked_before_extraction(self):
        blob = tarball()
        lock = make_lock(blob=blob)
        lock["sha256"][HOST] = "1" * 64
        with mock.patch.object(rtk_install.tarfile, "open", side_effect=AssertionError("opened")):
            with self.assertRaises(Exception) as cm:
                self.install(lock, self.fetch_for(lock, blob))
        self.assertNotIsInstance(cm.exception, AssertionError)


class PruneTest(Base):
    def make_version(self, name, age_days, marker=True):
        vdir = self.data / "rtk" / name
        vdir.mkdir(parents=True)
        (vdir / "rtk").write_bytes(b"x")
        when = time.time() - age_days * 86400
        if marker:
            (vdir / ".last-used").write_text("")
            os.utime(str(vdir / ".last-used"), (when, when))
        os.utime(str(vdir), (when, when))
        return vdir

    def test_install_prunes_old_versions_but_not_current_or_recent(self):
        old = self.make_version("0.9.0", 45)
        recent = self.make_version("1.0.0", 5)
        no_marker_old = self.make_version("0.8.0", 90, marker=False)
        stray = self.data / "rtk" / "notes"
        stray.mkdir()
        os.utime(str(stray), (1, 1))
        link_target = self.tmp / "elsewhere"
        link_target.mkdir()
        (link_target / "keep").write_text("x")
        os.utime(str(link_target), (1, 1))
        link = self.data / "rtk" / "0.1.0"
        os.symlink(str(link_target), str(link))

        lock = make_lock()
        current = self.data / "rtk" / lock["version"]
        current.mkdir()
        (current / "rtk").write_bytes(b"x")
        (current / ".last-used").write_text("")
        os.utime(str(current / ".last-used"), (1, 1))  # ancient, but it is the current version
        os.utime(str(current), (1, 1))
        fetch = self.fetch_for(lock, tarball())
        rtk_install.install(self.data, lock, fetch, tgt=HOST)

        self.assertFalse(old.exists())
        self.assertFalse(no_marker_old.exists())
        self.assertTrue(recent.exists())
        self.assertTrue(current.exists())
        self.assertTrue(stray.exists())
        self.assertTrue(link.is_symlink())
        self.assertTrue((link_target / "keep").exists())

    def test_install_touches_last_used_on_the_current_version(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        path = rtk_install.install(self.data, lock, fetch, tgt=HOST)
        marker = path.parent / ".last-used"
        os.utime(str(marker), (1, 1))
        rtk_install.install(self.data, lock, fetch, tgt=HOST)
        self.assertGreater(os.stat(str(marker)).st_mtime, time.time() - 60)
        self.assertEqual(len(fetch.calls), 1)


class ReconcileTest(Base):
    def reconcile(self, lock, fetch, **kw):
        kw.setdefault("system", "Linux")
        kw.setdefault("machine", "x86_64")
        return rtk_install.reconcile(self.data, lock, fetch, 5, **kw)

    def test_installs_and_records_the_path(self):
        lock = make_lock()
        res = self.reconcile(lock, self.fetch_for(lock, tarball()))
        want = str(self.data / "rtk" / lock["version"] / "rtk")
        self.assertEqual(res, {"rtk_path": want, "warning": None, "installed": True})
        self.assertEqual(state.read()["rtk_path"], want)

    def test_already_installed_does_not_fetch(self):
        lock = make_lock()
        fetch = self.fetch_for(lock, tarball())
        self.reconcile(lock, fetch)
        res = self.reconcile(lock, fetch)
        self.assertFalse(res["installed"])
        self.assertIsNone(res["warning"])
        self.assertEqual(len(fetch.calls), 1)

    def test_lock_change_installs_new_version_keeps_old_and_updates_state(self):
        old_lock = make_lock("1.2.1-anthology.1")
        self.reconcile(old_lock, self.fetch_for(old_lock, tarball()))
        blob2 = tarball([("rtk", "file", b"#!/bin/sh\necho v2\n")])
        new_lock = make_lock("1.3.0-anthology.1", blob=blob2)
        res = self.reconcile(new_lock, self.fetch_for(new_lock, blob2))
        self.assertTrue(res["installed"])
        self.assertEqual(self.entries(self.data / "rtk"), ["1.2.1-anthology.1", "1.3.0-anthology.1"])
        self.assertEqual(state.read()["rtk_path"],
                         str(self.data / "rtk" / "1.3.0-anthology.1" / "rtk"))
        self.assertTrue((self.data / "rtk" / "1.2.1-anthology.1" / "rtk").exists())

    def test_fetch_failure_returns_a_warning_and_keeps_state(self):
        lock = make_lock()
        state.write(rtk_path="/previous/rtk", other="kept")
        for error in (OSError("offline"), urllib.error.URLError("dns"), RuntimeError("weird")):
            res = self.reconcile(lock, FakeFetch(error=error))
            self.assertIsNone(res["rtk_path"])
            self.assertFalse(res["installed"])
            self.assertIn("rtk", res["warning"])
        self.assertEqual(state.read(), {"rtk_path": "/previous/rtk", "other": "kept"})
        self.assertEqual(self.entries(self.data / "rtk"), [])

    def test_bad_hash_returns_a_warning(self):
        lock = make_lock()
        lock["sha256"][HOST] = "2" * 64
        res = self.reconcile(lock, self.fetch_for(lock, tarball()))
        self.assertIsNone(res["rtk_path"])
        self.assertIn("sha256", res["warning"])
        self.assertNotIn("rtk_path", state.read())

    def test_unsupported_platform_returns_a_warning(self):
        lock = make_lock()
        fetch = FakeFetch()
        res = self.reconcile(lock, fetch, system="Windows", machine="AMD64")
        self.assertIn("Windows isn't supported", res["warning"])
        self.assertEqual(fetch.calls, [])

    def test_uses_the_running_platform_by_default(self):
        blob = tarball()
        lock = make_lock(blob=blob)
        fetch = self.fetch_for(lock, blob)
        with mock.patch.object(rtk_install.platform, "system", return_value="Linux"), \
                mock.patch.object(rtk_install.platform, "machine", return_value="x86_64"):
            res = rtk_install.reconcile(self.data, lock, fetch, 5)
        self.assertTrue(res["installed"])


class DefaultFetchTest(Base):
    def test_refuses_non_https_urls(self):
        for url in ("http://example.invalid/x", "file:///etc/passwd", "ftp://example.invalid/x"):
            with mock.patch.object(urllib.request, "urlopen", side_effect=AssertionError("network")):
                with self.assertRaises(Exception) as cm:
                    rtk_install.default_fetch(url, self.tmp / "out", 5)
            self.assertNotIsInstance(cm.exception, AssertionError)
            self.assertFalse((self.tmp / "out").exists())

    def test_refuses_redirects_to_non_https(self):
        handler = rtk_install.HttpsOnlyRedirect()
        req = urllib.request.Request("https://example.invalid/a")
        with self.assertRaises(urllib.error.URLError):
            handler.redirect_request(req, io.BytesIO(), 302, "Found", {}, "http://example.invalid/b")

    def test_https_redirect_is_followed(self):
        handler = rtk_install.HttpsOnlyRedirect()
        req = urllib.request.Request("https://example.invalid/a")
        new = handler.redirect_request(req, io.BytesIO(), 302, "Found", {}, "https://objects.example.invalid/b")
        self.assertEqual(new.full_url, "https://objects.example.invalid/b")


class CliTest(Base):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, str(PLUGIN / "scripts" / "rtk_install.py")] + list(args),
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True,
                              env=dict(os.environ))

    def write_lock(self, lock):
        path = self.tmp / "rtk.lock"
        path.write_text(json.dumps(lock))
        return path

    def test_dry_run_prints_the_plan_and_changes_nothing(self):
        lock = make_lock()
        lock_path = self.write_lock(lock)
        with mock.patch.object(rtk_install.platform, "system", return_value="Linux"), \
                mock.patch.object(rtk_install.platform, "machine", return_value="x86_64"), \
                mock.patch("sys.stdout", new_callable=io.StringIO) as out:
            code = rtk_install.main(["--data-dir", str(self.data), "--lock", str(lock_path), "--dry-run"])
        self.assertEqual(code, 0)
        text = out.getvalue()
        self.assertIn(HOST, text)
        self.assertIn(url_for(lock), text)
        self.assertIn(str(self.data / "rtk" / lock["version"] / "rtk"), text)
        self.assertFalse(self.data.exists())

    def test_bad_lock_exits_non_zero(self):
        lock_path = self.tmp / "rtk.lock"
        lock_path.write_text("{}")
        res = self.run_cli("--data-dir", str(self.data), "--lock", str(lock_path), "--dry-run")
        self.assertNotEqual(res.returncode, 0)
        self.assertTrue(res.stderr.strip())

    def test_missing_lock_exits_non_zero(self):
        res = self.run_cli("--data-dir", str(self.data), "--lock", str(self.tmp / "nope.lock"))
        self.assertNotEqual(res.returncode, 0)

    def test_install_failure_exits_non_zero_with_a_message(self):
        lock_path = self.write_lock(make_lock())
        with mock.patch.object(rtk_install.platform, "system", return_value="Linux"), \
                mock.patch.object(rtk_install.platform, "machine", return_value="x86_64"), \
                mock.patch.object(rtk_install, "default_fetch", FakeFetch(error=OSError("offline"))), \
                mock.patch("sys.stderr", new_callable=io.StringIO) as err:
            code = rtk_install.main(["--data-dir", str(self.data), "--lock", str(lock_path)])
        self.assertNotEqual(code, 0)
        self.assertIn("offline", err.getvalue())

    def test_install_success_exits_zero_and_records_state(self):
        blob = tarball()
        lock = make_lock(blob=blob)
        lock_path = self.write_lock(lock)
        with mock.patch.object(rtk_install.platform, "system", return_value="Linux"), \
                mock.patch.object(rtk_install.platform, "machine", return_value="x86_64"), \
                mock.patch.object(rtk_install, "default_fetch", self.fetch_for(lock, blob)), \
                mock.patch("sys.stdout", new_callable=io.StringIO):
            code = rtk_install.main(["--data-dir", str(self.data), "--lock", str(lock_path)])
        self.assertEqual(code, 0)
        self.assertEqual(state.read()["rtk_path"], str(self.data / "rtk" / lock["version"] / "rtk"))


class FilesTest(unittest.TestCase):
    def test_hooks_json_shape(self):
        hooks = json.loads((PLUGIN / "hooks" / "hooks.json").read_text(encoding="utf-8"))
        self.assertEqual(hooks, {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
            {"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/bin/rtk-hook"]}]}]}})

    def test_wrappers_are_executable_posix_sh(self):
        for name in ("rtk", "rtk-hook"):
            path = PLUGIN / "bin" / name
            self.assertTrue(os.access(str(path), os.X_OK), name)
            self.assertEqual(path.read_text(encoding="utf-8").splitlines()[0], "#!/bin/sh")

    def test_installer_is_python39_compatible(self):
        text = (PLUGIN / "scripts" / "rtk_install.py").read_text(encoding="utf-8")
        self.assertIn("from __future__ import annotations", text)
        self.assertNotIn("extractall", text)


@unittest.skipUnless(shutil.which("sh"), "sh is not installed")
class WrapperTest(Base):
    def setUp(self):
        super().setUp()
        self.fake = self.tmp / "fake-rtk"
        self.fake.write_text('#!/bin/sh\necho "args:$*"\necho "suppress:$RTK_SUPPRESS_HOOK_WARNING"\n'
                             'if [ "$1" = hook ]; then cat; fi\n')
        os.chmod(str(self.fake), 0o755)

    def run_bin(self, name, *args, stdin=""):
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(self.tmp / "home"),
               "CLAUDE_CONFIG_DIR": str(self.cfg)}
        return subprocess.run([str(PLUGIN / "bin" / name)] + list(args), input=stdin,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              universal_newlines=True, env=env)

    def copy_fake(self, dest):
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(str(self.fake), str(dest))
        os.chmod(str(dest), 0o755)
        return dest

    def test_rtk_execs_the_recorded_binary(self):
        state.write(rtk_path=str(self.fake))
        res = self.run_bin("rtk", "git", "status")
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(res.stdout.splitlines(), ["args:git status", "suppress:1"])

    def test_rtk_uses_home_when_config_dir_is_unset(self):
        home = self.tmp / "home"
        (home / ".claude" / "anthology").mkdir(parents=True)
        (home / ".claude" / "anthology" / "state.json").write_text(
            json.dumps({"rtk_path": str(self.fake)}, indent=2, sort_keys=True) + "\n")
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(home)}
        res = subprocess.run([str(PLUGIN / "bin" / "rtk"), "x"], stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, universal_newlines=True, env=env)
        self.assertEqual(res.stdout.splitlines()[0], "args:x")

    def test_rtk_finds_the_key_wherever_it_sits(self):
        state.write(data_dir="/d", rtk_path=str(self.fake), zzz="after")  # rtk_path mid-file
        self.assertEqual(self.run_bin("rtk", "a").stdout.splitlines()[0], "args:a")
        state.state_path().write_text(json.dumps({"rtk_path": str(self.fake)}, indent=2, sort_keys=True) + "\n")
        self.assertEqual(self.run_bin("rtk", "b").stdout.splitlines()[0], "args:b")  # sole (last) key

    def test_rtk_handles_a_path_with_spaces(self):
        state.write(rtk_path=str(self.copy_fake(self.tmp / "with space" / "rtk")))
        self.assertEqual(self.run_bin("rtk", "ok").stdout.splitlines()[0], "args:ok")

    def test_rtk_non_ascii_path_round_trips(self):
        state.write(rtk_path=str(self.copy_fake(self.tmp / "h\u00e9llo" / "rtk")))
        self.assertEqual(self.run_bin("rtk", "ok").stdout.splitlines()[0], "args:ok")

    def test_rtk_missing_state_exits_127_with_the_message(self):
        res = self.run_bin("rtk", "git", "status")
        self.assertEqual(res.returncode, 127)
        self.assertIn("rtk is not installed: run /anthology:setup", res.stderr)
        self.assertEqual(res.stdout, "")

    def test_rtk_unusable_paths_exit_127(self):
        notexec = self.tmp / "notexec"
        notexec.write_text("#!/bin/sh\n")
        for value in (str(self.tmp / "gone"), str(notexec), str(self.tmp), "relative/rtk", "",
                      str(self.fake) + '"; touch /tmp/pwned; "', "C:\\rtk.exe", str(self.fake) + "\\"):
            with self.subTest(value=value):
                state.write(rtk_path=value)
                res = self.run_bin("rtk", "x")
                self.assertEqual(res.returncode, 127)
                self.assertIn("run /anthology:setup", res.stderr)

    def test_hook_runs_rtk_hook_claude_with_stdin_passed_through(self):
        state.write(rtk_path=str(self.fake))
        res = self.run_bin("rtk-hook", stdin='{"tool_name":"Bash"}')
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(res.stdout.splitlines(), ["args:hook claude", "suppress:1", '{"tool_name":"Bash"}'])

    def test_hook_via_sh_in_exec_form(self):
        state.write(rtk_path=str(self.fake))
        res = subprocess.run(["sh", str(PLUGIN / "bin" / "rtk-hook")], input="{}", stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, universal_newlines=True,
                             env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "CLAUDE_CONFIG_DIR": str(self.cfg)})
        self.assertEqual(res.stdout.splitlines()[0], "args:hook claude")

    def test_hook_missing_exits_0_silently(self):
        res = self.run_bin("rtk-hook", stdin='{"tool_name":"Bash"}')
        self.assertEqual((res.returncode, res.stdout, res.stderr), (0, "", ""))

    def test_hook_unusable_paths_exit_0_silently(self):
        for value in (str(self.tmp / "gone"), "bad\\path", ""):
            state.write(rtk_path=value)
            res = self.run_bin("rtk-hook", stdin="{}")
            self.assertEqual((res.returncode, res.stdout, res.stderr), (0, "", ""), value)


if __name__ == "__main__":
    unittest.main()
