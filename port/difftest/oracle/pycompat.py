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
import warnings

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
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")  # FutureWarning on nested sets and the like
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


# --- difflib, fnmatch, quote, path, ipaddress, shlex ----------------------------------------

def _difflib_unified(case):
    """`{"a", "b", "fromfile", "tofile", "n", "lineterm"}`: `list(difflib.unified_diff(...))`."""
    import difflib
    return list(difflib.unified_diff(case["a"], case["b"], case["fromfile"], case["tofile"],
                                     n=case["n"], lineterm=case.get("lineterm", "")))


def _fnmatch_case(case):
    """`{"name", "pat"}`: `fnmatch.fnmatchcase(name, pat)`."""
    import fnmatch
    return fnmatch.fnmatchcase(case["name"], case["pat"])


def _quote(case):
    """`{"s", "safe"}`: `urllib.parse.quote(s, safe=safe)`."""
    import urllib.parse
    return urllib.parse.quote(case["s"], safe=case["safe"])


def _unquote(case):
    """`{"s"}`: `urllib.parse.unquote(s)`."""
    import urllib.parse
    return urllib.parse.unquote(case["s"])


def _build_tree(root, tree):
    """`tree`: `[[relpath, "dir" | "file" | "link", target], ...]`, in order."""
    for rel, kind, target in tree:
        path = os.path.join(root, rel)
        if kind == "dir":
            os.makedirs(path, exist_ok=True)
        elif kind == "file":
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w"):
                pass
        else:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            os.symlink(target, path)


def _path_realpath(case):
    """`{"tree", "p"}`: `os.path.realpath(<root>/p)` where `<root>` is
    `<tmp>/n1/n2/n3/n4/n5/n6/root` in a fresh temporary folder `<tmp>`, holding `tree` (see
    `_build_tree`); the realpath of `<tmp>` is written `<TMP>`, so a link or `..` that leads out
    of the root (up to six levels) still gives the same text on every machine."""
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        tmp = os.path.realpath(d)
        root = os.path.join(tmp, "n1/n2/n3/n4/n5/n6/root")
        os.makedirs(root)
        _build_tree(root, case["tree"])
        out = os.path.realpath(os.path.join(root, case["p"]))
        if out == tmp or out.startswith(tmp + "/"):
            out = "<TMP>" + out[len(tmp):]
        return out


def _ipaddress_v4(case):
    """`{"s"}`: `list(ipaddress.IPv4Address(s).packed)`."""
    import ipaddress
    return list(ipaddress.IPv4Address(case["s"]).packed)


def _ipaddress_v6(case):
    """`{"s"}`: the eight groups of `ipaddress.IPv6Address(s)`, and whether it is in fc00::/7
    and in fe80::/10."""
    import ipaddress
    a = ipaddress.IPv6Address(case["s"])
    b = a.packed
    groups = [b[i] * 256 + b[i + 1] for i in range(0, 16, 2)]
    return {"groups": groups,
            "unique_local": a in ipaddress.ip_network("fc00::/7"),
            "link_local": a in ipaddress.ip_network("fe80::/10")}


def _shlex_split(case):
    """`{"s"}`: `["ok", shlex.split(s)]` or `["error", str(ValueError)]`."""
    import shlex
    try:
        return ["ok", shlex.split(case["s"])]
    except ValueError as e:
        return ["error", str(e)]


def _shlex_quote(case):
    """`{"s"}`: `shlex.quote(s)`."""
    import shlex
    return shlex.quote(case["s"])


_B_OPS = {
    "difflib.unified": _difflib_unified,
    "fnmatch.case": _fnmatch_case,
    "quote.quote": _quote,
    "quote.unquote": _unquote,
    "path.normpath": lambda c: __import__("posixpath").normpath(c["p"]),
    "path.join": lambda c: __import__("posixpath").join(c["a"], c["b"]),
    "path.realpath": _path_realpath,
    "ipaddress.v4": _ipaddress_v4,
    "ipaddress.v6": _ipaddress_v6,
    "shlex.split": _shlex_split,
    "shlex.quote": _shlex_quote,
}


_OPS = {}
_OPS.update(_STR_OPS)
_OPS.update(_RE_OPS)
_OPS.update(_PYERR_OPS)
_OPS.update(_B_OPS)


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
