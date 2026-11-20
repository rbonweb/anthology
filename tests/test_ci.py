"""Static checks on the CI configuration: the pinned Claude Code and the workflow rules."""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

import yaml

from _repo import ROOT

CLAUDE_CODE = "@anthropic-ai/claude-code"
MIN_CLAUDE_CODE = (2, 1, 323)  # older versions reject `engines` in plugin.json under --strict
PLUGINS = {"anthology", "anthology-guardrails", "anthology-superpowers"}
WORKFLOWS = sorted((ROOT / ".github" / "workflows").glob("*.yml"))
USES_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$")


def load_workflow(path):
    return yaml.safe_load(Path(path).read_text(encoding="utf-8"))


def steps_of(workflow):
    for job_name, job in workflow.get("jobs", {}).items():
        for step in job.get("steps", []):
            yield job_name, step


class PinnedClaudeCodeTests(unittest.TestCase):
    def setUp(self):
        self.pkg = json.loads((ROOT / "ci/claude-code/package.json").read_text(encoding="utf-8"))
        self.lock = json.loads((ROOT / "ci/claude-code/package-lock.json").read_text(encoding="utf-8"))

    def test_exact_version_at_least_minimum(self):
        pin = self.pkg["dependencies"][CLAUDE_CODE]
        m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", pin)
        self.assertIsNotNone(m, f"{pin!r} must be an exact X.Y.Z, not a range")
        self.assertGreaterEqual(tuple(int(g) for g in m.groups()), MIN_CLAUDE_CODE)

    def test_lockfile_matches_the_pin(self):
        pin = self.pkg["dependencies"][CLAUDE_CODE]
        entry = self.lock["packages"]["node_modules/" + CLAUDE_CODE]
        self.assertEqual(entry["version"], pin)

    def test_package_is_private(self):
        self.assertIs(self.pkg.get("private"), True)


class WorkflowRuleTests(unittest.TestCase):
    def test_there_are_workflows(self):
        self.assertTrue(WORKFLOWS)

    def test_every_uses_is_pinned_by_full_sha(self):
        for path in WORKFLOWS:
            for job, step in steps_of(load_workflow(path)):
                if "uses" in step:
                    self.assertRegex(step["uses"], USES_RE, f"{path.name}:{job}")

    def test_top_level_permissions(self):
        for path in WORKFLOWS:
            self.assertIn("permissions", load_workflow(path), path.name)

    def test_no_expression_inside_run_scripts(self):
        for path in WORKFLOWS:
            for job, step in steps_of(load_workflow(path)):
                if "run" in step:
                    self.assertNotIn("${{", step["run"], f"{path.name}:{job}: pass it via env:")

    def test_checkouts_do_not_persist_credentials(self):
        for path in WORKFLOWS:
            for job, step in steps_of(load_workflow(path)):
                if step.get("uses", "").startswith("actions/checkout@"):
                    self.assertIs(step.get("with", {}).get("persist-credentials"), False,
                                  f"{path.name}:{job}")


class CiJobTests(unittest.TestCase):
    def setUp(self):
        self.jobs = load_workflow(ROOT / ".github/workflows/ci.yml")["jobs"]

    def test_version_bump_job(self):
        self.assertIn("version-bump", self.jobs)
        checkout = self.jobs["version-bump"]["steps"][0]
        self.assertEqual(checkout["with"]["fetch-depth"], 0)

    def test_plugin_validate_matrix_is_exactly_the_plugins(self):
        job = self.jobs["plugin-validate"]
        self.assertEqual(set(job["strategy"]["matrix"]["plugin"]), PLUGINS)
        self.assertEqual(len(job["strategy"]["matrix"]["plugin"]), len(PLUGINS))
        self.assertIs(job["strategy"]["fail-fast"], False)

    def test_gitleaks_job(self):
        self.assertIn("gitleaks", self.jobs)
        self.assertEqual(self.jobs["gitleaks"]["steps"][0]["with"]["fetch-depth"], 0)

    def test_every_plugin_has_a_validate_target(self):
        on_disk = {p.name for p in (ROOT / "plugins").iterdir() if p.is_dir()}
        self.assertEqual(on_disk, PLUGINS)


if __name__ == "__main__":
    unittest.main()
