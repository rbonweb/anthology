#!/usr/bin/env python3
"""List the memory files a pull request changes (PLAN section 13, "Governance for teams").

Usage: memory_diff.py <base> <head>

Lists every file added (A), modified (M), deleted (D), renamed (R), type-changed (T) or
copied (C) under the repo-root `.claude/memory/` and `.claude/agent-memory/` between the
merge base of <base> and <head> and <head>. Memory notes load into everyone's sessions, so
they are reviewed like code; this is a listing for the reviewer, not a gate.

Output: a Markdown section appended to $GITHUB_STEP_SUMMARY when it is set, else printed to
stdout. When the summary file is used, stdout gets only the count line
`memory-diff: N changed memory file(s)`: file names are chosen by the PR author, so they are
never echoed raw (a name could otherwise forge a `::workflow-command::`). In the section
every name sits in a code span with control characters, backticks and pipes neutralised.

Exit codes: 0 listed (with or without changes), 1 git error (bad ref), 2 usage error.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import os
import subprocess
import sys
import unicodedata
from typing import List, Sequence, Tuple

ROOTS = (".claude/memory", ".claude/agent-memory")
MAX_NAME = 200
TITLE = "Memory changes in this PR"
EXPLANATION = ("Memory notes load into everyone's sessions, so they are reviewed like code. "
               "This lists them; it does not judge them.")


def parse_name_status(raw: bytes) -> List[Tuple[str, List[str]]]:
    """Parse `git diff --name-status -z` output into [(status letter, [paths])].

    R and C entries carry two paths (old, new), every other entry one. Never splits on
    newlines: a path may contain one.
    """
    fields = raw.split(b"\0")
    if fields and fields[-1] == b"":
        fields.pop()
    entries: List[Tuple[str, List[str]]] = []
    i = 0
    while i < len(fields):
        status = fields[i].decode("utf-8", "replace")
        letter = status[:1] or "?"
        count = 2 if letter in ("R", "C") else 1
        paths = [f.decode("utf-8", "replace") for f in fields[i + 1:i + 1 + count]]
        entries.append((letter, paths))
        i += 1 + count
    return entries


def escape_name(name: str) -> str:
    """Make an untrusted file name safe inside a Markdown table code span."""
    out = []
    for ch in name[:MAX_NAME]:
        if unicodedata.category(ch)[0] == "C" or unicodedata.category(ch) in ("Zl", "Zp"):
            out.append("?")  # control, format, surrogate, private-use, line/paragraph separators
        elif ch == "`":
            out.append("'")
        elif ch == "|":
            out.append("\\|")
        else:
            out.append(ch)
    return "".join(out)


def render(entries: Sequence[Tuple[str, List[str]]]) -> str:
    lines = [f"### {TITLE}", "", EXPLANATION, ""]
    if not entries:
        lines.append("No memory changes.")
    else:
        lines += ["| status | file |", "| --- | --- |"]
        for letter, paths in entries:
            shown = " → ".join(f"`{escape_name(p)}`" for p in paths)
            lines.append(f"| {escape_name(letter)} | {shown} |")
    return "\n".join(lines) + "\n"


def changed_memory_files(base: str, head: str) -> bytes:
    cmd = ["git", "diff", "--name-status", "-z", "--find-renames", f"{base}...{head}", "--"]
    cmd += [f":(top,literal){root}" for root in ROOTS]
    try:
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError(f"git did not run: {exc.__class__.__name__}")
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", "replace").strip().splitlines()
        raise RuntimeError(escape_name(detail[0]) if detail else f"git exited {proc.returncode}")
    return proc.stdout


def main(argv: Sequence[str]) -> int:
    if len(argv) != 2 or any(a.startswith("-") for a in argv):
        print("usage: memory_diff.py <base> <head>", file=sys.stderr)
        return 2
    try:
        entries = parse_name_status(changed_memory_files(argv[0], argv[1]))
    except RuntimeError as exc:
        print(f"memory-diff: git failed: {exc}", file=sys.stderr)
        return 1
    section = render(entries)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(section + "\n")
    else:
        sys.stdout.write(section)
    print(f"memory-diff: {len(entries)} changed memory file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
