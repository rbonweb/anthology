"""The rtk subtree is the Apache-2.0 edition, Claude Code only, with no telemetry.

An allowlist: the exact `src/hooks/init` modules, the exact Cargo dependencies of every
section, the `rtk hook` subcommands, and the paths scripts/rtk-trim.sh removes. Anything an
upstream sync adds (a new agent, a new dependency, a re-added file) fails here, and the
answer is to extend scripts/rtk_trim.py, not this list."""
from __future__ import annotations

import os
import re
import tempfile
import unittest

import yaml

from _repo import ROOT

RTK = ROOT / "rtk"
# Build output and OS debris that are never tracked: skipped by the tree allowlists.
UNTRACKED_DEBRIS = {"target", ".DS_Store"}
TOP_LEVEL = [
    ".cargo", ".git-blame-ignore-revs", ".gitattributes", ".gitignore", ".rtk", "CHANGELOG.md",
    "CLAUDE.md", "CONTRIBUTING.md", "Cargo.lock", "Cargo.toml", "DISCLAIMER.md", "LICENSE",
    "README.md", "SECURITY.md", "build.rs", "docs", "hooks", "rust-toolchain.toml", "rustfmt.toml", "src", "tests",
    "text",
]
HOOKS_ENTRIES = ["README.md", "claude", "rtk-awareness-full.md", "rtk-awareness-high.md",
                 "rtk-awareness.md"]
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
_DEP_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
_HEADER = re.compile(r"^\s*\[\[?(.+?)\]\]?\s*(#.*)?$")
_KEY_PART = r"""(?:"[^"]*"|'[^']*'|[A-Za-z0-9_-]+)"""
_KEY = re.compile(r"^\s*(%s)\s*(?:\.\s*%s\s*)*=" % (_KEY_PART, _KEY_PART))


def _dotted(name: str) -> list:
    """The parts of a dotted TOML key or table name, raw (quotes kept), split outside quotes."""
    parts, cur, quote = [], "", None
    for ch in name:
        if quote:
            cur += ch
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            cur += ch
        elif ch == ".":
            parts.append(cur.strip())
            cur = ""
        else:
            cur += ch
    parts.append(cur.strip())
    return parts


def _unquote(part: str) -> str:
    return part[1:-1] if len(part) > 1 and part[0] in "\"'" and part[-1] == part[0] else part


def _depth_change(line: str) -> int:
    """Net `[`/`{` nesting a line opens, outside strings and comments."""
    depth, quote = 0, None
    for ch in line:
        if quote:
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
        elif ch == "#":
            break
        elif ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
    return depth


def cargo_deps(path) -> dict:
    """{section: sorted dependency names} for every dependency section of a Cargo.toml.

    A stdlib line scan (Python 3.9 has no tomllib) that tracks bracket nesting, so only
    top-level lines count. Dependencies are the keys of a `[*dependencies]` table (indented,
    quoted or dotted keys: the first part is the name) and the `foo` of a
    `[*dependencies.foo]` sub-table, filed under its parent section."""
    section, scan, depth, out = None, False, 0, {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            if depth == 0:
                header = _HEADER.match(line)
                if header:
                    parts = _dotted(header.group(1).strip())
                    section, scan = None, False
                    if parts[-1] in _DEP_SECTIONS:
                        section, scan = ".".join(parts), True
                    elif len(parts) > 1 and parts[-2] in _DEP_SECTIONS:
                        out.setdefault(".".join(parts[:-1]), []).append(_unquote(parts[-1]))
                    continue
                if scan:
                    key = _KEY.match(line)
                    if key:
                        out.setdefault(section, []).append(_unquote(key.group(1)))
            depth += _depth_change(line)
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


class CargoParserTest(unittest.TestCase):
    """cargo_deps sees a dependency in every form a manifest can declare it."""

    def deps(self, text: str) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "Cargo.toml")
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            return cargo_deps(path)

    def test_plain_dotted_and_multi_line_values(self):
        self.assertEqual(self.deps(
            '[dependencies]\nanyhow = "1"\nrtk-text = { path = "text" }\nserde.workspace = true\n'
            'windows-sys = { version = "0.61", features = [\n    "A",\n    "B",\n] }\n'),
            {"dependencies": ["anyhow", "rtk-text", "serde", "windows-sys"]})

    def test_sub_table_headers(self):
        self.assertEqual(self.deps(
            '[dependencies]\nanyhow = "1"\n\n[dependencies.foo]\nversion = "1"\nfeatures = ["x"]\n'
            "[target.'cfg(unix)'.dependencies.bar]\nversion = \"2\"\n"
            '[dev-dependencies."baz"]\nversion = "3"\n'),
            {"dependencies": ["anyhow", "foo"], "target.'cfg(unix)'.dependencies": ["bar"],
             "dev-dependencies": ["baz"]})

    def test_indented_and_quoted_keys(self):
        self.assertEqual(self.deps(
            '[build-dependencies]\n  indented = "1"\n"quoted" = "1"\n\'single\'.version = "1"\n'
            "  [ target.'cfg(windows)'.dependencies ]\nwin = \"1\"\n"),
            {"build-dependencies": ["indented", "quoted", "single"],
             "target.'cfg(windows)'.dependencies": ["win"]})

    def test_other_tables_and_nested_arrays_are_not_dependencies(self):
        self.assertEqual(self.deps(
            '[package]\nname = "rtk"\n[package.metadata.deb]\nassets = [\n'
            '    ["target/release/rtk", "usr/bin/", "755"]\n]\n[[bin]]\nname = "x"\n'
            '[dependencies]\nok = "1"\n[features]\nfoo = ["dep:ok"]\n'),
            {"dependencies": ["ok"]})


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


def _has_files(path) -> bool:
    if not path.is_dir():
        return True
    return any(files for _, _, files in os.walk(path))


def entries(directory) -> list:
    """The entries of `directory` git would see: no build output or OS debris, no empty dirs."""
    return sorted(p.name for p in directory.iterdir()
                  if p.name not in UNTRACKED_DEBRIS and _has_files(p))


class TreeAllowlistTest(unittest.TestCase):
    """A new host directory or top-level file (an upstream addition) fails here."""

    def test_top_level_entries(self):
        self.assertEqual(entries(RTK), TOP_LEVEL)

    def test_hooks_entries(self):
        self.assertEqual(entries(RTK / "hooks"), HOOKS_ENTRIES)


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

    def test_sync_strips_the_conflict_comments_from_the_merge_message(self):
        self.assertIn("git commit -q --no-edit --cleanup=strip", SYNC.read_text(encoding="utf-8"))

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
