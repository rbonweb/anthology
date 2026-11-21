"""MP1 oracle for the `privacy.findings` difftest surface (plan MP1, GC-12, GC-16).

    python3 -I port/difftest/oracle/privacy.py --emit-rust > crates/anthology-memory/src/privacy/pyclass.rs

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`. Cases:
- `{"op": "findings", "text": S, "home": S, "hostname": S}`: `memory_privacy.privacy_findings`
  with explicit `home` and `hostname` (so neither side reads its environment). stdout is one
  `<line>\\t<kind>` line per finding, then `describe: <describe(findings)>`.
- `{"op": "patterns"}`: `SECRET_PATTERNS` as `<label>\\t<IGNORECASE 0|1>\\t<pattern>` lines.
- `{"op": "constants"}`: every other pattern and table the scan uses: the module patterns with
  their flags, the `_home_literal` and hostname templates (for a sample value), and the
  exemption tables (labels, path and scheme characters, networks, reserved names, forges,
  generic host names). With `patterns`, a Python change that Rust has not copied is a diff.
- `{"op": "classes"}`: this Python's `re` tables for str patterns: the `\\w`, `\\s` and `\\d`
  classes as code point ranges, the zero of each run of decimal digits, `_sre.unicode_tolower`
  where it is not the identity, and `re._casefix._EXTRA_CASES` (IGNORECASE's equivalences):
  the tables `--emit-rust` writes, so a drift between them and this Python is a diff.
- `{"op": "fold", "chars": [CP, ...]}`: for each `c`, the members of `chars` that
  `re.compile(re.escape(c), re.IGNORECASE)` fully matches: the end-to-end check of the
  translator's IGNORECASE literals (the machine hostname's pattern).

`--emit-rust` prints `crates/anthology-memory/src/privacy/pyclass.rs`, the `classes` tables as
Rust constants (CPython 3.12, the pinned oracle image, is the authority, GC-16).

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import _sre
import importlib.util
import re
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PRIVACY_PY = ROOT / "plugins" / "anthology" / "scripts" / "memory_privacy.py"


def _load_privacy():
    spec = importlib.util.spec_from_file_location("_anthology_oracle_privacy", str(PRIVACY_PY))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


_PRIVACY = _load_privacy()


def _extra_cases():
    try:
        from re import _casefix  # 3.11+
        return _casefix._EXTRA_CASES
    except ImportError:  # 3.9 and 3.10 (the non-gating report)
        import sre_compile
        return sre_compile._ignorecase_fixes


def _scalars():
    return (cp for cp in range(0x110000) if not 0xD800 <= cp < 0xE000)


def _ranges(rx):
    out, start = [], None
    for cp in range(0x110001):
        ok = cp < 0x110000 and not 0xD800 <= cp < 0xE000 and rx.match(chr(cp)) is not None
        if ok and start is None:
            start = cp
        elif not ok and start is not None:
            out.append((start, cp - 1))
            start = None
    return out


def _classes():
    w = _ranges(re.compile(r"\w"))
    s = _ranges(re.compile(r"\s"))
    d = _ranges(re.compile(r"\d"))
    zeros = []
    for lo, hi in d:
        for cp in range(lo, hi + 1):
            if unicodedata.decimal(chr(cp)) == 0:
                zeros.append(cp)
    lower = [(cp, _sre.unicode_tolower(cp)) for cp in _scalars() if _sre.unicode_tolower(cp) != cp]
    extra = sorted((k, tuple(v)) for k, v in _extra_cases().items())
    return w, s, d, zeros, lower, extra


def _ranges_text(name, ranges):
    return "%s %s\n" % (name, " ".join("%x-%x" % r for r in ranges))


def _constants():
    p = _PRIVACY
    out = []
    for name in ("_HOME_PATH", "_EMAIL", "_IPV4", "_IPV6", "_PRIVATE_HOST", "_SSH_COMMAND",
                 "_USER_AT_HOST", "_SSH_PATH", "_KEY_NAME"):
        rx = getattr(p, name)
        out.append("pattern\t%s\t%d\t%s" % (name, 1 if rx.flags & re.IGNORECASE else 0, rx.pattern))
    home = p._home_literal("/srv/x")
    out.append("dynamic\thome\t%d\t%s" % (1 if home.flags & re.IGNORECASE else 0, home.pattern))
    host = p._hostname_patterns("abcd")[0]
    out.append("dynamic\thost\t%d\t%s" % (1 if host.flags & re.IGNORECASE else 0, host.pattern))
    out.append("table\t_LABELS\t%s" % "|".join("%s=%s" % kv for kv in p._LABELS.items()))
    out.append("table\t_PATH_CHARS\t%s" % "".join(sorted(p._PATH_CHARS)))
    out.append("table\t_SCHEME_CHARS\t%s" % "".join(sorted(p._SCHEME_CHARS)))
    for name in ("_PRIVATE_V4", "_EXEMPT_V4", "_PRIVATE_V6"):
        out.append("table\t%s\t%s" % (name, " ".join(str(n) for n in getattr(p, name))))
    for name in ("_RESERVED_DOMAINS", "_RESERVED_TLDS", "_PUBLIC_FORGES"):
        out.append("table\t%s\t%s" % (name, " ".join(getattr(p, name))))
    out.append("table\t_GENERIC_HOSTNAMES\t%s" % " ".join(sorted(p._GENERIC_HOSTNAMES)))
    return "".join(line + "\n" for line in out)


def run(case):
    op = case.get("op")
    if op == "findings":
        found = _PRIVACY.privacy_findings(case["text"], home=case["home"], hostname=case["hostname"])
        out = "".join("%d\t%s\n" % (f["line"], f["kind"]) for f in found)
        out += "describe: %s\n" % _PRIVACY.describe(found)
        return {"exit": 0, "stdout": out, "stderr": ""}
    if op == "patterns":
        out = "".join("%s\t%d\t%s\n" % (label, 1 if rx.flags & re.IGNORECASE else 0, rx.pattern)
                      for label, rx in _PRIVACY.SECRET_PATTERNS)
        return {"exit": 0, "stdout": out, "stderr": ""}
    if op == "constants":
        return {"exit": 0, "stdout": _constants(), "stderr": ""}
    if op == "classes":
        w, s, d, zeros, lower, extra = _classes()
        out = _ranges_text("w", w) + _ranges_text("s", s) + _ranges_text("d", d)
        out += "zeros %s\n" % " ".join("%x" % z for z in zeros)
        out += "lower %s\n" % " ".join("%x:%x" % pair for pair in lower)
        out += "extra %s\n" % " ".join("%x:%s" % (k, ",".join("%x" % x for x in v)) for k, v in extra)
        return {"exit": 0, "stdout": out, "stderr": ""}
    if op == "fold":
        chars = [chr(cp) for cp in case["chars"]]
        lines = []
        for c in chars:
            rx = re.compile(re.escape(c), re.IGNORECASE)
            hits = [d for d in chars if rx.fullmatch(d)]
            lines.append("%x %s\n" % (ord(c), " ".join("%x" % ord(d) for d in hits)))
        return {"exit": 0, "stdout": "".join(lines), "stderr": ""}
    raise ValueError("unknown op %r" % (op,))


def _emit_list(lines, name, doc, items, per_row, ty):
    lines.append("/// %s" % doc)
    lines.append("pub const %s: &[%s] = &[" % (name, ty))
    row = []
    for item in items:
        row.append(item + ",")
        if len(row) == per_row:
            lines.append("    " + " ".join(row))
            row = []
    if row:
        lines.append("    " + " ".join(row))
    lines.append("];")
    lines.append("")


def emit_rust():
    w, s, d, zeros, lower, extra = _classes()
    out = [
        "//! CPython %s's `re` tables for str patterns (Unicode %s), generated by" % (
            sys.version.split()[0], unicodedata.unidata_version),
        "//! `python3 -I port/difftest/oracle/privacy.py --emit-rust` in the pinned Python image.",
        "//! Do not edit by hand. The `privacy.findings` difftest compares them with the oracle",
        "//! (`{\"op\": \"classes\"}`). P1's `pycompat::re::{W, S, D}` replace the classes once P1",
        "//! merges.",
        "",
    ]
    ranges = lambda rs: ["(0x%X, 0x%X)" % r for r in rs]  # noqa: E731
    _emit_list(out, "W", "`\\w`: `str.isalnum()` or `_`.", ranges(w), 5, "(u32, u32)")
    _emit_list(out, "S", "`\\s`: `str.isspace()`.", ranges(s), 5, "(u32, u32)")
    _emit_list(out, "D", "`\\d`: the decimal digits (Nd).", ranges(d), 5, "(u32, u32)")
    _emit_list(out, "DIGIT_ZEROS",
               "The zero of each run of ten decimal digits (Nd comes in runs `0..=9`), ascending.",
               ["0x%X" % z for z in zeros], 8, "u32")
    _emit_list(out, "LOWER",
               "`_sre.unicode_tolower(c)` (the first character of `c.lower()`), where it is not "
               "`c`, by code point.", ["(0x%X, 0x%X)" % pair for pair in lower], 5, "(u32, u32)")
    _emit_list(out, "EXTRA_CASES",
               "`re._casefix._EXTRA_CASES`: under IGNORECASE, a lowercase character also matches "
               "these.", ["(0x%X, &[%s])" % (k, ", ".join("0x%X" % x for x in v)) for k, v in extra],
               2, "(u32, &[u32])")
    while out and out[-1] == "":
        out.pop()
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    if sys.argv[1:] == ["--emit-rust"]:
        emit_rust()
    else:
        sys.stderr.write("usage: python3 -I port/difftest/oracle/privacy.py --emit-rust\n")
        sys.exit(2)
