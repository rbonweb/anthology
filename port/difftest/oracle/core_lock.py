"""F3 oracle for the `core.lock` difftest surface (plan F3, GC-9, GC-17, design §8.2).

    python3 -I port/difftest/oracle/core_lock.py --golden PATH

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`.

Case: `{"op": "lock", "prefix": PREFIX, "text": TEXT}`: `anthology_core::lock::parse(TEXT,
PREFIX)`.
- `PREFIX` `rtk--v`: `plugins/anthology/scripts/rtk_install.py` is the oracle. `TEXT` is
  written (UTF-8) to `rtk.lock` in a temporary folder that becomes the working folder, and
  `load_lock("rtk.lock")` runs, so its "%s is not valid JSON" names `rtk.lock`, as Rust does.
- `PREFIX` `anthology-bin--v`: **there is no Python for anthology.lock.** `_anthology_load`
  below is hand-written from GC-17 and design §8.2: rtk's checks and messages with
  `anthology.lock` for `rtk.lock`, GC-17's version pattern, and the exact §8.2 layout. Golden
  lines for this prefix carry `"oracle": "hand-written from GC-17 (no Python)"`.

Result: `{"exit": 0, "stdout": "<version>\\n<tag>\\n<repo>\\n" + "<target> <sha256>\\n" per
target in TARGETS order, "stderr": ""}`, or `{"exit": 1, "stdout": "", "stderr":
"<type>: <message>"}` (`InstallError: ...` for every lock error).

D-6: when the JSON holds a lone surrogate, Rust decodes it as U+FFFD, so `%r` shows `'\\ufffd'`
where Python shows `'\\ud800'`. The result is then Python's with each lone surrogate replaced by
U+FFFD at decode time, with `"divergence": "D-6"` and `"python"` (Python's own result). A JSON
text deep enough for `RecursionError` (D-1) is not in the corpus.

`--golden PATH` writes the frozen corpus, one JSON line per case, from fixed cases and fixed
seeds. `crates/anthology-core/tests/golden.rs` replays it in `cargo test`;
`ci/jobs/difftest-linux.sh` regenerates it in the pinned 3.12 image and compares it byte for
byte with the committed file.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import random
import re
import shutil
import sys
import tempfile
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"

# plugins/anthology/rtk.lock as of F3 (rtk 1.2.1-anthology.1), frozen so an rtk bump does not
# change the golden corpus.
_RTK_LOCK_TEXT = (
    '{\n'
    '  "version": "1.2.1-anthology.1",\n'
    '  "tag": "rtk--v1.2.1-anthology.1",\n'
    '  "repo": "rbonweb/anthology",\n'
    '  "sha256": {\n'
    '    "aarch64-apple-darwin": "9c3ecad0d5e9f27c248b7c95157369e9e20fdfd84582a9aa321c92128aaadd4a",\n'
    '    "x86_64-apple-darwin": "bfdda72b35eb0c12cc0bff5db8d562e3587190cea32905d0ce2abba95c37c400",\n'
    '    "x86_64-unknown-linux-musl": "dfb363051a35930178d6361116f37349ecb8f9204d7ecbe70f7a3a8c9b6cbf79",\n'
    '    "aarch64-unknown-linux-musl": "88516082be3fbf158b4ed332b013f56eaadd977f85aea9597196963a4128f118"\n'
    '  }\n'
    '}\n'
)

RTK = "rtk--v"
ANTHOLOGY = "anthology-bin--v"
HAND_WRITTEN = "hand-written from GC-17 (no Python)"

_SURROGATE = re.compile("[\ud800-\udfff]")


def _load_rtk_install():
    sys.path.insert(0, str(SCRIPTS))  # rtk_install imports state
    try:
        spec = importlib.util.spec_from_file_location("_anthology_oracle_rtk_install",
                                                      str(SCRIPTS / "rtk_install.py"))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
    finally:
        sys.path.remove(str(SCRIPTS))
    return mod


_RTK = _load_rtk_install()
TARGETS = _RTK.TARGETS


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


def _loads_d6(text, **kw):
    return _fix(json.loads(text, object_pairs_hook=_fix_pairs, **kw))


def _ok(lock):
    lines = [lock["version"], lock["tag"], lock["repo"]]
    lines += ["%s %s" % (t, lock["sha256"][t]) for t in TARGETS]
    return {"exit": 0, "stdout": "".join(line + "\n" for line in lines), "stderr": ""}


def _err(exc):
    return {"exit": 1, "stdout": "", "stderr": "%s: %s" % (type(exc).__name__, exc)}


def _rtk(text, json_module):
    folder = tempfile.mkdtemp(prefix="core-lock-")
    saved_cwd = os.getcwd()
    saved_json = _RTK.json
    _RTK.json = json_module
    try:
        with open(os.path.join(folder, "rtk.lock"), "w", encoding="utf-8", newline="") as fh:
            fh.write(text)
        os.chdir(folder)
        try:
            return _ok(_RTK.load_lock("rtk.lock"))
        except Exception as exc:  # the oracle reports what Python raised
            return _err(exc)
    finally:
        os.chdir(saved_cwd)
        _RTK.json = saved_json
        shutil.rmtree(folder, ignore_errors=True)


# --- anthology.lock: hand-written from GC-17 and design §8.2 (there is no Python) -----------

class LockError(Exception):
    pass


_ANTHOLOGY_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?")


def render_anthology(lock):
    """The §8.2 layout: `json.dumps(indent=2)` of the keys in order, then a newline."""
    obj = {"version": lock["version"], "tag": lock["tag"], "repo": lock["repo"],
           "sha256": {t: lock["sha256"][t] for t in TARGETS if t in lock["sha256"]}}
    return json.dumps(obj, indent=2) + "\n"


def _anthology_load(text):
    name = "anthology.lock"
    try:
        lock = _loads_d6(text)
    except ValueError:
        raise LockError("%s is not valid JSON" % name) from None
    if not isinstance(lock, dict) or set(lock) != {"version", "tag", "repo", "sha256"}:
        raise LockError("%s must be an object with exactly version, tag, repo and sha256" % name)
    version, tag, repo, digests = lock["version"], lock["tag"], lock["repo"], lock["sha256"]
    if not isinstance(version, str) or not _ANTHOLOGY_VERSION.fullmatch(version):
        raise LockError("%s: invalid version %r" % (name, version))
    if tag != ANTHOLOGY + version:
        raise LockError("%s: tag must be %s%s" % (name, ANTHOLOGY, version))
    if (not isinstance(repo, str) or not _RTK.REPO_RE.fullmatch(repo)
            or any(part in (".", "..") for part in repo.split("/"))):
        raise LockError("%s: invalid repo %r" % (name, repo))
    if not isinstance(digests, dict) or set(digests) != set(TARGETS):
        raise LockError("%s: sha256 must have exactly the targets %s" % (name, ", ".join(TARGETS)))
    for target, digest in digests.items():
        if not isinstance(digest, str) or not _RTK.SHA256_RE.fullmatch(digest):
            raise LockError("%s: sha256 for %s must be 64 lowercase hex characters"
                            % (name, target))
    layout = render_anthology(lock)
    if text != layout and text != layout[:-1]:
        raise LockError("%s: not in the layout anthology-dev lock write produces" % name)
    return lock


def _anthology(text):
    try:
        return _ok(_anthology_load(text))
    except LockError as exc:
        return {"exit": 1, "stdout": "", "stderr": "InstallError: %s" % exc}


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    if case.get("op") != "lock":
        raise KeyError("unknown op %r" % case.get("op"))
    text, prefix = case["text"], case["prefix"]
    if prefix == ANTHOLOGY:
        return _anthology(text)
    if prefix != RTK:
        raise KeyError("unknown prefix %r" % prefix)
    result = _rtk(text, json)
    try:
        surrogates = _has_surrogate(json.loads(text))
    except (ValueError, RecursionError):
        surrogates = False
    if surrogates:
        expect = _rtk(text, types.SimpleNamespace(loads=_loads_d6))
        return dict(expect, divergence="D-6", python=result)
    return result


# ---------------------------------------------------------------------------------------------
# The golden corpus

_HEX = "0123456789abcdef" * 4


def _lock(prefix, version="1.2.3", **over):
    lock = {"version": version, "tag": "%s%s" % (prefix, version), "repo": "rbonweb/anthology",
            "sha256": {t: "%x" % i * 64 for i, t in enumerate(TARGETS, 10)}}
    lock.update(over)
    return lock


def _text(obj, indent=2):
    return json.dumps(obj, indent=indent)


_ODD_STRINGS = [
    "", "v1.2.3", "1.2", "1.2.", "1.2.3.", "1.2.3-", "1.2.3-a.", "1.2.3-.a", "1.2.3-a..b",
    "1.2.3+b", " 1.2.3", "1.2.3 ", "1.2.3\n", "1..2.3", "1.2.3-\u00e9", "\u00b2.1.1",
    "\u0661.\u0662.\u0663", "1.\uff11.3", "\U0001d7ce.1.1", "1.2.3-a-b", "1.2.3--", "1.2.3-A.b.9",
    "1.2.3-..", "1.2.3\x00", "1.2.3\x1f\x7f", "\u2028", "\u00e9", "\U0001f600", "\U000e0001",
    "\u00a0\u00ad\u0085", "\ufeff", "\u0378", "\U0010ffff", "a'b", 'a"b', "a'\"b", "\\", "\t\r",
    "\u200b\u202e", "\ue000", "\uffff", "\u3000", "x" * 300,
]
_ODD_VALUES = [None, True, False, 0, 1, -1, 1.5, -0.0, 1e16, 1e-7, 123456789012345678901234567890,
               [], {}, [1, "a", None], {"a": [1.5, True]}, {"k'": "v\""}, [[[]]]]


def _rtk_cases(prefix):
    """Every validate_lock branch, for `prefix`."""
    yield _text(_lock(prefix))
    yield _text(_lock(prefix), indent=None)
    yield _text(_lock(prefix)).rstrip("\n")
    yield _text(_lock(prefix)) + "\n"
    yield _text(_lock(prefix)).replace("\n", "\r\n")
    yield " \t" + _text(_lock(prefix), indent=4) + "\n\n"
    lock = _lock(prefix)
    yield _text({"sha256": lock["sha256"], "repo": lock["repo"], "tag": lock["tag"],
                 "version": lock["version"]})
    yield _text(dict(lock, sha256=dict(reversed(list(lock["sha256"].items())))))
    yield _text(_lock(prefix, "1.2.1-anthology.1"))
    yield _text(_lock(prefix, "10.20.30-rc.1", repo="A_b.c-d/x..y"))
    # not JSON, or not the object
    for text in ["", " ", "{", "nope", "\ufeff" + _text(_lock(prefix)), "[", "{\"version\": }",
                 "[]", "\"s\"", "1", "null", "true", "{}"]:
        yield text
    yield _text({k: v for k, v in _lock(prefix).items() if k != "repo"})
    yield _text(dict(_lock(prefix), extra=1))
    yield _text(dict(_lock(prefix), Version="1.2.3"))
    # duplicate keys: the last value wins, at the first position
    yield ('{"version": "bad", "tag": "%s1.2.3", "repo": "a/b", "sha256": %s, "version": "1.2.3"}'
           % (prefix, json.dumps(_lock(prefix)["sha256"])))
    # version
    for v in _ODD_STRINGS + _ODD_VALUES:
        yield _text(_lock(prefix, version=v, tag=prefix + v if isinstance(v, str) else "x"))
    for v in ["NaN", "Infinity", "-Infinity", "1E400", "-0", "1e16", "1.0"]:
        yield _text(_lock(prefix)).replace('"version": "1.2.3"', '"version": %s' % v)
    # tag
    for t in [None, 1, "", "rtk--v1.2.3 ", "RTK--v1.2.3", "rtk--v1.2.4", "anthology-bin--v1.2.3",
              "rtk-v1.2.3", "v1.2.3", ["rtk--v1.2.3"]]:
        yield _text(_lock(prefix, tag=t))
    # repo
    for rp in ["a", "a/b/c", "./b", "a/.", "../b", "a/..", "..", "/", "a/", "/b", "a b/c",
               "\u00e9/x", "a/b\n", "..a/b", "a/b.", ".a/b", "a\\b/c", "a/b%2f", None, 1,
               ["a/b"], "a'/b", "\u2028/x"]:
        yield _text(_lock(prefix, repo=rp))
    # sha256
    good = _lock(prefix)["sha256"]
    for d in [None, [], "x", 1, {}, dict(good, extra=_HEX),
              {k: v for k, v in good.items() if k != TARGETS[2]},
              dict({"x86_64-pc-windows-msvc": _HEX}, **{k: v for k, v in good.items()
                                                       if k != TARGETS[0]})]:
        yield _text(_lock(prefix, sha256=d))
    for bad in [None, 1, "", _HEX.upper(), _HEX[:63], _HEX + "0", "g" * 64, " " + _HEX[1:],
                [_HEX], "\u0660" * 64]:
        for t in (TARGETS[0], TARGETS[3]):
            yield _text(_lock(prefix, sha256=dict(good, **{t: bad})))
    # the first failing digest in the file's order is named
    yield _text(_lock(prefix, sha256={TARGETS[3]: "x", TARGETS[1]: _HEX, TARGETS[0]: "y",
                                      TARGETS[2]: _HEX}))
    # lone surrogates (D-6 for rtk)
    yield _text(_lock(prefix)).replace('"version": "1.2.3"', '"version": "1.2.3\\ud800"')
    yield _text(_lock(prefix)).replace('"repo": "rbonweb/anthology"', '"repo": "a/\\udc80b"')
    yield _text(_lock(prefix)).replace('"version": "1.2.3"', '"version": ["\\udfff", "x"]')


def _rand_char(r):
    k = r.random()
    if k < 0.4:
        return chr(r.randrange(0x20, 0x7F))
    if k < 0.5:
        return chr(r.randrange(0, 0x20))
    if k < 0.6:
        return chr(r.randrange(0x7F, 0x100))
    if k < 0.85:
        c = r.randrange(0x100, 0x10000)
        return chr(c) if not 0xD800 <= c <= 0xDFFF else "\u0378"
    return chr(r.randrange(0x10000, 0x110000))


def _random_texts(r, prefix, n):
    """Valid locks with one field replaced by a random value (repr coverage)."""
    for _ in range(n):
        field = r.choice(["version", "repo", "tag", "sha256"])
        k = r.random()
        if k < 0.6:
            value = "".join(_rand_char(r) for _ in range(r.randrange(1, 10)))
        elif k < 0.8:
            value = "%d.%d.%d-%s" % (r.randrange(100), r.randrange(100), r.randrange(100),
                                     "".join(_rand_char(r) for _ in range(r.randrange(1, 5))))
        else:
            value = r.choice(_ODD_VALUES)
        if field == "sha256":
            lock = _lock(prefix)
            lock["sha256"][r.choice(TARGETS)] = value
        else:
            lock = _lock(prefix, **{field: value})
            if field == "version" and isinstance(value, str) and r.random() < 0.7:
                lock["tag"] = prefix + value
        yield _text(lock, indent=r.choice([None, 2]))


def _cases():
    rtk_lock = _RTK_LOCK_TEXT
    yield {"op": "lock", "prefix": RTK, "text": rtk_lock}
    yield {"op": "lock", "prefix": ANTHOLOGY, "text": rtk_lock}
    for prefix in (RTK, ANTHOLOGY):
        for text in _rtk_cases(prefix):
            yield {"op": "lock", "prefix": prefix, "text": text}
    r = random.Random(20261121)
    for prefix in (RTK, ANTHOLOGY):
        for text in _random_texts(r, prefix, 250):
            yield {"op": "lock", "prefix": prefix, "text": text}


def golden(path):
    lines = []
    for case in _cases():
        line = {"case": case, "result": run(case)}
        if case["prefix"] == ANTHOLOGY:
            line["oracle"] = HAND_WRITTEN
        lines.append(json.dumps(line) + "\n")
    Path(path).write_text("".join(lines), encoding="ascii", newline="\n")


def main(argv):
    if len(argv) == 2 and argv[0] == "--golden":
        golden(argv[1])
        return 0
    sys.stderr.write("usage: python3 -I port/difftest/oracle/core_lock.py --golden PATH\n")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
