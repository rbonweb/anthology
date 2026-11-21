"""GD oracle for the `guard.check` and `guard.hook` difftest surfaces (plan GD1, GD2; design §9.5, §13.2).

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the standard
library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict` (the case's `data`, or a dict holding it under "data").

Case data. The rules are guard_rules.py's; the shell analysis under them is the shell oracle's
(`port/difftest/oracle/shell.py`: the vendored fix-wave H/I reference `guard_rules_fw.py` with the Rust-stricter
rules applied, lane SH), because Rust's guard runs on `anthology_shell::analyze`. The alternate readings
are checked against G3, G4, G1 and G2 after every entry of every reading, and against G6 with the entries,
as Rust's guard checks `Analysis.alternates` (final review C-1, I-3; review-gd-final I-1); this holds on
every path, a stopped analysis and the hook included. `guard.py` (`plugins/anthology-guardrails/scripts/`) runs on that module as well:
- `{"op": "check", "tool": T, "input": I, "home": H}`: `guard_rules.check(T, I, H)`, rendered as the
  canonical JSON of `[rule, message]` or `null`. `check` turns any exception into `None`; this module
  calls `_check` itself, so an exception is seen: it answers `null` with `"divergence": "D-1"` (Python
  failed open; Rust may block or match).
- `{"op": "hook", "stdin_hex": X, "home": H|null}`: `guard.py`'s `main()` with the bytes of `X` on stdin
  and `HOME=H` (unset when null), in plain mode. The result is its exit code, stdout and stderr.

Result: `{"exit": E, "stdout": S, "stderr": R}`; for "check", `E` is 0, `S` the canonical JSON
(`json.dumps(obj, sort_keys=True, separators=(",", ":"))`, Rust: `pycompat::json::dumps(.., &COMPACT_SORTED)`)
and `R` empty.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import io
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SCRIPTS = os.path.join(ROOT, "plugins", "anthology-guardrails", "scripts")
GUARD = os.path.join(SCRIPTS, "guard.py")


def _load():
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "shell.py")
    spec = importlib.util.spec_from_file_location("anthology_oracle_shell_gd", path)
    shell = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shell)
    sys.modules["guard_rules"] = shell.G                  # what guard.py imports
    return shell


SHELL = _load()
G = SHELL.G

# Rust's guard reports a rule an analyzed entry hits before it reports an unfinished analysis (crates/
# anthology-guard/src/check.rs): when the analysis stops (`_TooComplex`), the entries collected so far are
# checked first, and only when none hits is it G0. The contexts of one check are kept for that.
_CONTEXTS = []


class _Context(G._Context):
    def __init__(self, home):
        super().__init__(home)
        _CONTEXTS.append(self)


G._Context = _Context


def _stopped_rule(name, inp, home, command):
    """`_check_reading` over the entries collected before the analysis stopped, unbudgeted."""
    entries = [e for ctx in _CONTEXTS for e in ctx.entries]
    G._STATE.reset(None)
    if G._g6_file_tool(inp, home) or (command and G._g6_command(command, entries, home)):
        return "G6"
    grep = name == "Grep"
    skip = G._PLAIN_TEXT_KEYS | {"pattern"} if grep else G._PLAIN_TEXT_KEYS
    paths = list(G._path_strings(inp, skip)) + G._search_roots(
        inp, ("glob",) if grep else ("pattern", "glob"))
    if G._words_hit(paths, G._is_profile_path, home, None) or G._code_hits_profile(
            G._code_strings(inp), home, None):
        return "G3"
    return _entries_rule(entries + list(SHELL._ALTERNATES), home) or "G0"


def _entries_rule(entries, home):
    """The first of G3, G4, G1, G2 that an entry hits, entry by entry (Rust's per-entry loop), or None."""
    for entry in entries:
        for rule, test in (("G3", lambda e: G._g3(e, home)), ("G4", G._g4), ("G1", G._g1), ("G2", G._g2)):
            if test(entry):
                return rule
    return None


_G6_COMMAND = G._g6_command


def _g6_command(command, entries, home):
    """`_g6_command` over the entries and the alternate readings collected so far: Rust's G6 reads
    `Analysis.alternates` too (review-gd-final I-1). Every caller (`_check_reading`, `_stopped_rule`)
    goes through this."""
    return _G6_COMMAND(command, list(entries) + list(SHELL._ALTERNATES), home)


G._g6_command = _g6_command


_CHECK = G._check


def _check_alternates(tool_name, tool_input, home):
    """`_check` over every reading, then G3, G4, G1, G2 on each alternate reading: Rust checks
    `Analysis.alternates` after the entries of all its passes (crates/anthology-guard/src/check.rs)."""
    rule = _CHECK(tool_name, tool_input, home)
    if rule is None:
        rule = _entries_rule(list(SHELL._ALTERNATES), home if isinstance(home, str) else "")
    return rule


G._check = _check_alternates


def _rule(tool, tool_input, home):
    """`check`'s rule (None, a rule id, or "G0"); other exceptions propagate."""
    name = tool if isinstance(tool, str) else ""
    inp = tool_input if isinstance(tool_input, dict) else {}
    home = home if isinstance(home, str) else ""
    command = inp.get("command") if isinstance(inp.get("command"), str) else None
    G._STATE.reset(G._WORK_PER_CHAR * (len(command) if command else 0) + G._WORK_BASE)
    del SHELL._ALTERNATES[:]
    del _CONTEXTS[:]
    try:
        return G._check(tool, tool_input, home)
    except G._TooComplex:
        return _stopped_rule(name, inp, home, command)
    finally:
        G._STATE.reset(None)
        del _CONTEXTS[:]


def _guard_check(tool_name, tool_input, home):
    """`check`, as guard.py calls it, with Rust's order on a stopped analysis."""
    try:
        rule = _rule(tool_name, tool_input, home)
    except Exception:  # a guard bug must never brick a session
        return None
    if rule == "G0":
        return "G0", G.TOO_COMPLEX
    return (rule, G.MESSAGES[rule]) if rule else None


G.check = _guard_check


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


def _check(tool, tool_input, home):
    try:
        rule = _rule(tool, tool_input, home)
    except Exception as exc:  # check() would return None (allow): Rust may block or match (D-1)
        return {"exit": 0, "stdout": canonical(None), "stderr": "", "divergence": "D-1",
                "python": type(exc).__name__}
    if rule == "G0":
        result = ["G0", G.TOO_COMPLEX]
    else:
        result = [rule, G.MESSAGES[rule]] if rule else None
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def _load_guard():
    spec = importlib.util.spec_from_file_location("anthology_oracle_guard_py_gd", GUARD)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_GUARD = None


def _hook(stdin, home):
    """guard.py `main()` in this process (a child process per case is too slow for 10^4 cases): its
    stdin, stdout, stderr and `HOME` are swapped in for the call and restored after."""
    global _GUARD
    if _GUARD is None:
        _GUARD = _load_guard()
    saved = (sys.stdin, sys.stdout, sys.stderr, os.environ.get("HOME"))
    out, err = io.BytesIO(), io.BytesIO()
    sys.stdin = io.TextIOWrapper(io.BytesIO(stdin), encoding="utf-8")
    sys.stdout = io.TextIOWrapper(out, encoding="utf-8", write_through=True)
    sys.stderr = io.TextIOWrapper(err, encoding="utf-8", write_through=True)
    if home is None:
        os.environ.pop("HOME", None)
    else:
        os.environ["HOME"] = home
    try:
        code = _GUARD.main()
    finally:
        sys.stdout.flush()
        sys.stderr.flush()
        # read before the wrappers go: dropping one closes the buffer under it
        written = out.getvalue(), err.getvalue()
        sys.stdin, sys.stdout, sys.stderr = saved[:3]
        if saved[3] is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = saved[3]
    answer = {"exit": code, "stdout_hex": written[0].hex(), "stderr_hex": written[1].hex()}
    if _raises(stdin, home):
        answer["divergence"] = "D-1"  # guard.py allowed because Python raised; Rust may block or match
    return answer


def _raises(stdin, home):
    """Whether guard.py's `decide` raises on this input (RecursionError on deep JSON, for one)."""
    try:
        data = json.loads(stdin.decode("utf-8", errors="replace"))
    except json.JSONDecodeError:
        return False  # a malformed envelope is guard.py's defined allow, not a failure (GC-6)
    except Exception:
        return True  # RecursionError on deep JSON: Rust parses it and checks (F-10)
    try:
        if isinstance(data, dict) and isinstance(data.get("tool_name"), str):
            _rule(data["tool_name"], data.get("tool_input"), home or os.path.expanduser("~"))
    except Exception:
        return True
    return False


def run(case):
    data = case["data"] if "data" in case and "op" not in case else case
    op = data["op"]
    if op == "check":
        return _check(data["tool"], data["input"], data["home"])
    if op == "hook":
        return _hook(bytes.fromhex(data["stdin_hex"]), data.get("home"))
    raise ValueError("guard oracle: unknown op %r" % (op,))


if __name__ == "__main__":
    print("guard oracle: loaded by port/difftest/oracle/_driver.py", file=sys.stderr)
    sys.exit(2)
