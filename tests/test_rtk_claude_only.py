"""The rtk subtree is the Apache-2.0 edition, Claude Code only, with no telemetry.

An allowlist: the exact `src/hooks/init` modules, the exact Cargo dependencies of every
section, the `rtk hook` subcommands, and the paths scripts/rtk-trim.sh removes. Anything an
upstream sync adds (a new agent, a new dependency, a re-added file) fails here, and the
answer is to extend scripts/rtk_trim.py, not this list."""
from __future__ import annotations

import os
import re
import unittest

import yaml

from _repo import ROOT

RTK = ROOT / "rtk"
TRIM_SH = ROOT / "scripts" / "rtk-trim.sh"
TRIM_PY = ROOT / "scripts" / "rtk_trim.py"
NET_TEST = ROOT / "scripts" / "rtk-net-test.sh"
SYNC = ROOT / "scripts" / "sync-rtk.sh"

INIT_MODULES = ["agents_md.rs", "claude.rs", "mod.rs"]

CARGO_DEPS = {
    "dependencies": [
        "anyhow", "base16ct", "chrono", "clap", "codepage", "colored", "crossterm", "dirs",
        "encoding_rs", "flate2", "ignore", "oem_cp", "quick-xml", "ratatui", "regex", "rtk-text",
        "rusqlite", "serde", "serde_json", "sha2", "tempfile", "toml", "toml_edit", "ureq",
        "walkdir", "which",
    ],
    "target.'cfg(unix)'.dependencies": ["libc"],
    "target.'cfg(windows)'.dependencies": ["windows-sys"],
    "target.'cfg(target_env = \"musl\")'.dependencies": ["mimalloc"],
    "build-dependencies": ["toml"],
    "dev-dependencies": ["temp-env"],
}
TEXT_CARGO_DEPS = {"dependencies": ["regex"]}
CARGO_MANIFESTS = ["Cargo.toml", "text/Cargo.toml"]
CHECK_CFG = ("unexpected_cfgs = { level = \"deny\", "
             "check-cfg = ['cfg(feature, values(\"engine\", \"example-plugins\"))'] }")

HOOK_COMMANDS = {"Claude", "Check"}

FORK_TESTS = [
    "fork_1_auto_allow_hook.rs",
    "fork_2_read_minimal_default.rs",
    "fork_3_node_filter.rs",
    "fork_4_python_filter.rs",
    "fork_5_cd_chain_rewrite.rs",
    "fork_6_biome_lint.rs",
    "fork_7_opencode_run.rs",
    "fork_9_awareness_exact.rs",
]

_OTHER_INIT = ("antigravity", "codex", "copilot", "cursor", "droid", "gemini", "hermes",
               "instructions_agents", "opencode", "pi", "trae", "vibe")
_OTHER_HOOKS = ("antigravity", "cline", "codex", "copilot", "cursor", "hermes", "kilocode",
                "opencode", "pi", "vibe", "windsurf")
_REMOVED_TESTS = ("agent_coverage_doc_test", "antigravity_hook_test", "copilot_repo_hook_test",
                  "copilot_selfheal_test", "fork_8_upstream_headers",
                  "hook_detection_multi_agent_test", "init_agent_cursor_test",
                  "init_codex_uninstall_test", "init_opencode_test", "omp_init_test",
                  "trae_hook_test", "hook_config_io_test", "init_uninstall_exit_code_test")
REMOVED_PATHS = [
    # BUSL-1.1 crates and what only they serve
    "engine", "plugin", "proxy", "examples",
    "src/example_plugins.rs",
    "src/core/call_context.rs", "src/core/classify.rs", "src/core/pipeline_cmd.rs",
    "src/core/pipeline_config.rs", "src/core/pipeline_log.rs", "src/core/pipeline_remote.rs",
    "src/core/plugins_cmd.rs", "src/core/task_cmd.rs",
    "src/hooks/posthook.rs", "src/hooks/session_start.rs", "src/hooks/hook_cmd",
    # telemetry
    "src/core/telemetry.rs", "src/core/telemetry_cmd.rs",
    "docs/TELEMETRY.md", "docs/guide/resources/telemetry.md",
    # other agents
    *["src/hooks/init/%s.rs" % m for m in _OTHER_INIT],
    *["hooks/" + h for h in _OTHER_HOOKS],
    # packaging, translations, upstream tooling
    "README_es.md", "README_fr.md", "README_ja.md", "README_ko.md", "README_pt.md",
    "README_zh.md", "Formula", "install.sh", "INSTALL.md", ".release-please-manifest.json",
    "release-please-config.json", ".claude", ".github", ".semgrep.yml", "openclaw", "scripts",
    # docs of removed features
    "docs/guide/pipelines", "docs/guide/proxy", "docs/guide/resources/agent-coverage.md",
    "docs/guide/resources/editions.md", "docs/guide/getting-started/supported-agents.md",
    # tests of removed code
    *["tests/%s.rs" % t for t in _REMOVED_TESTS],
]

_SECTION = re.compile(r"^\[([^\]]+)\]\s*(#.*)?$")
_DEP = re.compile(r"^([A-Za-z0-9_-]+)(\.[A-Za-z0-9_-]+)?\s*=")


def cargo_deps(path) -> dict:
    """{section: sorted dependency names} for every `*dependencies` section, by a line scan:
    a header is `[...]` at column 0; a dependency is `name =` or `name.workspace =` at
    column 0. Continuation lines of multi-line inline tables are indented or start with
    `]`, so they never match."""
    section, out = None, {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = _SECTION.match(line)
            if m:
                section = m.group(1).strip()
                continue
            if section and section.endswith("dependencies"):
                m = _DEP.match(line)
                if m:
                    out.setdefault(section, []).append(m.group(1))
    return {k: sorted(v) for k, v in out.items()}


def section_lines(path, name: str) -> list:
    """The non-blank, non-comment lines of the TOML section `[name]`."""
    lines, inside = [], False
    for line in path.read_text(encoding="utf-8").splitlines():
        m = _SECTION.match(line)
        if m:
            inside = m.group(1).strip() == name
            continue
        if inside and line.strip() and not line.lstrip().startswith("#"):
            lines.append(line)
    return lines


class InitModulesTest(unittest.TestCase):
    def test_only_claude_and_its_helpers(self):
        init = RTK / "src" / "hooks" / "init"
        self.assertEqual(sorted(p.name for p in init.glob("*.rs")), INIT_MODULES)
        self.assertEqual([p.name for p in init.iterdir() if p.is_dir()], [])


class CargoDependenciesTest(unittest.TestCase):
    def test_cli_dependencies_per_section(self):
        self.assertEqual(cargo_deps(RTK / "Cargo.toml"), CARGO_DEPS)

    def test_text_crate_dependencies(self):
        self.assertEqual(cargo_deps(RTK / "text" / "Cargo.toml"), TEXT_CARGO_DEPS)

    def test_workspace_is_the_cli_and_the_text_crate(self):
        self.assertEqual(section_lines(RTK / "Cargo.toml", "workspace"), ['members = [".", "text"]'])

    def test_no_features(self):
        self.assertEqual(section_lines(RTK / "Cargo.toml", "features"), [])

    def test_only_two_manifests(self):
        found = []
        for dirpath, dirnames, filenames in os.walk(RTK):
            rel = os.path.relpath(dirpath, RTK)
            if rel == ".":
                dirnames[:] = [d for d in dirnames if d not in ("target", ".git")]
            if "Cargo.toml" in filenames:
                found.append(os.path.normpath(os.path.join(rel, "Cargo.toml")).replace(os.sep, "/"))
        self.assertEqual(sorted(found), CARGO_MANIFESTS)

    def test_dead_engine_cfg_is_declared_and_dead_code_allowed(self):
        lints = section_lines(RTK / "Cargo.toml", "lints.rust")
        self.assertIn(CHECK_CFG, lints)
        self.assertIn('dead_code = "allow"', lints)
        self.assertIn('warnings = "deny"', lints)

    def test_no_busl_license_in_the_tree(self):
        for path in (RTK / "Cargo.toml", RTK / "text" / "Cargo.toml"):
            self.assertNotRegex(path.read_text(encoding="utf-8"), r'(?m)^license\s*=\s*"BUSL')


class NoTelemetryTest(unittest.TestCase):
    def test_no_telemetry_sources(self):
        self.assertEqual(sorted(str(p) for p in (RTK / "src").rglob("telemetry*")), [])

    def test_no_telemetry_module_command_or_ping(self):
        self.assertNotRegex((RTK / "src" / "core" / "mod.rs").read_text(encoding="utf-8"),
                            r"\bmod telemetry")
        lib = (RTK / "src" / "lib.rs").read_text(encoding="utf-8")
        self.assertNotIn("Telemetry {", lib)
        self.assertNotIn("telemetry_cmd", lib)
        self.assertNotIn("maybe_ping", lib)
        self.assertNotIn("prompt_telemetry_consent",
                         (RTK / "src" / "hooks" / "init" / "mod.rs").read_text(encoding="utf-8"))


class RemovedPathsTest(unittest.TestCase):
    def test_none_of_the_removed_paths_exist(self):
        present = [p for p in REMOVED_PATHS if os.path.lexists(RTK / p)]
        self.assertEqual(present, [])


class ClaudeOnlyCliTest(unittest.TestCase):
    def setUp(self):
        self.lib = (RTK / "src" / "lib.rs").read_text(encoding="utf-8")

    def test_hook_subcommands_are_claude_and_check(self):
        m = re.search(r"(?ms)^(?:pub )?enum HookCommands \{\n(.*?)^\}", self.lib)
        self.assertIsNotNone(m, "enum HookCommands not found in rtk/src/lib.rs")
        variants = set(re.findall(r"(?m)^    ([A-Z]\w*)\b", m.group(1)))
        self.assertEqual(variants, HOOK_COMMANDS)

    def test_no_agent_target(self):
        self.assertNotRegex(self.lib, r"\benum AgentTarget\b")
        self.assertNotIn("AgentTarget", self.lib)

    def test_fork_tests_are_the_eight_kept(self):
        tests = RTK / "tests"
        self.assertEqual(sorted(p.name for p in tests.glob("fork_*.rs")), FORK_TESTS)


class TrimScriptsTest(unittest.TestCase):
    def test_trim_entry_is_executable_strict_and_runs_the_python_trim(self):
        text = TRIM_SH.read_text(encoding="utf-8")
        self.assertTrue(os.access(TRIM_SH, os.X_OK))
        self.assertTrue(text.startswith("#!"))
        self.assertRegex(text, r"(?m)^set -eu$")
        self.assertIn("scripts/rtk_trim.py", text)
        self.assertTrue(TRIM_PY.is_file())

    def test_trim_prunes_the_lockfile_offline_in_the_pinned_image(self):
        text = TRIM_SH.read_text(encoding="utf-8")
        self.assertIn('awk \'$1 == "FROM" {print $NF; exit}\' "$root/ci/images/rust.Dockerfile"', text)
        self.assertIn("cargo metadata", text)
        self.assertIn("--offline", text)
        self.assertIn("--format-version 1", text)
        self.assertIn("anthology-cargo:/usr/local/cargo/registry", text)
        self.assertIn("--lock-guard", text)
        self.assertNotIn("generate-lockfile", text)
        self.assertNotRegex(text, r"rust:[0-9.]+-slim")

    def test_sync_runs_the_trim_unconditionally(self):
        text = SYNC.read_text(encoding="utf-8")
        self.assertIn("if bash scripts/rtk-trim.sh; then", text)
        self.assertNotIn("trim skipped", text)
        self.assertNotIn("if [ -f scripts/rtk-trim.sh ]", text)

    def test_network_test_runs_the_binary_with_the_network_off(self):
        text = NET_TEST.read_text(encoding="utf-8")
        self.assertTrue(os.access(NET_TEST, os.X_OK))
        self.assertRegex(text, r"(?m)^set -eu$")
        self.assertIn("--network none", text)
        self.assertIn("cargo build --release --locked", text)
        self.assertIn('awk \'$1 == "FROM" {print $NF; exit}\' "$root/ci/images/rust.Dockerfile"', text)
        self.assertNotRegex(text, r"rust:[0-9.]+-slim")

    def test_ci_runs_the_network_test_after_the_rtk_tests(self):
        jobs = yaml.safe_load((ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8"))["jobs"]
        runs = [s.get("run", "") for s in jobs["rtk-test"]["steps"]]
        self.assertIn("scripts/rtk-test.sh", runs)
        self.assertIn("scripts/rtk-net-test.sh", runs)
        self.assertLess(runs.index("scripts/rtk-test.sh"), runs.index("scripts/rtk-net-test.sh"))


if __name__ == "__main__":
    unittest.main()
