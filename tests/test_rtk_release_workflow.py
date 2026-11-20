"""Structure of the rtk release workflow, its toolchain pin and its version.

The workflow itself only runs on GitHub; these tests keep its shape from drifting:
every action pinned by SHA, exactly the four target builds, no Windows, the publish
job holding the attestation permissions, and rtk/rust-toolchain.toml matching the
Rust image the tests run in.
"""
from __future__ import annotations

import re
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "rtk-release.yml"
TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
}


def load() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def triggers(wf: dict) -> dict:
    # PyYAML (YAML 1.1) parses the key `on` as the boolean True.
    return wf["on"] if "on" in wf else wf[True]


def steps_of(job: dict) -> list:
    return job.get("steps", [])


class RtkReleaseWorkflowTest(unittest.TestCase):
    def setUp(self):
        self.wf = load()
        self.jobs = self.wf["jobs"]

    def test_every_action_is_pinned_by_sha(self):
        uses = [s["uses"] for job in self.jobs.values() for s in steps_of(job) if "uses" in s]
        self.assertTrue(uses)
        for u in uses:
            self.assertRegex(u, r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$", u)

    def test_targets_are_exactly_the_four(self):
        matrix = self.jobs["build"]["strategy"]["matrix"]["include"]
        self.assertEqual({m["target"] for m in matrix}, TARGETS)
        self.assertEqual(len(matrix), len(TARGETS))
        self.assertIs(self.jobs["build"]["strategy"]["fail-fast"], False)

    def test_no_windows(self):
        self.assertNotIn("windows", WORKFLOW.read_text(encoding="utf-8").lower())
        for m in self.jobs["build"]["strategy"]["matrix"]["include"]:
            self.assertNotIn("windows", m["os"])

    def test_publish_job_permissions_and_attestation(self):
        publish = self.jobs["publish"]
        perms = publish["permissions"]
        self.assertEqual(perms.get("id-token"), "write")
        self.assertEqual(perms.get("attestations"), "write")
        self.assertEqual(perms.get("contents"), "write")
        self.assertTrue(
            any(s.get("uses", "").startswith("actions/attest-build-provenance@") for s in steps_of(publish))
        )

    def test_permissions_are_least_privilege(self):
        self.assertEqual(self.wf["permissions"], {})
        for name, job in self.jobs.items():
            if name != "publish":
                self.assertEqual(job["permissions"], {"contents": "read"}, name)

    def test_triggers(self):
        on = triggers(self.wf)
        self.assertIn("rtk--v*", on["push"]["tags"])
        self.assertTrue(on["workflow_dispatch"]["inputs"]["tag"]["required"])
        self.assertIn("rtk/rust-toolchain.toml", on["pull_request"]["paths"])

    def test_every_job_has_a_timeout(self):
        for name, job in self.jobs.items():
            self.assertIn("timeout-minutes", job, name)

    def test_no_untrusted_expression_in_run_scripts(self):
        for name, job in self.jobs.items():
            for s in steps_of(job):
                self.assertNotIn("${{", s.get("run", ""), f"{name}: {s.get('name')}")

    def test_toolchain_matches_the_rust_image(self):
        toolchain = (ROOT / "rtk" / "rust-toolchain.toml").read_text(encoding="utf-8")
        channel = re.search(r'^channel\s*=\s*"([^"]+)"', toolchain, re.M)
        self.assertIsNotNone(channel)
        self.assertNotIn("targets", toolchain)
        dockerfile = (ROOT / "ci" / "images" / "rust.Dockerfile").read_text(encoding="utf-8")
        image = re.search(r"^FROM\s+rust:(\d+(?:\.\d+)*)", dockerfile, re.M)
        self.assertIsNotNone(image)
        self.assertTrue(
            channel.group(1) == image.group(1) or channel.group(1).startswith(image.group(1) + "."),
            f"toolchain {channel.group(1)} vs image rust:{image.group(1)}",
        )

    def test_version_is_the_anthology_edition(self):
        cargo = (ROOT / "rtk" / "Cargo.toml").read_text(encoding="utf-8")
        version = re.search(r'^version = "([^"]+)"', cargo, re.M).group(1)
        self.assertRegex(version, r"^\d+\.\d+\.\d+-anthology\.\d+$")
        lock = (ROOT / "rtk" / "Cargo.lock").read_text(encoding="utf-8")
        self.assertIn(f'name = "rtk"\nversion = "{version}"\n', lock)


if __name__ == "__main__":
    unittest.main()
