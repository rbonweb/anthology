"""MS1 oracle for the `notes.parse` difftest surface (plan MS1, GC-12).

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`, always `{"exit": 0, "stdout": ..., "stderr": ""}`.
Cases:
- `{"op": "parse", "text": S, "stem": S}`: `memory_index.parse_text(text, stem)`. stdout is
  one JSON line (`json.dumps`, ASCII): `[name, description, type, updated, [supersedes,
  depends_on, relates_to], body, ok, memory_sync.is_user_note(info), normalize_type(type)]`.
- `{"op": "note_info", "name": S, "data_hex": HEX}`: `memory_sync.note_info(name, data)`,
  printed the same way (the bytes need not be UTF-8: `errors="replace"`).
- `{"op": "render", "notes": [[file, text], ...], "links": [[file, target], ...],
  "budget": [lines, bytes], "suffix": S, "exclude": [S, ...]}`: the notes are written (UTF-8)
  into a new temporary folder, with the symlinks; stdout is `render_index(...)` (or `<None>`),
  then `regenerate_index` twice (`regen <first> <second>`), then whether `MEMORY.md` now
  holds the rendered text.
- `{"op": "remote", "url": S, "base": S, "home": S}`: `memory_sync.normalize_remote(url,
  base)` with `HOME` set to `home` while it runs.

A frontmatter nested about a thousand levels deep makes Python's `_parse_block` raise
`RecursionError` (caught by `parse_text`, D-1); Rust parses it (the `seed:deep` case,
declared in `port/difftest/divergences/notes.parse.toml`).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

import memory_index  # noqa: E402
import memory_sync  # noqa: E402

KINDS = ("supersedes", "depends_on", "relates_to")


def _fields(info):
    return [info["name"], info["description"], info["type"], info["updated"],
            [info["relations"].get(k, []) for k in KINDS], info["body"], info["ok"],
            memory_sync.is_user_note(info), memory_sync.normalize_type(info["type"])]


def _out(text):
    return {"exit": 0, "stdout": text, "stderr": ""}


def _render(case):
    d = tempfile.mkdtemp(prefix="notes-oracle-")
    try:
        for name, text in case["notes"]:
            with open(os.path.join(d, name), "wb") as fh:
                fh.write(text.encode("utf-8"))
        for name, target in case.get("links", []):
            os.symlink(target, os.path.join(d, name))
        budget = memory_sync.Budget(*case["budget"])
        exclude = tuple(case["exclude"])
        text = memory_sync.render_index(d, budget, case["suffix"], exclude_types=exclude)
        out = "<None>\n" if text is None else text
        first = memory_sync.regenerate_index(d, budget, exclude)
        second = memory_sync.regenerate_index(d, budget, exclude)
        out += "regen %s %s\n" % (first, second)
        index = os.path.join(d, "MEMORY.md")
        same = (text is not None and not os.path.islink(index) and os.path.isfile(index)
                and Path(index).read_bytes() == text.encode("utf-8"))
        out += "index %s\n" % same
        return out
    finally:
        shutil.rmtree(d, ignore_errors=True)


def _remote(case):
    old = os.environ.get("HOME")
    os.environ["HOME"] = case["home"]
    try:
        return memory_sync.normalize_remote(case["url"], base=case["base"]) + "\n"
    finally:
        if old is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = old


def run(case):
    op = case.get("op")
    if op == "parse":
        info = memory_index.parse_text(case["text"], case["stem"])
        return _out(json.dumps(_fields(info)) + "\n")
    if op == "note_info":
        info = memory_sync.note_info(case["name"], bytes.fromhex(case["data_hex"]))
        return _out(json.dumps(_fields(info)) + "\n")
    if op == "render":
        return _out(_render(case))
    if op == "remote":
        return _out(_remote(case))
    raise KeyError("unknown op %r" % (op,))
