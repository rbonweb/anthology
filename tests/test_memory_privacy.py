"""Tests for scripts/memory_privacy.py: the privacy scan that guards every write into repo memory,
and the one shared secret-pattern list (M9).

Fixtures are assembled at run time: no real home path, email, hostname or token sits in this file.
"""
import importlib.util
import os
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
        for text in (U + "/alice/project", "cd " + H + "/bob", "(" + U + "/alice)", U + "/alice."):
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

    def test_hostname_lookup_failure_never_raises(self):
        with mock.patch.object(memory_privacy.socket, "gethostname", side_effect=OSError("no")):
            self.assertEqual(memory_privacy.privacy_findings("plain text", home=HOME), [])

    def test_ssh_target(self):
        self.assertEqual(kinds("ssh -p 2222 deploy" + "@build-01"), ["ssh-target"])
        for cmd in ("scp f ", "sftp ", "rsync -a x ", "mosh "):
            self.assertIn("ssh-target", kinds(cmd + "bob" + "@buildhost"), cmd)
        self.assertEqual(kinds("deploy" + "@build-01 without a command"), [])

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
        }
        for label, text in cases.items():
            found = memory_privacy.privacy_findings("x\n" + text + "\n", home=HOME, hostname=HOST)
            self.assertIn({"kind": "secret:" + label, "line": 2}, found, label)
            self.assertNotIn(text, repr(found))
        self.assertIn("secret:anthropic-key", kinds("sk" + "-ant-" + "abcdefghij0123456789"))


class ExemptionTest(unittest.TestCase):
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
                "https://example.com/home/index.html", "an .ssh folder"):
            self.assertEqual(kinds(text), [], text)


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
