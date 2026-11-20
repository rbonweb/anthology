"""F2 oracle for the `pycompat.json` difftest surface (plan F2, design §12 and §13.2).

    python3 -I port/difftest/oracle/json.py --golden PATH

This file shadows the standard library's `json` for any script run from this folder, so every
oracle script runs as `python3 -I ...` (no script folder on `sys.path`), and F4's driver loads
oracle modules by file path under another module name. Run without `-I`, it stops with a message.

Library surface (F4's `_driver.py` calls it): `run(case: dict) -> dict`.

Cases:
- `{"op": "loads_dumps", "text": STR, "preset": NAME}`: `json.dumps(json.loads(text), **preset)`.
- `{"op": "loads_bytes_lossy", "hex": HEX, "preset": NAME}`: the bytes are hex; the text is
  `bytes.decode("utf-8", errors="replace")`, as `guard.py` decodes stdin.
- `{"op": "loads_utf8_sig", "hex": HEX, "preset": NAME}`: the text is `bytes.decode("utf-8")`
  with one leading BOM dropped, as `settings_patch.read_text` plus its `text[1:] if
  text.startswith("\\ufeff")` reads settings.json. A strict decode error is the result.
`NAME` is a GC-4 preset (`PRESETS`).

Result: `{"exit": 0, "stdout": OUT, "stderr": ""}`, or on an exception `{"exit": 1, "stdout":
"", "stderr": "<type name>: <str(exc)>"}` (the last line of a traceback). For a
`JSONDecodeError` that is `msg: line L column C (char POS)`, so the message and the code-point
position are both compared; a `UnicodeDecodeError` names its byte position; the 4300-digit int
limit is a `ValueError` with no position.

The result is what Rust must print. Where Rust differs from Python by an allowed class (GC-16),
the result carries `"divergence"` and `"python"` (Python's own result):
- `D-6` (lone surrogates): Rust decodes a lone surrogate escape as U+FFFD. The expected output is
  Python's with every lone surrogate in a decoded string or key replaced by U+FFFD at decode time
  (`object_pairs_hook`, so keys that then collide keep the first position and take the last
  value, as Rust's `Map` does). Nothing else is normalized.
- `D-1` (Python raised `RecursionError`): Rust parses and prints any depth (F-10). The expected
  output is the pure-Python `json` decoder and encoder's, run with the recursion limit raised in a
  thread with a large stack.

`--golden PATH` writes the frozen corpus, one JSON line `{"case": ..., "result": ...}` per case,
generated from fixed seeds. A `stdout` longer than `_BIG` characters is stored as `stdout_len`
(code points) and `stdout_fnv1a64` (FNV-1a 64 of its UTF-8, 16 lowercase hex digits).
`crates/anthology-pycompat/tests/json.rs` replays it in `cargo test`; `ci/jobs/difftest-linux.sh`
regenerates it in the pinned 3.12 image and compares it byte for byte with the committed file.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import sys
from pathlib import Path

import json

if Path(getattr(json, "__file__", "")).resolve() == Path(__file__).resolve():
    sys.exit("port/difftest/oracle/json.py shadows the stdlib json: run oracle scripts as python3 -I")

import json.decoder  # noqa: E402
import json.encoder  # noqa: E402
import json.scanner  # noqa: E402
import random  # noqa: E402
import re  # noqa: E402
import struct  # noqa: E402
import threading  # noqa: E402

PRESETS = {
    "PY_DEFAULT": {},
    "PY_DEFAULT_SORTED": {"sort_keys": True},
    "PY_DEFAULT_UNICODE": {"ensure_ascii": False},
    "INDENT2": {"indent": 2},
    "INDENT2_SORTED": {"indent": 2, "sort_keys": True},
    "STATE": {"indent": 2, "sort_keys": True, "ensure_ascii": False},
    "COMPACT_SORTED": {"sort_keys": True, "separators": (",", ":")},
}
PRESET_NAMES = list(PRESETS)

_SURROGATE = re.compile("[\ud800-\udfff]")
_BIG = 16384


# ---------------------------------------------------------------------------------------------
# run(case)

def _text_of(case):
    op = case["op"]
    if op == "loads_dumps":
        return case["text"]
    raw = bytes.fromhex(case["hex"])
    if op == "loads_bytes_lossy":
        return raw.decode("utf-8", errors="replace")
    if op == "loads_utf8_sig":
        text = raw.decode("utf-8")
        return text[1:] if text.startswith("\ufeff") else text
    raise KeyError("unknown op %r" % op)


def _python(case, loads, dumps):
    """`(result, value)`; value is None on an exception."""
    try:
        value = loads(_text_of(case))
        out = dumps(value, **PRESETS[case["preset"]])
    except (ValueError, RecursionError) as exc:  # JSONDecodeError, UnicodeDecodeError: ValueError
        return {"exit": 1, "stdout": "", "stderr": "%s: %s" % (type(exc).__name__, exc)}, None
    return {"exit": 0, "stdout": out, "stderr": ""}, value


def _has_surrogate(value):
    stack = [value]
    while stack:
        v = stack.pop()
        if isinstance(v, str):
            if _SURROGATE.search(v):
                return True
        elif isinstance(v, list):
            stack.extend(v)
        elif isinstance(v, dict):
            for k, item in v.items():
                if _SURROGATE.search(k):
                    return True
                stack.append(item)
    return False


def _fix(v):
    """Lone surrogates in strings and list items → U+FFFD (dicts come fixed from the hook)."""
    if isinstance(v, str):
        return _SURROGATE.sub("\ufffd", v)
    if isinstance(v, list):
        return [_fix(x) for x in v]
    return v


def _fix_pairs(pairs):
    d = {}
    for k, v in pairs:
        d[_SURROGATE.sub("\ufffd", k)] = _fix(v)
    return d


def _loads_d6(text):
    return _fix(json.loads(text, object_pairs_hook=_fix_pairs))


def _pure(case):
    """Python's result from the pure-Python decoder and encoder (no C recursion limit)."""
    decoder = json.JSONDecoder()
    decoder.parse_string = json.decoder.py_scanstring
    decoder.scan_once = json.scanner.py_make_scanner(decoder)

    def loads(text):
        if text.startswith("\ufeff"):
            raise json.JSONDecodeError("Unexpected UTF-8 BOM (decode using utf-8-sig)", text, 0)
        return decoder.decode(text)

    def dumps(value, **kw):
        return json.JSONEncoder(**kw).encode(value)

    out = {}

    def work():
        saved_limit = sys.getrecursionlimit()
        saved_encoder = json.encoder.c_make_encoder
        sys.setrecursionlimit(1_000_000)
        json.encoder.c_make_encoder = None
        try:
            out["result"], out["value"] = _python(case, loads, dumps)
        finally:
            json.encoder.c_make_encoder = saved_encoder
            sys.setrecursionlimit(saved_limit)

    saved_size = threading.stack_size(1 << 29)
    try:
        t = threading.Thread(target=work)
        t.start()
        t.join()
    finally:
        threading.stack_size(saved_size)
    return out["result"], out["value"]


def run(case):
    """What Rust must print for `case` (module docstring)."""
    result, value = _python(case, json.loads, json.dumps)
    if result["stderr"].startswith("RecursionError:"):
        expect, value = _pure(case)
        if value is not None and _has_surrogate(value):
            raise AssertionError("a deep case must not hold lone surrogates")
        return dict(expect, divergence="D-1", python=result)
    if value is not None and _has_surrogate(value):
        expect, _ = _python(case, _loads_d6, json.dumps)
        return dict(expect, divergence="D-6", python=result)
    return result


# ---------------------------------------------------------------------------------------------
# The golden corpus (fixed seeds)

def _fnv1a64(text):
    h = 0xCBF29CE484222325
    for b in text.encode("utf-8"):
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return "%016x" % h


def _compact(result):
    out = dict(result)
    if len(out["stdout"]) > _BIG:
        stdout = out.pop("stdout")
        out["stdout_len"] = len(stdout)
        out["stdout_fnv1a64"] = _fnv1a64(stdout)
    if "python" in out:
        out["python"] = _compact(out["python"])
    return out


def _valid(text):
    try:
        json.loads(text)
    except (ValueError, RecursionError):
        return False
    return True


_FIXED_VALID = [
    # literals and numbers
    "null", "true", "false", "0", "-0", "1", "-1", "123", "-123",
    "9007199254740991", "9007199254740992", "9007199254740993", "-9007199254740993",
    "9223372036854775807", "9223372036854775808", "-9223372036854775808",
    "-9223372036854775809", "18446744073709551615", "18446744073709551616",
    "12345678901234567890123456789", "1" + "0" * 20, "1" + "0" * 21, "1" + "0" * 22,
    "1" * 4300, "-" + "9" * 4300, "1" + "0" * 4299,
    "0.0", "-0.0", "1.0", "-1.0", "1.5", "100.0", "0.1", "0.2", "0.30000000000000004",
    "1e16", "1E16", "1e+16", "1e15", "1e-5", "1e-4", "0.0001", "0.00001", "1e-7", "1.5e-7",
    "1e21", "1e22", "1e20", "123e-20", "2.5e-7", "1.0e+2", "0.1e1", "1E400", "1e400",
    "-1e400", "1e-400", "-1e-400", "5e-324", "4.9e-324", "2e-324", "3e-324",
    "2.2250738585072014e-308", "1.7976931348623157e308", "1.7976931348623159e308",
    "123456789012345678", "123456789012345678.0", "1234567890123456.0", "12345678901234567.0",
    "9007199254740993.0", "0.000001", "0.0000001", "1e-6", "123456.789e3", "-1.5E-10",
    "1" + "0" * 400 + ".5", "0." + "0" * 400 + "1", "1" * 5000 + ".0", "1" * 5000 + "e-5000",
    "NaN", "Infinity", "-Infinity", "[NaN, Infinity, -Infinity, -0.0, 0.0]",
    # strings
    '""', '"abc"', '"\\"\\\\\\/\\b\\f\\n\\r\\t"', '"\\u0000\\u001f\\u007f\\u0080\\u009f"',
    '"' + "".join("\\u%04x" % c for c in range(32)) + '"', '"\\u00e9\\u00E9"', '"\\u0041"',
    '"\u00e9"', '"\u2028\u2029"', '"\U0001f600"', '"\\ud83d\\ude00"', '"\\uD83D\\uDE00"',
    '"\\udbff\\udfff"', '"\\ud800\\udc00"', '"\U0010ffff\U00010000"', '"\\ud800"', '"\\udc00"',
    '"\\udc00\\ud800"', '"\\ud800\\u0041"', '"\\ud800x"', '"a\\ud800"', '"\\ud800\\ud800\\udc00"',
    '"\\ud83d\\ude00\\ud83d"', '"\\ufffd"', '"\\uffff"', '"\uffff\ufffe"', '"\x7f"', '"\ufeff"',
    '"\\ufeff"', '"\x80\x9f\xa0\xff"', '"/"', '"a/b"', '"\u0661\u0662"', '"\u00a0\u3000"',
    '"\\ud7ff\\ue000"', '"\\u2028"', '"\U0001d11e\u20ac$"',
    # containers
    "[]", "{}", "[ ]", "{ }", "[[]]", "[{}]", '{"a": []}', '{"a": {}}', "[[], {}, [[]]]",
    '[1, [2, [3, []]], {}]', '{"b": 1, "a": 2}', '{"a": 1, "b": 2, "a": 3}',
    '{"a": 1, "a": {"x": 1}, "a": [2]}', '{"a": {"b": {"c": [1, {"d": null}]}}}',
    '{"\u00e9": 1, "z": 2, "Z": 3, "\U0001f600": 4, "\uffff": 5, "": 6, "a\\u0000": 7, "aa": 8}',
    '{"\\ud800": 1, "\\udc00": 2, "\\ud800": 3}', '{"\\ud800": 1, "\\ue000": 2, "\\ud7ff": 3}',
    '{"b": 1, "2": 2, "1": 3, "01": 4, "-1": 5, "4294967294": 6, "4294967295": 7, "10": 8,'
    ' "a": 9, "0": 10, "9007199254740993": 11, "1.0": 12, " 1": 13}',
    '{"__proto__": 1, "constructor": 2}', '[1, "a", null, true, false, 1.5, {"k": "v"}]',
    '{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "x\\ny"},'
    ' "systemMessage": "m"}',
    # whitespace
    " \t\n\r[ 1 ,\n2\t]\r\n ", '\n{\n"a"\n:\n1\n,\n"b":[\n]\n}\n', " 1 ", "\t\"x\"\t",
]

_FIXED_INVALID = [
    "", " ", "\n\t", "[", "{", "]", "}", "[1", "[1,", "[1,]", "[,1]", "[1 2]", "[1,,2]",
    '{"a"}', '{"a" 1}', '{"a":}', '{"a":1,}', "{,}", "{1: 2}", "{'a': 1}", "[1]]", "1 2",
    '{"a":1}x', '{"a":1 "b":2}', '["a""b"]', "[true false]", "truefalse", "nul", "tru", "fals",
    "nulll", "NaNx", "Nan", "nan", "-", "-a", "--1", "+1", "01", "-01", "00", "1.", ".5", "1e",
    "1e+", "1.e5", "1.5e", "-Infinityx", "-NaN", "infinity", "-infinity", "Infinit", "1_000",
    "0x10", "1e5x", "\u00a0 1", "1\u00a0", "[1,\u3000 2]", "\u0661", '"abc', '"\\', '"\\x"',
    '"\\a"', '"\\U0041"', '"\\u12"', '"\\u12', '"\\u123"', '"\\u1234', '"\\uXYZW"', '"\\u12G4"',
    '"\\u 123"', '"\\u+123"', '"\\u0x12"', '"\\u1_23"', '"\\ud800\\u12"', '"\\ud800\\uZZZZ"',
    '"\\ud800\\udc00', '"\\ud800\\', '"\\ud800\\u', '"\\ud800\\udc0"', '"\\ud800\\x"',
    '"a\nb"', '"\t"', '"\x00"', '"\x1f"', '["\x01"]', '{"a\x02": 1}', '{"a": "b\rc"}',
    "\ufeff{}", "\ufeff", "\ufeff 1", " \ufeff1", "[1, 2", '{"a": [1, {"b": ', "[" * 20,
    '{"a": 1, "a": 2', "\x00", "/* c */ 1", "[1] // c", "'a'", "undefined", "[-]", "[.1]",
    "{\"a\": 1, }", "[\"a\",]", '{"a":1,"b"}',
    # past the 4300-digit int limit: a ValueError, not a JSONDecodeError
    "1" * 4301, "-" + "9" * 4301, '{"n": [1, ' + "7" * 6000 + "]}", "1" * 4301 + ".0x",
]


def _fixed_cases():
    for text in _FIXED_VALID:
        if not _valid(text):
            raise AssertionError("not valid: %r" % text)
        for preset in PRESET_NAMES:
            yield {"op": "loads_dumps", "text": text, "preset": preset}
    for text in _FIXED_INVALID:
        if _valid(text):
            raise AssertionError("valid: %r" % text)
        yield {"op": "loads_dumps", "text": text, "preset": "PY_DEFAULT"}


# Random JSON texts: a value tree is generated, then written with random whitespace and escapes.
_WS = " \t\n\r"
_SHORT = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\f": "\\f", "\n": "\\n", "\r": "\\r",
          "\t": "\\t", "/": "\\/"}


def _rand_char(r):
    k = r.random()
    if k < 0.45:
        return chr(r.randrange(0x20, 0x7F))
    if k < 0.55:
        return r.choice('"\\/\b\f\n\r\t\x00\x01\x1f\x7f')
    if k < 0.62:
        return chr(r.randrange(0x80, 0x100))
    if k < 0.80:
        c = r.randrange(0x100, 0xFFFE)
        return chr(c) if not 0xD800 <= c <= 0xDFFF else "\ufffd"
    if k < 0.84:
        return r.choice("\u2028\u2029\ufeff\uffff\ufffe\u00a0\u0085\u3000")
    if k < 0.995:
        return chr(r.randrange(0x10000, 0x110000))
    return chr(r.randrange(0xD800, 0xE000))  # a lone surrogate: only ever written escaped


def _rand_str(r, n_max=8):
    return "".join(_rand_char(r) for _ in range(r.randrange(n_max + 1)))


def _write_str(r, s):
    out = ['"']
    for ch in s:
        c = ord(ch)
        if 0xD800 <= c <= 0xDFFF:
            out.append("\\u%04x" % c if r.random() < 0.5 else "\\u%04X" % c)
        elif c > 0xFFFF and r.random() < 0.3:
            v = c - 0x10000
            out.append("\\u%04x\\u%04x" % (0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)))
        elif ch in _SHORT and (c < 0x20 or ch in '"\\' or r.random() < 0.5):
            out.append(_SHORT[ch] if r.random() < 0.7 else "\\u%04x" % c)
        elif c < 0x20 or (r.random() < 0.08 and c <= 0xFFFF):
            out.append("\\u%04x" % c if r.random() < 0.5 else "\\u%04X" % c)
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def _rand_int(r):
    k = r.random()
    if k < 0.4:
        n = r.randrange(-1000, 1000)
    elif k < 0.6:
        n = r.randrange(-(2 ** 63), 2 ** 63)
    elif k < 0.75:
        n = r.choice([2 ** 53, 2 ** 63, 2 ** 64, 10 ** 21]) + r.randrange(-3, 4)
        n = -n if r.random() < 0.3 else n
    elif k < 0.95:
        n = r.randrange(10 ** 18, 10 ** r.randrange(19, 60))
        n = -n if r.random() < 0.3 else n
    else:
        n = r.randrange(10 ** 300, 10 ** 320)
    return str(n)


def _rand_float(r):
    k = r.random()
    if k < 0.45:
        while True:
            f = struct.unpack("<d", struct.pack("<Q", r.getrandbits(64)))[0]
            if f == f and f not in (float("inf"), float("-inf")):
                return repr(f)
    if k < 0.65:
        mant = "%d.%d" % (r.randrange(10 ** r.randrange(1, 8)), r.randrange(10 ** r.randrange(1, 8)))
        if r.random() < 0.5:
            mant += "%s%s%d" % (r.choice("eE"), r.choice(["", "+", "-"]), r.randrange(30))
        return mant
    if k < 0.80:
        e = r.randrange(-330, 310)
        return "%s%de%d" % (r.choice(["", "-"]), r.randrange(1, 10), e)
    if k < 0.9:
        return repr(r.uniform(-1e6, 1e6))
    return r.choice(["NaN", "Infinity", "-Infinity", "-0.0", "0.0", "1e16", "5e-324", "1e400"])


def _rand_key(r):
    k = r.random()
    if k < 0.15:
        return str(r.choice([0, 1, 2, 10, 99, 4294967294, 4294967295, 2 ** 53 + 1]))
    if k < 0.2:
        return r.choice(["01", "-1", "1.0", "", " 1"])
    return _rand_str(r, 6)


def _rand_text(r, depth=0):
    def ws():
        return "".join(r.choice(_WS) for _ in range(r.choice([0, 0, 0, 1, 2])))

    k = r.random()
    if depth < 4 and k < 0.3:
        n = r.randrange(0, 6)
        if n == 0:
            return "[" + ws() + "]"
        return "[" + ws() + ("," + ws()).join(_rand_text(r, depth + 1) + ws() for _ in range(n)) + "]"
    if depth < 4 and k < 0.55:
        n = r.randrange(0, 6)
        if n == 0:
            return "{" + ws() + "}"
        keys = [_rand_key(r) for _ in range(n)]
        if n > 1 and r.random() < 0.3:
            keys[r.randrange(n)] = keys[0]  # a duplicate key
        items = [ws() + _write_str(r, key) + ws() + ":" + ws() + _rand_text(r, depth + 1) + ws()
                 for key in keys]
        return "{" + ",".join(items) + "}"
    if k < 0.7:
        return _write_str(r, _rand_str(r))
    if k < 0.82:
        return _rand_int(r)
    if k < 0.95:
        return _rand_float(r)
    return r.choice(["null", "true", "false"])


def _random_cases(r, count):
    for i in range(count):
        text = _rand_text(r)
        if r.random() < 0.2:
            text = r.choice(_WS) + text + r.choice(_WS)
        yield {"op": "loads_dumps", "text": text, "preset": PRESET_NAMES[i % len(PRESET_NAMES)]}


_MUTATION_CHARS = '[]{}:,"\\ 0123456789eE.+-aunltrfsINx\x00\x1f\t\n\u00e9\U0001f600'


def _mutation_cases(r, count):
    for _ in range(count):
        text = _rand_text(r, 2)
        if not text:
            continue
        k = r.random()
        i = r.randrange(len(text))
        if k < 0.35:
            text = text[:i] + text[i + 1:]
        elif k < 0.7:
            text = text[:i] + r.choice(_MUTATION_CHARS) + text[i:]
        elif k < 0.9:
            text = text[:i]
        else:
            text = text[:i] + r.choice(_MUTATION_CHARS) + text[i + 1:]
        yield {"op": "loads_dumps", "text": text, "preset": "PY_DEFAULT"}


# Byte strings with invalid UTF-8.
_BAD_BYTES = [
    b"\x80", b"\xbf", b"\x80\x80", b"\xc0", b"\xc1", b"\xc0\x80", b"\xc0\xaf", b"\xc1\xbf",
    b"\xc2", b"\xdf", b"\xe0", b"\xe0\x80", b"\xe0\x80\x80", b"\xe0\x80\xaf", b"\xe0\x9f\xbf",
    b"\xe0\xa0", b"\xe2\x82", b"\xed\xa0\x80", b"\xed\xbf\xbf", b"\xed\xa0", b"\xed\xb0\x80",
    b"\xed\xa0\xbd\xed\xb8\x80", b"\xef\xbf", b"\xf0", b"\xf0\x80\x80\x80", b"\xf0\x8f\xbf\xbf",
    b"\xf0\x90", b"\xf0\x90\x80", b"\xf4\x90\x80\x80", b"\xf4\x8f\xbf", b"\xf5\x80\x80\x80",
    b"\xf7\xbf\xbf\xbf", b"\xf8\x88\x80\x80\x80", b"\xfc\x84\x80\x80\x80\x80", b"\xfe", b"\xff",
    b"\xff\xfe", b"\xfe\xff",
]
_GOOD_BYTES = [
    b"a", b" ", b"\\n", b"\\u00e9", b"\\ud800", b"\xc2\x80", b"\xc3\xa9", b"\xdf\xbf",
    b"\xe0\xa0\x80", b"\xe2\x82\xac", b"\xed\x9f\xbf", b"\xee\x80\x80", b"\xef\xbf\xbf",
    b"\xef\xbb\xbf", b"\xf0\x90\x80\x80", b"\xf0\x9f\x98\x80", b"\xf4\x8f\xbf\xbf",
]
_BOM = b"\xef\xbb\xbf"


def _byte_cases(r, count):
    fixed = [
        b"", _BOM, _BOM + b"{}", _BOM + _BOM + b"{}", _BOM + b' {"a": 1}', b" " + _BOM + b"1",
        _BOM + b"\xff", _BOM + b'"\xff"', _BOM + b'"a\xe2\x82"', b'"\xe2\x82"', b'"\xe2\x82',
        b"\xe2\x82", b'{"a": "\xed\xa0\x80"}', b'{"\xc0\x80": 1, "\xc1\x81": 2}', b"[1, 2]\xff",
        b'["\xf0\x9f\x98"]', b'"\xf0\x9f\x98\x80"', b"\xff\xfe1\x00", b'"\x00"',
        b'{"tool_name": "Bash", "tool_input": {"command": "echo \xe9t\xe9"}}',
    ]
    for raw in fixed:
        yield raw
    for _ in range(count):
        parts = [b'"']
        for _ in range(r.randrange(1, 8)):
            parts.append(r.choice(_BAD_BYTES) if r.random() < 0.5 else r.choice(_GOOD_BYTES))
        if r.random() < 0.9:
            parts.append(b'"')
        raw = b"".join(parts)
        if r.random() < 0.15:
            raw = _BOM + raw
        if r.random() < 0.2:
            raw = b'{"k": ' + raw + b"}"
        yield raw


def _bytes_to_cases(raws):
    for i, raw in enumerate(raws):
        yield {"op": "loads_bytes_lossy", "hex": raw.hex(),
               "preset": "PY_DEFAULT" if i % 2 == 0 else "PY_DEFAULT_UNICODE"}
        yield {"op": "loads_utf8_sig", "hex": raw.hex(),
               "preset": "PY_DEFAULT_UNICODE" if i % 2 == 0 else "COMPACT_SORTED"}


def _deep_cases():
    d = 10_000
    arrays = "[" * d + "]" * d
    objects = '{"a":' * d + "1" + "}" * d
    mixed = '[{"k": ' * (d // 2) + '"\U0001f600"' + "}]" * (d // 2)
    yield {"op": "loads_dumps", "text": arrays, "preset": "PY_DEFAULT"}
    yield {"op": "loads_dumps", "text": arrays, "preset": "COMPACT_SORTED"}
    yield {"op": "loads_dumps", "text": objects, "preset": "PY_DEFAULT_SORTED"}
    yield {"op": "loads_dumps", "text": mixed, "preset": "PY_DEFAULT_UNICODE"}
    yield {"op": "loads_dumps", "text": mixed, "preset": "PY_DEFAULT"}
    yield {"op": "loads_dumps", "text": "[" * d + "1,]" + "]" * (d - 1), "preset": "PY_DEFAULT"}
    yield {"op": "loads_dumps", "text": '{"a": ' * d + "}" * d, "preset": "PY_DEFAULT"}
    yield {"op": "loads_bytes_lossy", "hex": ("[" * d + "\"\xff\"" + "]" * d).encode("latin-1").hex(),
           "preset": "PY_DEFAULT"}
    shallow = 1_500
    yield {"op": "loads_dumps", "text": "[" * shallow + "]" * shallow, "preset": "INDENT2"}
    yield {"op": "loads_dumps", "text": '{"b": [' * shallow + "]}" * shallow,
           "preset": "INDENT2_SORTED"}
    yield {"op": "loads_dumps", "text": '{"\u00e9": ' * shallow + "null" + "}" * shallow,
           "preset": "STATE"}
    yield {"op": "loads_dumps", "text": "[" * 900 + "1.5" + "]" * 900, "preset": "INDENT2"}


def _cases():
    yield from _fixed_cases()
    yield from _random_cases(random.Random(20261120), 1000)
    yield from _mutation_cases(random.Random(20261121), 900)
    yield from _bytes_to_cases(_byte_cases(random.Random(20261122), 300))
    yield from _deep_cases()


def golden(path):
    lines = []
    for case in _cases():
        if case["op"] == "loads_dumps":
            case["text"].encode("utf-8")  # a case text is always valid Unicode
        lines.append(json.dumps({"case": case, "result": _compact(run(case))}) + "\n")
    Path(path).write_text("".join(lines), encoding="ascii", newline="\n")


def main(argv):
    if len(argv) == 2 and argv[0] == "--golden":
        golden(argv[1])
        return 0
    sys.stderr.write("usage: python3 -I port/difftest/oracle/json.py --golden PATH\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
