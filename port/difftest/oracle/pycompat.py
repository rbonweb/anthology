"""P1's oracle for the `pycompat.*` difftest surfaces other than json (design §12, §13.2).

F4's `_driver.py` loads this file by path and calls `run(case) -> dict`. A case is
`{"op": "<surface>.<operation>", ...}`; the answer is `{"exit": 0, "stdout": TEXT, "stderr": ""}`
where TEXT is `json.dumps(result, sort_keys=True)`, or `{"exit": 1, "stdout": "", "stderr":
"<exception type name>"}` when Python raises (the message is not compared, unless the op says
so). Each op's docs say what `result` is; the Rust surface builds the same JSON.

Offsets are code points, as Python's are. Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import json
import os
import re
import sys
import unicodedata

_MAX = 0x110000


def _ok(result):
    return {"exit": 0, "stdout": json.dumps(result, sort_keys=True), "stderr": ""}


def _raised(exc):
    return {"exit": 1, "stdout": "", "stderr": type(exc).__name__}


def _points(lo, hi):
    for cp in range(lo, hi):
        if not 0xD800 <= cp <= 0xDFFF:
            yield cp


def _runs(lo, hi, pred):
    out = []
    for cp in _points(lo, hi):
        if pred(chr(cp)):
            if out and out[-1][1] == cp - 1:
                out[-1][1] = cp
            else:
                out.append([cp, cp])
    return out


# --- str ------------------------------------------------------------------------------------

_SIGMA = "Σ"
_FINAL = "ς"

_STR_PREDICATES = {
    "isspace": str.isspace,
    "isdecimal": str.isdecimal,
    "isdigit": str.isdigit,
    "isalpha": str.isalpha,
    "isalnum": str.isalnum,
    "isprintable": str.isprintable,
}


def _str_table(case):
    """`{"lo", "hi"}`: for the code points in `[lo, hi)`, the runs of each predicate, the
    category runs (`[lo, hi, cat]`, Cn included), the decimal values, every `lower()` that is not
    the character itself, and the Final_Sigma probes: the runs of `c` for which
    `(c + "Σ").lower()`, `("A" + c + "Σ").lower()` end in final sigma and `("AΣ" + c).lower()[1]`
    is final."""
    lo, hi = case["lo"], case["hi"]
    out = {name: _runs(lo, hi, pred) for name, pred in _STR_PREDICATES.items()}
    cats = []
    for cp in _points(lo, hi):
        cat = unicodedata.category(chr(cp))
        if cats and cats[-1][2] == cat and cats[-1][1] == cp - 1:
            cats[-1][1] = cp
        else:
            cats.append([cp, cp, cat])
    out["category"] = cats
    out["decimal"] = [[cp, unicodedata.decimal(chr(cp))] for cp in _points(lo, hi)
                      if unicodedata.decimal(chr(cp), None) is not None]
    out["lower"] = [[cp, chr(cp).lower()] for cp in _points(lo, hi) if chr(cp).lower() != chr(cp)]
    out["sigma_after"] = _runs(lo, hi, lambda c: (c + _SIGMA).lower()[-1] == _FINAL)
    out["sigma_skip"] = _runs(lo, hi, lambda c: ("A" + c + _SIGMA).lower()[-1] == _FINAL)
    out["sigma_before"] = _runs(lo, hi, lambda c: ("A" + _SIGMA + c).lower()[1] == _FINAL)
    return out


def _str_float(s):
    v = float(s)
    return repr(v)


_STR_OPS = {
    "str.table": _str_table,
    "str.lower": lambda c: c["s"].lower(),
    "str.split": lambda c: c["s"].split(),
    "str.strip": lambda c: c["s"].strip(),
    "str.repr": lambda c: repr(c["s"]),
    "str.len": lambda c: len(c["s"]),
    "str.slice": lambda c: c["s"][c["a"]:c["b"]],
    "str.int": lambda c: int(c["s"]),
    "str.float": lambda c: _str_float(c["s"]),
}


# --- re -------------------------------------------------------------------------------------

_FLAGS = {"i": re.IGNORECASE, "m": re.MULTILINE, "s": re.DOTALL, "x": re.VERBOSE, "a": re.ASCII}


def _compile(case):
    flags = 0
    for k in case.get("flags", ""):
        flags |= _FLAGS[k]
    return re.compile(case["pattern"], flags)


def _re_finditer(case):
    """`{"pattern", "flags", "text"}`: `[m.regs for m in finditer]`: per match, each group's
    `[start, end]` or `null`, group 0 first."""
    p = _compile(case)
    out = []
    for m in p.finditer(case["text"]):
        out.append([list(m.span(g)) if m.span(g) != (-1, -1) else None
                    for g in range(p.groups + 1)])
    return out


def _re_classes(case):
    """`{"patterns": [...], "flags", "lo", "hi"}`: for each pattern, the runs of single
    characters in `[lo, hi)` it fully matches."""
    out = []
    for pat in case["patterns"]:
        p = _compile({"pattern": pat, "flags": case.get("flags", "")})
        out.append(_runs(case["lo"], case["hi"], lambda c: p.fullmatch(c) is not None))
    return out


_RE_OPS = {
    "re.finditer": _re_finditer,
    "re.classes": _re_classes,
}


# --- pyerr ----------------------------------------------------------------------------------

def _pyerr_oserror(case):
    """`{"errno", "filename"}`: `type(e).__name__ + ": " + str(e)` for
    `OSError(errno, os.strerror(errno)[, filename])` (the class follows the errno)."""
    n = case["errno"]
    f = case.get("filename")
    e = OSError(n, os.strerror(n)) if f is None else OSError(n, os.strerror(n), f)
    return "%s: %s" % (type(e).__name__, e)


def _pyerr_keyerror(case):
    """`{"key"}`: `str(KeyError(key))`, which is `repr(key)`."""
    return str(KeyError(case["key"]))


_PYERR_OPS = {
    "pyerr.oserror": _pyerr_oserror,
    "pyerr.keyerror": _pyerr_keyerror,
}


_OPS = {}
_OPS.update(_STR_OPS)
_OPS.update(_RE_OPS)
_OPS.update(_PYERR_OPS)


def run(case):
    op = _OPS.get(case.get("op"))
    if op is None:
        raise ValueError("pycompat oracle: unknown op %r" % (case.get("op"),))
    try:
        result = op(case)
    except (ValueError, TypeError, KeyError, OverflowError, re.error, OSError) as e:
        return _raised(e)
    return _ok(result)


if __name__ == "__main__":
    sys.exit("pycompat.py is a library for port/difftest/oracle/_driver.py")
