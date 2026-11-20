#!/usr/bin/env python3
"""Version-bump gate: a plugin whose files changed must carry a higher version.

Usage: check_version_bump.py <base> <head> [--repo PATH]

`base` and `head` are commits (in CI: the pull-request base and head SHAs, or the push's
before/after). The comparison is made against `git merge-base <base> <head>`, so the base
branch advancing with its own bumps after the branch point does not change the verdict.

Exit codes: 0 pass (or nothing to check), 1 a plugin changed without a valid bump,
2 the refs could not be resolved or share no history.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from typing import List, Optional, Sequence, Tuple

GATE_PATH = "scripts/check_version_bump.py"
PLUGINS_DIR = "plugins"

_NUM = r"(0|[1-9][0-9]*)"
_PRE_ID = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
_BUILD_ID = r"[0-9A-Za-z-]+"
SEMVER_RE = re.compile(
    rf"^{_NUM}\.{_NUM}\.{_NUM}(?:-({_PRE_ID}(?:\.{_PRE_ID})*))?(?:\+({_BUILD_ID}(?:\.{_BUILD_ID})*))?"
)


# ---- pure logic (no git) -------------------------------------------------------------

def parse_semver(version) -> Optional[tuple]:
    """Return (major, minor, patch, prerelease identifiers) or None if not valid semver."""
    if not isinstance(version, str):
        return None
    m = SEMVER_RE.fullmatch(version)
    if not m:
        return None
    pre = m.group(4).split(".") if m.group(4) else []
    return (int(m.group(1)), int(m.group(2)), int(m.group(3)), pre)


def _compare_prerelease(a: List[str], b: List[str]) -> int:
    if not a and not b:
        return 0
    if not a:  # a release outranks any prerelease of the same version
        return 1
    if not b:
        return -1
    for x, y in zip(a, b):
        if x == y:
            continue
        x_num, y_num = x.isdigit(), y.isdigit()
        if x_num and y_num:
            return (int(x) > int(y)) - (int(x) < int(y))
        if x_num:
            return -1
        if y_num:
            return 1
        return (x > y) - (x < y)
    return (len(a) > len(b)) - (len(a) < len(b))


def compare_semver(a: str, b: str) -> int:
    """-1, 0 or 1 by semver precedence (build metadata ignored). Both must be valid."""
    pa, pb = parse_semver(a), parse_semver(b)
    if pa is None or pb is None:
        raise ValueError(f"not valid semver: {a!r} / {b!r}")
    for x, y in zip(pa[:3], pb[:3]):
        if x != y:
            return (x > y) - (x < y)
    return _compare_prerelease(pa[3], pb[3])


def changed_plugins(paths: Sequence[str]) -> List[str]:
    """Sorted plugin names with at least one changed path under plugins/<name>/."""
    names = set()
    prefix = PLUGINS_DIR + "/"
    for path in paths:
        if not path.startswith(prefix):
            continue
        name, sep, _rest = path[len(prefix):].partition("/")
        if name and sep:
            names.add(name)
    return sorted(names)


def verdict(plugin: str, base_version, head_version, base_exists: bool = True,
            head_exists: bool = True) -> Tuple[bool, str]:
    """(ok, reason) for one changed plugin."""
    if not base_exists:
        return True, f"plugins/{plugin} is new"
    if not head_exists:
        return True, f"plugins/{plugin} was removed"
    if parse_semver(head_version) is None:
        return False, f"plugins/{plugin} version {head_version!r} is not valid semver"
    if parse_semver(base_version) is None:
        # A broken base version must not lock the plugin out of fixing it.
        return True, f"plugins/{plugin} {head_version} (base version {base_version!r} was invalid)"
    cmp = compare_semver(head_version, base_version)
    if cmp == 0:
        return False, f"plugins/{plugin} changed without a version bump (still {base_version})"
    if cmp < 0:
        return False, (f"plugins/{plugin} version went down "
                       f"({base_version} -> {head_version})")
    return True, f"plugins/{plugin} {base_version} -> {head_version}"


# ---- git layer -----------------------------------------------------------------------

class GateError(Exception):
    """Refs could not be resolved (exit 2)."""


def _git(repo: str, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", repo, *args], stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, universal_newlines=True)


def _git_ok(repo: str, *args: str) -> str:
    r = _git(repo, *args)
    if r.returncode != 0:
        raise GateError(f"git {' '.join(args)} failed: {r.stderr.strip()}")
    return r.stdout


def _resolve(repo: str, ref: str) -> str:
    r = _git(repo, "rev-parse", "--verify", "--quiet", ref + "^{commit}")
    if r.returncode != 0:
        raise GateError(f"cannot resolve {ref!r} to a commit (is the checkout deep enough?)")
    return r.stdout.strip()


def _plugin_version_at(repo: str, rev: str, plugin: str) -> Tuple[bool, Optional[str]]:
    """(manifest exists at rev, version or None if unreadable)."""
    r = _git(repo, "show", f"{rev}:{PLUGINS_DIR}/{plugin}/.claude-plugin/plugin.json")
    if r.returncode != 0:
        return False, None
    try:
        version = json.loads(r.stdout).get("version")
    except (ValueError, AttributeError):
        return True, None
    return True, version


def run(base: str, head: str, repo: str) -> int:
    if re.fullmatch("0+", base):
        print("version gate: no base commit (new branch); nothing to compare")
        return 0
    base_c, head_c = _resolve(repo, base), _resolve(repo, head)
    mb = _git(repo, "merge-base", base_c, head_c)
    if mb.returncode != 0 or not mb.stdout.strip():
        raise GateError(f"no merge base between {base} and {head}")
    mb_c = mb.stdout.strip()
    if _git(repo, "cat-file", "-e", f"{mb_c}:{GATE_PATH}").returncode != 0:
        print("version gate: base predates the gate; skipping")
        return 0
    # -z: NUL-separated and never quoted (core.quotePath would hide non-ASCII paths from the filter)
    diff = _git_ok(repo, "-c", "core.quotePath=false", "diff", "--name-only", "-z", "--no-renames",
                   mb_c, head_c)
    paths = [p for p in diff.split("\0") if p]
    plugins = changed_plugins(paths)
    if not plugins:
        print("version gate: no plugin changed")
        return 0
    failed = False
    for plugin in plugins:
        base_exists, base_version = _plugin_version_at(repo, mb_c, plugin)
        head_exists, head_version = _plugin_version_at(repo, head_c, plugin)
        ok, reason = verdict(plugin, base_version, head_version, base_exists, head_exists)
        print(f"{'ok  ' if ok else 'FAIL'} {reason}")
        failed = failed or not ok
    return 1 if failed else 0


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("base", help="base commit (all zeros = new branch, no-op)")
    ap.add_argument("head", help="head commit")
    ap.add_argument("--repo", default=".", help="repository root (default: current directory)")
    args = ap.parse_args(argv)
    try:
        return run(args.base, args.head, args.repo)
    except GateError as e:
        print(f"version gate: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
