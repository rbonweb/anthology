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


# --- argparse -------------------------------------------------------------------------------
#
# A spec is JSON: `{"prog", "description"?, "epilog"?, "allow_abbrev"?, "args": [item]}`; an item
# is an argument, `{"mutex": {"required", "args": [argument]}}` or `{"sub": {"dest"?,
# "required"?, "help"?, "metavar"?, "parsers": [{"name", "help"?, "spec"}]}}` (a parser's spec
# without "prog" gets the default one). An argument is `{"name"}` (a positional) or
# `{"flags": [...], "dest"?}`, with any of "action" (store, store_true, store_false,
# store_const, append), "nargs", "const", "default", "type" (int, float, path), "choices",
# "required", "help", "help_suppress", "metavar" (a str, or a list for a tuple). A value is
# JSON, a float written `{"float": repr}`.

_AP_REPO = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                         "..", "..", ".."))

# The command lines the port carries: (script, entry); `main` is called with `[]` and stopped
# at its `parse_args`, `_parser` returns the parser.
_AP_CLIS = {
    "memory_portable": ("plugins/anthology/scripts/memory_portable.py", "main"),
    "rtk_install": ("plugins/anthology/scripts/rtk_install.py", "main"),
    "memory_index": ("plugins/anthology/scripts/memory_index.py", "_parser"),
    "anthology_setup": ("plugins/anthology/scripts/anthology_setup.py", "main"),
    "doctor": ("plugins/anthology/scripts/doctor.py", "main"),
    "release_edit": ("scripts/release_edit.py", "main"),
    "rtk_trim": ("scripts/rtk_trim.py", "main"),
    "vendor": ("scripts/vendor.py", "main"),
    "upstream_watch": ("scripts/upstream_watch.py", "main"),
    "eval_baseline": ("scripts/eval_baseline.py", "main"),
    "check_version_bump": ("scripts/check_version_bump.py", "main"),
    "startup_budget": ("scripts/startup_budget.py", "main"),
}
_AP_CAPTURED = {}


def _ap_in(v):
    if isinstance(v, dict):
        return float(v["float"])
    if isinstance(v, list):
        return [_ap_in(x) for x in v]
    return v


def _ap_add(container, a):
    import argparse
    import pathlib
    kw = {k: a[k] for k in ("action", "nargs", "required", "choices", "help", "dest") if k in a}
    if "const" in a:
        kw["const"] = _ap_in(a["const"])
    if "default" in a:
        kw["default"] = _ap_in(a["default"])
    if "type" in a:
        kw["type"] = {"int": int, "float": float, "path": pathlib.Path}[a["type"]]
    if a.get("help_suppress"):
        kw["help"] = argparse.SUPPRESS
    if "metavar" in a:
        m = a["metavar"]
        kw["metavar"] = tuple(m) if isinstance(m, list) else m
    if "name" in a:
        container.add_argument(a["name"], **kw)
    else:
        container.add_argument(*a["flags"], **kw)


def _ap_fill(p, spec):
    for item in spec["args"]:
        if "mutex" in item:
            g = p.add_mutually_exclusive_group(required=item["mutex"].get("required", False))
            for a in item["mutex"]["args"]:
                _ap_add(g, a)
        elif "sub" in item:
            s = item["sub"]
            kw = {k: s[k] for k in ("dest", "required", "help", "metavar") if k in s}
            sub = p.add_subparsers(**kw)
            for e in s["parsers"]:
                pkw = _ap_parser_kwargs(e["spec"])
                if "help" in e:
                    pkw["help"] = e["help"]
                _ap_fill(sub.add_parser(e["name"], **pkw), e["spec"])
        else:
            _ap_add(p, item)
    return p


def _ap_parser_kwargs(spec):
    kw = {k: spec[k] for k in ("prog", "description", "epilog") if spec.get(k) is not None}
    if spec.get("allow_abbrev") is False:
        kw["allow_abbrev"] = False
    return kw


def _ap_capture(name):
    """The real parser of `_AP_CLIS[name]`, made with the current COLUMNS."""
    import argparse
    import importlib.util
    rel, entry = _AP_CLIS[name]
    path = os.path.join(_AP_REPO, rel)
    folder = os.path.dirname(path)

    class Got(Exception):
        pass

    def stop(self, args=None, namespace=None):
        raise Got(self)

    saved = sys.argv[0], argparse.ArgumentParser.parse_args
    sys.path.insert(0, folder)
    sys.argv[0] = os.path.basename(path)
    try:
        spec = importlib.util.spec_from_file_location("difftest_cli_" + name, path)
        mod = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = mod
        spec.loader.exec_module(mod)
        if entry == "_parser":
            return _ap_unroot(mod._parser())
        argparse.ArgumentParser.parse_args = stop
        try:
            mod.main([])
        except Got as got:
            return _ap_unroot(got.args[0])
        raise RuntimeError("%s: main([]) did not call parse_args" % name)
    finally:
        sys.argv[0], argparse.ArgumentParser.parse_args = saved
        sys.path.remove(folder)


def _ap_unroot(parser):
    """`parser` with the repository root in its defaults written `<REPO>` (the help wraps a
    default as text, so it is replaced before formatting, not after)."""
    import argparse
    import pathlib
    for action in parser._actions:
        d = action.default
        if isinstance(d, (str, pathlib.PurePath)) and str(d).startswith(_AP_REPO):
            new = "<REPO>" + str(d)[len(_AP_REPO):]
            action.default = new if isinstance(d, str) else type(d)(new)
        if isinstance(action, argparse._SubParsersAction):
            for sub in action.choices.values():
                _ap_unroot(sub)
    return parser


def _ap_out(v):
    import pathlib
    if isinstance(v, pathlib.PurePath):
        v = str(v)
    if isinstance(v, str):
        return v.replace(_AP_REPO, "<REPO>")
    if v is None or isinstance(v, (bool, int)):
        return v
    if isinstance(v, float):
        return {"float": repr(v)}
    if isinstance(v, (list, tuple)):
        return [_ap_out(x) for x in v]
    raise TypeError("argparse oracle: a namespace value of type %s" % type(v).__name__)


def _ap_parse(case):
    """`{"cli" | "spec", "argv", "columns"}`: `parse_args(argv)` of the real parser of a command
    line (`_AP_CLIS`) or of a parser built from a spec, with `COLUMNS=columns`. The answer is
    the exit status and what it printed, the repository root written `<REPO>`; for a parse
    that returns, stdout is `{"ns": vars(namespace), "order": [its keys]}` (callables, e.g. a
    `set_defaults(func=...)`, left out)."""
    import argparse
    import contextlib
    import io
    os.environ["COLUMNS"] = str(case["columns"])
    try:
        if "cli" in case:
            key = (case["cli"], case["columns"])
            if key not in _AP_CAPTURED:
                _AP_CAPTURED[key] = _ap_capture(case["cli"])
            parser = _AP_CAPTURED[key]
        else:
            spec = case["spec"]
            parser = _ap_fill(argparse.ArgumentParser(**_ap_parser_kwargs(spec)), spec)
        out, err = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                ns = parser.parse_args(case["argv"])
        except SystemExit as e:
            return {"exit": e.code, "stdout": _ap_out(out.getvalue()),
                    "stderr": _ap_out(err.getvalue())}
    finally:
        del os.environ["COLUMNS"]
    items = {k: v for k, v in vars(ns).items() if not callable(v)}
    result = {"ns": {k: _ap_out(v) for k, v in items.items()}, "order": list(items)}
    return {"exit": 0, "stdout": json.dumps(result, sort_keys=True) + out.getvalue(),
            "stderr": err.getvalue()}


# Ops that answer with the whole `{"exit", "stdout", "stderr"}`.
_FULL_OPS = {
    "argparse.parse": _ap_parse,
}


_OPS = {}
_OPS.update(_STR_OPS)
_OPS.update(_RE_OPS)
_OPS.update(_PYERR_OPS)
_OPS.update(_B_OPS)


def run(case):
    full = _FULL_OPS.get(case.get("op"))
    if full is not None:
        return full(case)
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
