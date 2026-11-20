"""Tests for scripts/memory_privacy.py: the privacy scan that guards every write into repo memory,
and the one shared secret-pattern list (M9).

Fixtures are assembled at run time: no real home path, email, hostname or token sits in this file.
"""
import importlib.util
import os
import time
import unittest
from unittest import mock

from _mem import ROOT

import memory_privacy  # noqa: E402  (path set up by _mem)
import memory_sync  # noqa: E402

U = "/" + "Users"
H = "/" + "home"
HOST = "alice-" + "laptop"
HOME = U + "/zed"


def kinds(text, home=HOME, hostname=HOST):
    return sorted({f["kind"] for f in memory_privacy.privacy_findings(text, home=home, hostname=hostname)})


def at(*parts):
    return "@".join(parts)


class FindingShapeTest(unittest.TestCase):
    def test_labels_and_line_numbers_only_sorted_and_unique(self):
        mail = at("alice", "mail.acme.zz")
        text = "clean line\n%s/alice/notes and %s/bob/x\nwrite to %s\n%s/carol\n" % (U, U, mail, H)
        found = memory_privacy.privacy_findings(text, home=HOME, hostname=HOST)
        self.assertEqual(found, [{"kind": "home-path", "line": 2}, {"kind": "email", "line": 3},
                                 {"kind": "home-path", "line": 4}])
        for f in found:
            self.assertEqual(sorted(f), ["kind", "line"])
        dumped = repr(found)
        for secret in ("alice", "bob", "carol", "acme", U):
            self.assertNotIn(secret, dumped)

    def test_clean_text_has_no_findings(self):
        text = ("# Build\nRun `make test` in Docker. See [[project-ci]].\n"
                "Version 1.2.3 ships on 2026-04-05; the API listens on port 8080.\n")
        self.assertEqual(memory_privacy.privacy_findings(text, home=HOME, hostname=HOST), [])

    def test_describe(self):
        found = [{"kind": "home-path", "line": 3}, {"kind": "email", "line": 7},
                 {"kind": "home-path", "line": 9}, {"kind": "secret:jwt", "line": 9}]
        self.assertEqual(memory_privacy.describe(found),
                         "home path (lines 3, 9), email address (line 7), secret-looking jwt (line 9)")
        self.assertEqual(memory_privacy.describe([]), "")

    def test_reexported_from_memory_sync(self):
        self.assertIs(memory_sync.privacy_findings, memory_privacy.privacy_findings)

    def test_memory_privacy_imports_no_anthology_module(self):
        source = (ROOT / "plugins" / "anthology" / "scripts" / "memory_privacy.py").read_text()
        for mod in ("memory_sync", "memory_index", "memory_portable", "anthology_setup", "state"):
            self.assertNotRegex(source, r"(?m)^\s*(import|from)\s+%s\b" % mod)


class KindTest(unittest.TestCase):
    def test_home_path(self):
        for text in (U + "/alice/project", "cd " + H + "/bob", "(" + U + "/alice)", U + "/alice.",
                     "/mnt/c" + U + "/alice/proj", "/var" + H + "/alice/src",
                     "/System/Volumes/Data" + U + "/alice/x", "file://" + U + "/alice/x",
                     "C:" + U + "/alice/x", "/mnt/c" + U + "/alice",
                     "/api" + H + "/feed",  # an accepted false positive on the safe side
                     "1://h" + U + "/alice/x", "+.-://h" + U + "/alice/x",  # no scheme letter
                     "PATH=/opt" + H + "/carol/bin"):
            self.assertEqual(kinds(text), ["home-path"], text)

    def test_literal_home_value(self):
        home = "/" + "srv" + "/people/zed"
        self.assertEqual(kinds("cache in " + home + "/.cache", home=home), ["home-path"])
        self.assertEqual(kinds("cache in " + home + "s/.cache", home=home), [])
        self.assertEqual(kinds("just / here", home="/"), [])  # a 1-char home is ignored

    def test_home_defaults_to_the_environment(self):
        home = "/" + "srv" + "/people/zed"
        with mock.patch.dict(os.environ, {"HOME": home}):
            found = memory_privacy.privacy_findings("in " + home + "/x", hostname=HOST)
        self.assertEqual([f["kind"] for f in found], ["home-path"])

    def test_email(self):
        self.assertEqual(kinds("mail " + at("alice", "mail.acme.zz")), ["email"])

    def test_private_ip(self):
        for ip in ("10.1.2.3", "172.16.0.9", "172.31.255.1", "192.168.1.20", "100.64.3.4",
                   "169.254.10.10", "fd12:3456:789a::1", "fe80::1%en0"):
            self.assertEqual(kinds("host at " + ip), ["private-ip"], ip)

    def test_ip_address(self):
        for ip in ("8.8.4.4", "172.32.0.1", "1.2.3.4"):
            self.assertEqual(kinds("server " + ip + "."), ["ip-address"], ip)

    def test_private_hostname(self):
        for host in ("nas.lan", "build-box.local", "db.internal", "wiki.corp", "router.home.arpa",
                     "pi.tail1234.ts.net", "box.localdomain", "printer.home", "intra.intranet"):
            self.assertEqual(kinds("open http://" + host + ":8080/ now"), ["private-hostname"], host)

    def test_machine_hostname(self):
        self.assertEqual(kinds("built on " + HOST.upper() + " yesterday"), ["private-hostname"])
        self.assertEqual(kinds("built on " + HOST, hostname=HOST + ".local"), ["private-hostname"])
        self.assertEqual(kinds("built on " + HOST + "-2"), [])
        self.assertEqual(kinds("a box called abc", hostname="abc"), [])  # under 4 chars
        self.assertEqual(kinds("on localhost", hostname="localhost"), [])

    def test_generic_machine_names_are_never_a_finding(self):
        for name in ("ubuntu", "raspberrypi", "MacBook-Pro", "docker", "workstation", "laptop",
                     "debian.local"):
            self.assertEqual(kinds("runs on " + name.split(".")[0] + " today", hostname=name), [], name)

    def test_hostname_lookup_failure_never_raises(self):
        with mock.patch.object(memory_privacy.socket, "gethostname", side_effect=OSError("no")):
            self.assertEqual(memory_privacy.privacy_findings("plain text", home=HOME), [])

    def test_ssh_target(self):
        self.assertEqual(kinds("ssh -p 2222 deploy" + "@build-01"), ["ssh-target"])
        for cmd in ("scp f ", "sftp ", "rsync -a x ", "mosh "):
            self.assertIn("ssh-target", kinds(cmd + "bob" + "@buildhost"), cmd)
        self.assertEqual(kinds("deploy" + "@build-01 via ssh"), ["ssh-target"])  # target first
        self.assertEqual(kinds("deploy" + "@build-01 without a command"), [])

    def test_git_user_is_exempt_only_on_public_forges(self):
        for forge in ("github.com", "gitlab.com", "bitbucket.org", "codeberg.org",
                      "ssh.dev.azure.com", "git.sr.ht"):
            self.assertEqual(kinds("ssh -T " + at("git", forge)), [], forge)
            self.assertEqual(kinds("remote " + at("git", forge) + ":acme/app.git"), [], forge)
        self.assertEqual(kinds("remote " + at("git", "forge.acme.zz") + ":a/b.git"), ["email"])
        self.assertEqual(kinds("ssh " + at("git", "build-box")), ["ssh-target"])

    def test_ssh_key_path(self):
        for text in ("~/.ssh/id_work", "$HOME/.ssh/config", "/" + "etc" + "/ssh-keys/.ssh/k",
                     "the id_ed25519 key", "id_rsa.pub", "copy id_ecdsa", "an id_dsa file"):
            self.assertEqual(kinds(text), ["ssh-key-path"], text)

    def test_secrets(self):
        cases = {
            "aws-access-key": "AK" + "IA" + "ABCDEFGHIJKLMNOP",
            "github-token": "gh" + "p_" + "a1" * 20,
            "slack-token": "xo" + "xb-" + "1234567890-abc",
            "private-key": "-----BEGIN " + "OPENSSH PRIVATE KEY-----",
            "jwt": "eyJ" + "hbGciOiJIUzI1NiJ9" + ".eyJ" + "zdWIiOiIxMjM0In0" + ".c2lnbmF0dXJlLXg",
            "google-api-key": "AI" + "za" + "C" * 35,
            "stripe-live-key": "sk" + "_live_" + "abcdefghij0123",
            "bearer-token": "Bear" + "er " + "abcdefghij0123456789xyz",
            "basic-auth": "Authorization: Bas" + "ic " + "dXNlcjpwYXNzd29yZA==",
            "gitlab-token": "gl" + "pat-" + "abcdefghij0123456789",
            "huggingface-token": "hf" + "_" + "abcdefghij" * 3 + "xy",
            "npm-token": "np" + "m_" + "abcdefghij" * 3 + "abcdef",
        }
        for label, text in cases.items():
            found = memory_privacy.privacy_findings("x\n" + text + "\n", home=HOME, hostname=HOST)
            self.assertIn({"kind": "secret:" + label, "line": 2}, found, label)
            self.assertNotIn(text, repr(found))
        self.assertIn("secret:anthropic-key", kinds("sk" + "-ant-" + "abcdefghij0123456789"))

    def test_cloud_secret_key_assignment_and_url_credentials(self):
        # m7: an AWS-style secret key assignment and `scheme://user:pw@host` were not caught
        cases = {
            "secret:cloud-secret-key": ("aws_secret_" + "access_key = " + "wJalrXUtnFEMI" + "K7MDENGbPxRfiCY",
                                        "AWS_SECRET_" + "ACCESS_KEY=" + "wJalrXUtnFEMI" + "K7MDENG",
                                        "secret-" + "access-key: " + "abcdefgh12345678",
                                        "access_" + "key = " + "abcdefgh12345678",
                                        '"aws_secret_' + 'access_key": "' + "wJalrXUtnFEMI" + 'K7MDENG"',
                                        '"Secret' + 'AccessKey": "' + "wJalrXUtnFEMI" + 'K7MDENG"',
                                        "'secret_" + "key': '" + "abcdefgh12345678'"),
            "secret:url-credentials": ("postgres" + "://app:" + "Sup3rS3cr" + "etPw@db.acme.zz/prod",
                                       "see https" + "://bob:" + "hunter22xx" + "@host.acme.zz/x",
                                       "redis" + "://:" + "pw12345678" + "@cache.acme.zz"),
        }
        for kind, texts in cases.items():
            for text in texts:
                found = memory_privacy.privacy_findings("x\n" + text + "\n", home=HOME, hostname=HOST)
                self.assertIn({"kind": kind, "line": 2}, found, text)
                self.assertNotIn(text, repr(found))


class QuotedKeyScalingTest(unittest.TestCase):
    def test_quoted_secret_key_scan_scales_linearly(self):
        # 4x the input must cost well under 10x (the quadratic cost); the limit is 6x
        def cost(k):
            text = ('secret_key" ' + "a" * 20 + " ") * k
            best = None
            for _ in range(2):
                start = time.monotonic()
                memory_privacy.privacy_findings(text, home="/" + "srv/zed", hostname=HOST)
                took = time.monotonic() - start
                best = took if best is None else min(best, took)
            return best

        small, large = cost(10000), cost(40000)
        self.assertLess(large, max(6 * small, 0.2), "%.3fs then %.3fs" % (small, large))


class ExemptionTest(unittest.TestCase):
    def test_home_segments_are_case_sensitive(self):
        # macOS spells it /Users/ and Linux /home/: API routes such as /users/42 are not homes
        for text in ("GET /api/v1/users/42", "GET /users/42", "/" + "USERS/alice/x",
                     "/" + "Home/alice/x", "see /api/v1/users/alice/settings"):
            self.assertEqual(kinds(text), [], text)

    def test_nothing_is_flagged(self):
        for text in (
                U + "/Shared/build", U + "/<name>/project", H + "/<user>/x", "$HOME/project",
                at("noreply", "acme.zz"), at("no-reply", "acme.zz"),
                at("1234+alice", "users.noreply.github.com"), at("alice", "example.com"),
                at("bob", "mail.example.org"), at("carol", "example.net"), at("dan", "host.test"),
                at("eve", "box.invalid"), at("frank", "site.example"), at("gail", "box.localhost"),
                "git remote: " + at("git", "github.com") + ":acme/app.git",
                "ssh -T " + at("git", "github.com"), "ssh " + at("alice", "build.example.com"),
                "127.0.0.1", "127.8.9.10", "0.0.0.0", "255.255.255.255", "192.0.2.1",
                "198.51.100.7", "203.0.113.200", "v1.2.3.4", "1.2.3.4.5", "999.1.2.3",
                "2001:db8::1", "at 12:30:45 today", "settings.local.json", "Path.home() call",
                "https://example.com/home/index.html", "an .ssh folder", "src/home/view.py",
                "see https://example.com" + U + "/alice/profile",
                "x1a://h" + U + "/alice", "svn+ssh://h" + U + "/alice",
                "https://example.com:8443/path", "see https://example.com:443/a@b", "git+ssh://host.test/repo",
                "the access key is rotated monthly", "secret key handling", "key = short",
                "https://example.com/x?next=http://example.org/y"):
            self.assertEqual(kinds(text), [], text)


class HostileInputTest(unittest.TestCase):
    """Import scans untrusted archives: one huge line must never take quadratic time."""

    BOUND = 1.0  # seconds per line; a linear scan takes a few tens of milliseconds here

    def assertFast(self, text, label):
        start = time.monotonic()
        memory_privacy.privacy_findings(text, home="/" + "srv/zed", hostname=HOST)
        self.assertLess(time.monotonic() - start, self.BOUND, label)

    def test_long_tokens_before_every_kind(self):
        n = 100000
        tails = {
            "home path": " " + U + "/x",
            "email": " " + at("alice", "mail.acme.zz"),
            "private ip": " 10.1.2.3",
            "hostname": " nas.lan " + HOST,
            "ssh": " ssh " + at("bob", "buildhost"),
            "key path": " ~/.ssh/id_rsa",
            "secret": " " + "gh" + "p_" + "a1" * 20,
        }
        for prefix in ("a" * n, "a/" * (n // 2), "-" * n, "a." * (n // 2), "a@" * (n // 2)):
            for label, tail in tails.items():
                self.assertFast(prefix + tail, "%s after %r..." % (label, prefix[:4]))

    def test_repeated_structures(self):
        n = 100000
        cases = {
            "late scheme then many homes": "/" + "a" * n + "https://h" + (U + "/x") * 1000,
            "many homes in a relative run": "a" + (U + "/x") * (n // 8),
            "many file urls": ("x:///" + U[1:] + "/x ") * (n // 14),
            "schemes in one token": "a://" * (n // 4) + U + "/x",
            "jwt starts after dashes": "eyJ-" * (n // 4),
            "jwt segments": "eyJ" + "a" * n + "." + "b" * 8,
            "labels": ("a" * 70 + ".") * (n // 71),
            "domain dots": "a@" + "b." * (n // 2) + "_",
            "hex colons": "fe80:" * (n // 5),
            "url credentials, no at sign": "a://b:" * (n // 6),
            "url user then long password": "a://b:" + "c" * n,
            "secret key assignments": "secret_key" * (n // 10),
            "quoted secret keys": 'secret_key"' * (n // 11),
        }
        for label, text in cases.items():
            self.assertFast(text, label)

    def test_many_url_tokens_on_one_line(self):
        # each token's end is found without reading to the end of the line
        # Asserted on scaling, not wall time, so a fast or slow host cannot hide or fake a regression:
        # 4x the tokens costs ~4x when linear and 10x+ when quadratic (round 2 code); the limit is 6x.
        def cost(k):
            text = ("a://h" + U + "/x" + sep) * k
            best = None
            for _ in range(2):
                start = time.monotonic()
                self.assertEqual(memory_privacy.privacy_findings(text, home="/" + "srv/zed", hostname=HOST), [])
                took = time.monotonic() - start
                best = took if best is None else min(best, took)
            return best

        for sep in (" ", "\t"):
            small, large = cost(40000), cost(160000)
            self.assertLess(large, max(6 * small, 0.2), "%r: %.3fs then %.3fs" % (sep, small, large))


def _load_guard_rules():
    path = ROOT / "plugins" / "anthology-guardrails" / "scripts" / "guard_rules.py"
    spec = importlib.util.spec_from_file_location("_guard_rules_for_privacy_test", str(path))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SharedSecretPatternsTest(unittest.TestCase):
    def test_every_guard_pattern_is_in_the_shared_list(self):
        """The guard (G6) can never block something the scan misses."""
        shared = {(rx.pattern, rx.flags) for _label, rx in memory_privacy.SECRET_PATTERNS}
        guard = _load_guard_rules()._SECRET_PATTERNS
        self.assertTrue(guard)
        for rx in guard:
            self.assertIn((rx.pattern, rx.flags), shared, rx.pattern)

    def test_shape(self):
        for label, rx in memory_privacy.SECRET_PATTERNS:
            self.assertRegex(label, r"^[a-z][a-z0-9-]*$")
            self.assertTrue(hasattr(rx, "search"))

    def test_memory_sync_defines_no_private_pattern_list(self):
        self.assertFalse(hasattr(memory_sync, "_SECRET_PATTERNS"))


if __name__ == "__main__":
    unittest.main()
