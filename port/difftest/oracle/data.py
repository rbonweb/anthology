"""F1 oracle: the data compiled into the Rust crates equals the Python values (plan F1, GC-6).

    python3 -I port/difftest/oracle/data.py   # -I: json.py here shadows the stdlib json

Compares, reading the Python modules at run time (so a later Python change shows up here):

- `anthology_guard::data` with `guard_rules.PROFILE_ROOTS`, `_PREFIX_VARIANTS` and
  `json.dumps(SELF_TEST_SAMPLE)`;
- `anthology_guard::secrets::GUARD_SECRET_PATTERNS` with `guard_rules._SECRET_PATTERNS`
  (pattern source and `re.IGNORECASE`, in order), and checks that each one is also in
  `memory_privacy.SECRET_PATTERNS`;
- `anthology_core::roster::ROSTER` with `memory_sync.ROSTER`.

The Rust values are read from the source files, so no Rust toolchain is needed. Exit 0 when
everything is equal, 1 otherwise. Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
GUARD_SRC = ROOT / "crates" / "anthology-guard" / "src"
CORE_SRC = ROOT / "crates" / "anthology-core" / "src"

sys.path.insert(0, str(ROOT / "plugins" / "anthology-guardrails" / "scripts"))
sys.path.insert(0, str(ROOT / "plugins" / "anthology" / "scripts"))
import guard_rules  # noqa: E402
import memory_privacy  # noqa: E402
import memory_sync  # noqa: E402

# One token of a Rust constant initializer.
_TOKEN = re.compile(r"""
    (?P<ws>\s+|//[^\n]*)
  | (?P<raw>r(?P<hashes>\#*)"(?P<rawbody>.*?)"(?P=hashes))
  | (?P<str>"(?P<body>(?:[^"\\]|\\.)*)")
  | (?P<bool>\btrue\b|\bfalse\b)
  | (?P<punct>[\[\](),&;])
""", re.VERBOSE | re.DOTALL)
_ESCAPES = {"n": "\n", "r": "\r", "t": "\t", "\\": "\\", "0": "\0", "'": "'", '"': '"'}


def _unescape(body: str) -> str:
    out, i = [], 0
    while i < len(body):
        c = body[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        e = body[i + 1]
        if e in _ESCAPES:
            out.append(_ESCAPES[e])
            i += 2
        elif e == "x":
            out.append(chr(int(body[i + 2:i + 4], 16)))
            i += 4
        elif e == "u":
            end = body.index("}", i)
            out.append(chr(int(body[i + 3:end], 16)))
            i = end + 1
        elif e == "\n":  # a line continuation skips the following whitespace
            i += 2
            while i < len(body) and body[i] in " \t\n\r":
                i += 1
        else:
            raise ValueError("unsupported escape \\" + e)
    return "".join(out)


def rust_const(path: Path, name: str):
    """The value of `pub const <name>: … = <init>;`: arrays and tuples of strings and bools."""
    text = path.read_text(encoding="utf-8")
    m = re.search(r"(?m)^pub const " + re.escape(name) + r"\s*:[^=]*=", text)
    if not m:
        raise SystemExit(f"data: {path.relative_to(ROOT)}: no `pub const {name}`")
    stack, pos = [[]], m.end()
    while True:
        t = _TOKEN.match(text, pos)
        if not t:
            raise SystemExit(f"data: {path.relative_to(ROOT)}: cannot read {name} at {text[pos:pos + 30]!r}")
        pos = t.end()
        if t.group("ws") or t.group("punct") in ("&", ","):
            continue
        if t.group("raw") is not None:
            stack[-1].append(t.group("rawbody"))
        elif t.group("str") is not None:
            stack[-1].append(_unescape(t.group("body")))
        elif t.group("bool"):
            stack[-1].append(t.group("bool") == "true")
        elif t.group("punct") in "[(":
            stack.append([])
        elif t.group("punct") in "])":
            items = stack.pop()
            stack[-1].append(items if t.group("punct") == "]" else tuple(items))
        elif t.group("punct") == ";":
            (value,) = stack[0]
            return value


def python_patterns(compiled):
    compiled = list(compiled)
    allowed = re.IGNORECASE | re.UNICODE
    for p in compiled:
        if p.flags & ~allowed:
            raise SystemExit(f"data: {p.pattern!r} has flags beyond IGNORECASE: {p.flags}")
    return [(p.pattern, bool(p.flags & re.IGNORECASE)) for p in compiled]


def main() -> int:
    data_rs, secrets_rs, roster_rs = GUARD_SRC / "data.rs", GUARD_SRC / "secrets.rs", CORE_SRC / "roster.rs"
    guard_patterns = python_patterns(guard_rules._SECRET_PATTERNS)
    checks = [
        ("anthology_guard::data::PROFILE_ROOTS", rust_const(data_rs, "PROFILE_ROOTS"),
         list(guard_rules.PROFILE_ROOTS)),
        ("anthology_guard::data::PREFIX_VARIANTS", rust_const(data_rs, "PREFIX_VARIANTS"),
         list(guard_rules._PREFIX_VARIANTS)),
        ("anthology_guard::data::SELF_TEST_SAMPLE_JSON", rust_const(data_rs, "SELF_TEST_SAMPLE_JSON"),
         json.dumps(guard_rules.SELF_TEST_SAMPLE)),
        ("anthology_guard::secrets::GUARD_SECRET_PATTERNS", rust_const(secrets_rs, "GUARD_SECRET_PATTERNS"),
         guard_patterns),
        ("anthology_core::roster::ROSTER", rust_const(roster_rs, "ROSTER"), list(memory_sync.ROSTER)),
    ]
    failed = False
    for name, rust, python in checks:
        if rust == python:
            print(f"ok    {name} ({len(python)} {'chars' if isinstance(python, str) else 'items'})")
        else:
            failed = True
            print(f"DIFF  {name}\n  rust:   {rust!r}\n  python: {python!r}")
    privacy = python_patterns(rx for _, rx in memory_privacy.SECRET_PATTERNS)
    missing = [p for p in guard_patterns if p not in privacy]
    if missing:
        failed = True
        print(f"DIFF  guard G6 patterns missing from memory_privacy.SECRET_PATTERNS: {missing!r}")
    else:
        print(f"ok    every G6 pattern is in memory_privacy.SECRET_PATTERNS ({len(guard_patterns)})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
