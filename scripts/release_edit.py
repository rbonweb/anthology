#!/usr/bin/env python3
"""Text and JSON helpers for scripts/release.sh (the shell orchestrates git and claude).

Subcommands (every failure prints `release: <reason>` to stderr and exits 1):

  plugin-version PLUGIN_JSON                  print the top-level version
  check-version CURRENT NEW TAG_EXISTS        print `bump` or `same` (TAG_EXISTS: 1 or 0)
  check-dependents ROOT PLUGIN VERSION        every plugin depending on PLUGIN accepts VERSION
  changelog-check CHANGELOG PLUGIN            PLUGIN's `### [Unreleased]` has content
  changelog-move CHANGELOG PLUGIN VERSION [--date YYYY-MM-DD]
  set-version PLUGIN_JSON VERSION             replace only the top-level "version" string
  set-source-version SOURCES_JSON TARGET VERSION
                                              replace only that entry's "plugin_version"

Edits are made in place and are minimal: the rest of each file stays byte-identical.
Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import sys
from pathlib import Path
from typing import List, Optional, Sequence, Tuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from check_version_bump import compare_semver, parse_semver  # noqa: E402

PLUGIN_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")


class ReleaseError(Exception):
    """A precondition failed or a file could not be edited."""


class UnsupportedRange(ReleaseError):
    """A dependency range in a form this tool does not evaluate."""


# ---- version checks ------------------------------------------------------------------

def check_version(current: str, new: str, tag_exists: bool) -> str:
    """`bump` if NEW > CURRENT; `same` if NEW == CURRENT and its tag does not exist yet."""
    if parse_semver(new) is None:
        raise ReleaseError(f"{new!r} is not a valid semver version (expected X.Y.Z)")
    if parse_semver(current) is None:
        raise ReleaseError(f"the current plugin version {current!r} is not valid semver")
    cmp = compare_semver(new, current)
    if cmp > 0:
        return "bump"
    if cmp == 0:
        if tag_exists:
            raise ReleaseError(f"version {new} is already released (its tag exists)")
        return "same"
    raise ReleaseError(f"{new} is lower than the current version {current}")


# ---- dependency ranges (npm semantics) -----------------------------------------------

_RANGE_RE = re.compile(r"^(~|\^|>=)?(.+)$")
_PARTIAL_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


def range_accepts(rng: object, version: str) -> bool:
    """Does `rng` accept `version`? Forms: ~X.Y, ~X.Y.Z, ^X.Y.Z, X.Y.Z, >=X.Y.Z.

    Prerelease versions are accepted only by a range whose base names a prerelease of the
    same X.Y.Z (npm's rule). Anything else raises UnsupportedRange.
    """
    if not isinstance(rng, str) or not rng.strip() or rng != rng.strip():
        raise UnsupportedRange(f"can't check range {rng!r}")
    m = _RANGE_RE.match(rng)
    op, rest = m.group(1) or "", m.group(2)
    pv = parse_semver(version)
    if pv is None:
        raise ReleaseError(f"{version!r} is not a valid semver version")
    partial = _PARTIAL_RE.match(rest)
    if partial and op == "~":
        base = f"{partial.group(1)}.{partial.group(2)}.0"
    elif parse_semver(rest) is not None and "+" not in rest:
        base = rest
    else:
        raise UnsupportedRange(f"can't check range {rng!r}")
    bp = parse_semver(base)
    major, minor, patch = bp[0], bp[1], bp[2]
    if pv[3] and not (bp[3] and pv[:3] == bp[:3]):
        return False
    if op == "":
        return compare_semver(version, base) == 0
    if compare_semver(version, base) < 0:
        return False
    if op == ">=":
        return True
    if op == "~":
        upper = (major, minor + 1, 0)
    elif major > 0:
        upper = (major + 1, 0, 0)
    elif minor > 0:
        upper = (0, minor + 1, 0)
    else:
        upper = (0, 0, patch + 1)
    return pv[:3] < upper


def check_dependents(root, plugin: str, version: str) -> List[str]:
    """Problems (one message each) for plugins whose dependency range excludes `version`."""
    problems = []
    plugins_dir = Path(root) / "plugins"
    for manifest in sorted(plugins_dir.glob("*/.claude-plugin/plugin.json")):
        dependent = manifest.parent.parent.name
        rel = f"plugins/{dependent}/.claude-plugin/plugin.json"
        try:
            data = json.loads(manifest.read_text(encoding="utf-8"))
        except ValueError:
            problems.append(f"{rel} is not valid JSON; can't check its dependencies")
            continue
        deps = data.get("dependencies") if isinstance(data, dict) else None
        for dep in deps if isinstance(deps, list) else []:
            if isinstance(dep, str):
                continue  # a bare name carries no range
            if not isinstance(dep, dict) or dep.get("name") != plugin:
                continue
            rng = dep.get("version")
            if rng is None:
                continue
            try:
                ok = range_accepts(rng, version)
            except UnsupportedRange:
                problems.append(f"{rel} depends on {plugin} with range {rng!r}: "
                                f"can't check range (supported: ~X.Y, ~X.Y.Z, ^X.Y.Z, "
                                f"X.Y.Z, >=X.Y.Z)")
                continue
            if not ok:
                problems.append(f"{rel} depends on {plugin} {rng}, which does not accept "
                                f"{version}; widen that range first (it is an {dependent} "
                                f"change and needs its own version bump)")
    return problems


# ---- CHANGELOG -----------------------------------------------------------------------

UNRELEASED = "### [Unreleased]"


def _plugin_span(lines: List[str], plugin: str) -> Tuple[int, int]:
    start = None
    for i, line in enumerate(lines):
        if line.startswith("## ") and not line.startswith("### "):
            if start is not None:
                return start, i
            if line.strip() == f"## {plugin}":
                start = i
    if start is None:
        raise ReleaseError(f"CHANGELOG.md has no `## {plugin}` section")
    return start, len(lines)


def _unreleased_span(lines: List[str], plugin: str) -> Tuple[int, int, int]:
    """(heading index, first body index, end index exclusive) of the Unreleased section."""
    p_start, p_end = _plugin_span(lines, plugin)
    for i in range(p_start + 1, p_end):
        if lines[i].strip() == UNRELEASED:
            end = p_end
            for j in range(i + 1, p_end):
                if lines[j].startswith("### "):
                    end = j
                    break
            return i, i + 1, end
    raise ReleaseError(f"CHANGELOG.md `## {plugin}` has no `{UNRELEASED}` heading")


def _content(body: List[str]) -> List[str]:
    """Body lines without surrounding blank lines."""
    lo, hi = 0, len(body)
    while lo < hi and not body[lo].strip():
        lo += 1
    while hi > lo and not body[hi - 1].strip():
        hi -= 1
    return body[lo:hi]


def _has_entries(body: List[str]) -> bool:
    # Subheadings alone (`#### Added` with nothing under it) are an empty skeleton.
    return any(line.strip() and not line.startswith("#") for line in body)


def changelog_check(text: str, plugin: str) -> None:
    lines = text.splitlines(keepends=True)
    _h, first, end = _unreleased_span(lines, plugin)
    if not _has_entries(lines[first:end]):
        raise ReleaseError(f"the `{UNRELEASED}` section of {plugin} in CHANGELOG.md is empty; "
                           f"write the release notes there first")


def changelog_move(text: str, plugin: str, version: str, date: str) -> str:
    """Move the plugin's Unreleased content into a new `### [version] - date` section."""
    changelog_check(text, plugin)
    lines = text.splitlines(keepends=True)
    if lines and not lines[-1].endswith("\n"):
        lines[-1] += "\n"
    p_start, p_end = _plugin_span(lines, plugin)
    wanted = f"### [{version}]"
    for line in lines[p_start:p_end]:
        if line.startswith(wanted):
            raise ReleaseError(f"CHANGELOG.md already has a {version} section for {plugin}")
    heading, first, end = _unreleased_span(lines, plugin)
    content = _content(lines[first:end])
    new = [lines[heading], "\n", f"### [{version}] - {date}\n", "\n"] + content
    if end < len(lines):
        new += ["\n"]
    return "".join(lines[:heading] + new + lines[end:])


# ---- minimal JSON edits --------------------------------------------------------------

def _tokens(text: str) -> List[Tuple[str, int, int]]:
    """Structural characters and strings with their [start, end) offsets."""
    out = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            if j >= n:
                raise ReleaseError("unterminated string in JSON")
            out.append(("s", i, j + 1))
            i = j + 1
            continue
        if c in "{}[]:,":
            out.append((c, i, i + 1))
        i += 1
    return out


def _member_value(text: str, toks, open_idx: int, key: str) -> Optional[int]:
    """Token index of the value of `key` directly inside the object opened at `open_idx`."""
    depth = 0
    for k in range(open_idx, len(toks)):
        kind = toks[k][0]
        if kind in "{[":
            depth += 1
        elif kind in "}]":
            depth -= 1
            if depth == 0:
                return None
        elif (kind == "s" and depth == 1 and k + 2 < len(toks) and toks[k + 1][0] == ":"
              and json.loads(text[toks[k][1]:toks[k][2]]) == key):
            return k + 2
    return None


def _replace_string_value(text: str, toks, idx: int, new: str, what: str) -> str:
    kind, start, end = toks[idx]
    if kind != "s":
        raise ReleaseError(f"{what} is not a string")
    return text[:start] + json.dumps(new) + text[end:]


def plugin_version(text: str) -> str:
    try:
        version = json.loads(text).get("version")
    except (ValueError, AttributeError):
        raise ReleaseError("plugin.json is not a valid JSON object")
    if not isinstance(version, str):
        raise ReleaseError("plugin.json has no top-level string \"version\"")
    return version


def set_plugin_version(text: str, version: str) -> str:
    plugin_version(text)  # validates shape
    toks = _tokens(text)
    if not toks or toks[0][0] != "{":
        raise ReleaseError("plugin.json is not a JSON object")
    idx = _member_value(text, toks, 0, "version")
    if idx is None:
        raise ReleaseError("plugin.json has no top-level \"version\"")
    return _replace_string_value(text, toks, idx, version, "plugin.json \"version\"")


def set_source_plugin_version(text: str, target: str, version: str) -> str:
    try:
        data = json.loads(text)
    except ValueError:
        raise ReleaseError("sources.json is not valid JSON")
    toks = _tokens(text)
    sources_val = _member_value(text, toks, 0, "sources")
    if sources_val is None or toks[sources_val][0] != "[":
        raise ReleaseError("sources.json has no \"sources\" array")
    # Walk the array's direct children: each `{` at relative depth 1 starts an entry.
    depth, entries = 0, []
    for k in range(sources_val, len(toks)):
        kind = toks[k][0]
        if kind in "{[":
            if kind == "{" and depth == 1:
                entries.append(k)
            depth += 1
        elif kind in "}]":
            depth -= 1
            if depth == 0:
                break
    if len(entries) != len(data["sources"]):
        raise ReleaseError("could not map sources.json entries")
    for entry, open_idx in zip(data["sources"], entries):
        if entry.get("target") != target:
            continue
        idx = _member_value(text, toks, open_idx, "plugin_version")
        if idx is None:
            raise ReleaseError(f"the sources.json entry for {target} has no plugin_version")
        return _replace_string_value(text, toks, idx, version, "plugin_version")
    raise ReleaseError(f"sources.json has no entry with target {target!r}")


# ---- command line --------------------------------------------------------------------

def _read(path: str) -> str:
    with open(path, encoding="utf-8", newline="") as f:
        return f.read()


def _write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("plugin-version")
    p.add_argument("path")
    p = sub.add_parser("check-version")
    p.add_argument("current")
    p.add_argument("new")
    p.add_argument("tag_exists", choices=["0", "1"])
    p = sub.add_parser("check-dependents")
    p.add_argument("root")
    p.add_argument("plugin")
    p.add_argument("version")
    p = sub.add_parser("changelog-check")
    p.add_argument("path")
    p.add_argument("plugin")
    p = sub.add_parser("changelog-move")
    p.add_argument("path")
    p.add_argument("plugin")
    p.add_argument("version")
    p.add_argument("--date", default="")
    p = sub.add_parser("set-version")
    p.add_argument("path")
    p.add_argument("version")
    p = sub.add_parser("set-source-version")
    p.add_argument("path")
    p.add_argument("target")
    p.add_argument("version")
    a = ap.parse_args(argv)
    try:
        if a.cmd == "plugin-version":
            print(plugin_version(_read(a.path)))
        elif a.cmd == "check-version":
            print(check_version(a.current, a.new, a.tag_exists == "1"))
        elif a.cmd == "check-dependents":
            problems = check_dependents(a.root, a.plugin, a.version)
            if problems:
                for msg in problems:
                    print(f"release: {msg}", file=sys.stderr)
                return 1
        elif a.cmd == "changelog-check":
            changelog_check(_read(a.path), a.plugin)
        elif a.cmd == "changelog-move":
            date = a.date or datetime.date.today().isoformat()
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
                raise ReleaseError(f"date {date!r} is not YYYY-MM-DD")
            _write(a.path, changelog_move(_read(a.path), a.plugin, a.version, date))
        elif a.cmd == "set-version":
            _write(a.path, set_plugin_version(_read(a.path), a.version))
        elif a.cmd == "set-source-version":
            _write(a.path, set_source_plugin_version(_read(a.path), a.target, a.version))
    except ReleaseError as e:
        print(f"release: {e}", file=sys.stderr)
        return 1
    except OSError as e:
        print(f"release: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
