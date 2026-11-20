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

    @staticmethod
    def checkout_of(job):
        return next(s for s in job["steps"] if s.get("uses", "").startswith("actions/checkout@"))

    def test_version_bump_job(self):
        self.assertIn("version-bump", self.jobs)
        job = self.jobs["version-bump"]
        self.assertEqual(self.checkout_of(job)["with"]["fetch-depth"], 0)
        gate = next(s for s in job["steps"] if "check_version_bump.py" in s.get("run", ""))
        self.assertEqual(gate["env"]["BASE"], "${{ github.event.pull_request.base.sha || github.event.before }}")
        self.assertEqual(gate["env"]["HEAD_SHA"], "${{ github.event.pull_request.head.sha || github.sha }}")
        self.assertIn('"$BASE" "$HEAD_SHA"', gate["run"])

    def test_plugin_validate_matrix_is_exactly_the_plugins(self):
        job = self.jobs["plugin-validate"]
        self.assertEqual(set(job["strategy"]["matrix"]["plugin"]), PLUGINS)
        self.assertEqual(len(job["strategy"]["matrix"]["plugin"]), len(PLUGINS))
        self.assertIs(job["strategy"]["fail-fast"], False)

    def test_gitleaks_job(self):
        self.assertIn("gitleaks", self.jobs)
        job = self.jobs["gitleaks"]
        self.assertEqual(self.checkout_of(job)["with"]["fetch-depth"], 0)
        scan = next(s for s in job["steps"] if "docker run" in s.get("run", ""))
        self.assertIn('-v "$PWD":/repo:ro', scan["run"])  # the scan only reads the checkout

    def test_new_jobs_have_a_timeout(self):
        for name in ("version-bump", "plugin-validate", "gitleaks"):
            self.assertEqual(self.jobs[name].get("timeout-minutes"), 15, name)

    def test_every_plugin_has_a_validate_target(self):
        on_disk = {p.name for p in (ROOT / "plugins").iterdir() if p.is_dir()}
        self.assertEqual(on_disk, PLUGINS)


class UpstreamWatchWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.wf = load_workflow(ROOT / ".github/workflows/upstream-watch.yml")

    def test_least_privilege_permissions(self):
        self.assertEqual(self.wf["permissions"], {})
        jobs = self.wf["jobs"]
        self.assertEqual(jobs["watch"]["permissions"], {"contents": "read", "issues": "write"})
        self.assertEqual(jobs["trigger-evals"]["permissions"], {"contents": "read", "actions": "write"})

    def test_weekly_schedule_and_manual_run(self):
        triggers = self.wf.get("on", self.wf.get(True))  # YAML 1.1 reads a bare `on` as True
        self.assertEqual(len(triggers["schedule"]), 1)
        self.assertIn("workflow_dispatch", triggers)

    def test_jobs_have_a_timeout(self):
        for name, job in self.wf["jobs"].items():
            self.assertEqual(job.get("timeout-minutes"), 15, name)

    def test_evals_are_dispatched_on_the_default_branch(self):
        step = next(s for s in self.wf["jobs"]["trigger-evals"]["steps"] if "gh workflow run" in s.get("run", ""))
        self.assertEqual(step["env"]["REF"], "${{ github.event.repository.default_branch }}")

    def test_evals_trigger_follows_the_watch_output(self):
        job = self.wf["jobs"]["trigger-evals"]
        self.assertEqual(job["needs"], "watch")
        self.assertIn("needs.watch.outputs.claude_code_changed == 'true'", job["if"])
        self.assertIn("claude_code_changed", self.wf["jobs"]["watch"]["outputs"])


class DependabotTests(unittest.TestCase):
    def test_claude_code_updates_wait_three_days(self):
        config = yaml.safe_load((ROOT / ".github/dependabot.yml").read_text(encoding="utf-8"))
        entry = next(u for u in config["updates"] if u["package-ecosystem"] == "npm")
        self.assertEqual(entry["directory"], "/ci/claude-code")
        self.assertEqual(entry["cooldown"], {"default-days": 3})


if __name__ == "__main__":
    unittest.main()
