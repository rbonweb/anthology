"""SH oracle for the `shell.segments` and `shell.analyze` difftest surfaces (plan SH1, SH2; design §7.1, §13.2).

    python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl > results.jsonl

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the standard
library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict` (the case's `data`, or a dict holding it under "data").

Case data. The reference is `guard_rules_fw.py` beside this file: guard_rules.py with fix-waves H and I
merged, vendored read-only (its header names both commits; controller ruling Q1=A). The owner moved those
fix-waves to Rust, so the plugin's own guard_rules.py is no longer the shell oracle.
- `{"op": "segments", "text": T, "depth": D}`: `_segments(T, D)`, reported with `lenient` (the top-level
  `_Lexer` raised `_Unbalanced`, so `_lenient_segments` produced the segments) and `incomplete` (false).
- `{"op": "shell_parse", "prog": P, "args": [..], "i": I}`: `_shell_parse(P, args, I)`.
- `{"op": "script_starts", "words": [..]}`: `_script_starts(words)`.
- `{"op": "analyze", "command": C, "home": H}`: `_analyze(C, H)` as `check` runs it: the bash 5 reading,
  then the bash 3.2 and wide readings when a heredoc line inside `$(...)` made them matter (`_check`), every
  pass's entries in order. Every `_Entry` field (`Entry` minus `opaque`), and `alternates`: each alternate
  reading `_add_alternates` looks at whose command differs from its entry's, as an entry (Rust keeps them all
  for G1, lead ruling Q3=A; the reference checks G1 as it goes, which this oracle replaces by recording).

Rust is stricter than the reference where a reviewer's bash-verified payload needs it (owner direction, D-8).
Where such a rule reaches across the grammar, so that no case list could name the cases, this oracle applies
it to the reference too, so the difftest still checks everything else; each such rule is listed in
`port/difftest/divergences/shell.*.toml` and pinned by Rust tests:
- FW-H review R1-M1: the lenient split also reads each closed `$'...'` decoded (`_lenient_segments` below).

Both ops run under `check`'s work budget (`_WORK_PER_CHAR` per character plus `_WORK_BASE`); over it the
reference raises `_TooComplex` and `check` blocks. The expected result is then Rust's incomplete one (`incomplete`
true, nothing else), marked D-7: Rust spends the same budget where `_charge` does, so it must stop too.

Groups are identified by object identity in Python; a result numbers them by first appearance (segments in
order, entries in order) and lists each group's texts in that order.

Result: `{"exit": 0, "stdout": <canonical JSON of the result>, "stderr": ""}`, canonical meaning
`json.dumps(obj, sort_keys=True, separators=(",", ":"))` (Rust: `pycompat::json::dumps(.., &COMPACT_SORTED)`).
When Python raises (RecursionError on hostile nesting), the expected result is the incomplete one, marked
D-1: guard.py turns any exception into "allow", and Rust blocks instead (GC-16 D-1).

`--batch`: JSON lines `{"id": ID, "data": DATA}` on stdin, JSON lines `{"id": ID, "out": run(DATA)}` on stdout
(canonical form), in input order.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
GUARD_RULES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "guard_rules_fw.py")


def _load():
    spec = importlib.util.spec_from_file_location("anthology_oracle_guard_rules", GUARD_RULES)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


G = _load()


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


class _Groups:
    """Ordinal of each group object, by first appearance; keeps the objects alive while numbering."""

    def __init__(self):
        self.ids = {}
        self.objects = []

    def ordinal(self, group):
        key = id(group)
        if key not in self.ids:
            self.ids[key] = len(self.objects)
            self.objects.append(group)
        return self.ids[key]

    def texts(self):
        return [list(group.texts) for group in self.objects]


def _budget(text):
    """`check`'s work budget for a command of this length."""
    G._STATE.reset(G._WORK_PER_CHAR * len(text) + G._WORK_BASE)


def _segments(text, depth):
    _budget(text)
    G._charge(len(text) + 1)                              # as `_segments` does
    try:
        segs, lenient = G._Lexer(text, depth).run(), False
    except G._Unbalanced:
        segs, lenient = G._lenient_segments(text), True
    groups = _Groups()
    out = [{"tokens": list(seg.tokens), "redirs": [list(r) for r in seg.redirs],
            "group": groups.ordinal(seg.group)} for seg in segs]
    return {"lenient": lenient, "incomplete": False, "segments": out, "groups": groups.texts()}


def _shell_parse(prog, args, i):
    mode, start, scripts = G._shell_parse(prog, list(args), i)
    return {"mode": mode, "start": start, "scripts": [list(s) for s in scripts]}


def _script_starts(words):
    return {"starts": [list(s) for s in G._script_starts(list(words))]}


_ALTERNATES = []


def _add_alternates(alternates, scripts, entry, ctx, exempt):
    """`_add_alternates`, recording every alternate whose command differs from the entry's instead of
    checking G1 (the scripts are joined as the reference joins them, from every alternate)."""
    main = G._strip_prefixes(entry.raw)[0]
    for alternate in alternates:
        cmd, more = G._strip_prefixes(alternate, layer_scripts=False)
        scripts.extend(script for script in more if script not in scripts)
        if cmd != main:
            other = G._Entry(entry.raw, [], cmd, entry.cwd, exempt, entry.group)
            other.exempt = exempt or other.prog in G._DOCKER
            _ALTERNATES.append(other)


G._add_alternates = _add_alternates


def _decode_ansi_c(piece):
    """`piece` with each closed `$'...'` decoded (bash 5) and the other quotes dropped, or None."""
    out, last, start, found = [], 0, 0, False
    while True:
        at = piece.find("$'", start)
        if at < 0:
            break
        got = G._ansi_c(piece, at + 2)
        if got is None:
            break
        decoded, end = got
        out += [G._LENIENT_QUOTES.sub("", piece[last:at]), decoded]
        last = start = end
        found = True
    if not found:
        return None
    out.append(G._LENIENT_QUOTES.sub("", piece[last:]))
    return "".join(out)


def _lenient_segments(text):
    """`_lenient_segments` with Rust's third reading (FW-H review R1-M1, D-8)."""
    out = []
    G._charge(2 * len(text))
    for piece in G.re.split(r"[;&|\n()`]+|\$\(", text):
        spaced = piece.replace("'", " ").replace('"', " ").split()
        joined = G._LENIENT_QUOTES.sub("", piece).split()
        readings = [spaced, joined] if joined != spaced else [spaced]
        decoded = _decode_ansi_c(piece)
        if decoded is not None and decoded.split() not in readings:
            readings.append(decoded.split())
        for tokens in readings:
            if not tokens:
                continue
            seg = G._Seg(G._Group())
            seg.tokens = tokens
            out.append(seg)
            for start, first in G._script_starts(tokens):   # `bash -c <rest of the line>`
                G._charge(len(tokens) - start)
                extra = G._Seg(G._Group())
                extra.tokens = [first] + tokens[start + 1:]
                out.append(extra)
    return out


G._lenient_segments = _lenient_segments


def _entry(e, groups):
    return {
        "raw": list(e.raw), "redirs": [list(r) for r in e.redirs], "prog_path": e.prog_path,
        "prog": e.prog, "args": list(e.args), "cwd": e.cwd, "exempt": e.exempt,
        "group": groups.ordinal(e.group), "code": list(e.code),
    }


def _analyze(command, home):
    _budget(command)
    del _ALTERNATES[:]
    entries = G._analyze(command, home) if command else []
    for reading in ("exact", "wide"):                     # `_check`, without its early exit on a rule
        if not (command and G._STATE.paren_lines):
            break
        G._STATE.reading = reading
        entries += G._analyze(command, home)
    groups = _Groups()
    out = [_entry(e, groups) for e in entries]
    alternates = [_entry(e, groups) for e in _ALTERNATES]
    return {"incomplete": False, "entries": out, "alternates": alternates, "groups": groups.texts()}


def _call(data):
    """The function and its arguments for a case (a malformed case is a harness bug: it raises here)."""
    op = data["op"]
    if op == "segments":
        return _segments, (data["text"], data.get("depth", 0))
    if op == "shell_parse":
        return _shell_parse, (data["prog"], data["args"], data.get("i", 0))
    if op == "script_starts":
        return _script_starts, (data["words"],)
    if op == "analyze":
        return _analyze, (data["command"], data["home"])
    raise ValueError("shell oracle: unknown op %r" % (op,))


def run(case):
    data = case["data"] if "data" in case and "op" not in case else case
    function, args = _call(data)
    try:
        result = function(*args)
    except G._TooComplex:  # over the work budget: `check` blocks ("too complex")
        return dict(_incomplete(data["op"]), divergence="D-7")
    except Exception:  # RecursionError and anything else guard.py would turn into "allow"
        return dict(_incomplete(data["op"]), divergence="D-1")
    finally:
        G._STATE.reset(None)
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def _incomplete(op):
    """Rust's result for a command it stopped on (the guard blocks it)."""
    if op == "segments":
        result = {"lenient": False, "incomplete": True, "segments": [], "groups": []}
    else:
        result = {"incomplete": True, "entries": [], "alternates": [], "groups": []}
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def main(argv):
    if argv != ["--batch"]:
        print("usage: python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl", file=sys.stderr)
        return 2
    out = sys.stdout
    for line in sys.stdin:
        if not line.strip():
            continue
        case = json.loads(line)
        out.write(canonical({"id": case["id"], "out": run(case["data"])}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
