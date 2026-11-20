"""DV4 oracle for the `dev.rtk-trim` difftest surface (plan DV4, GC-16).

    python3 -I port/difftest/oracle/dev_rtk_trim.py        # self-check of the helpers

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path and calls `run(case)`.

The oracle is `scripts/rtk_trim.py` itself, loaded by path. Every case runs in a fresh temporary
folder (the working folder for the run, so paths in messages are relative, as the Rust side's
`run_in(cwd, ...)` shows them). Library surface: `run(case: dict) -> dict`.

Cases (`"mode"`):
- `"trim"`: the real trim. The files the operations edit are copied from `rtk/` of this
  repository into `rtk/` of the folder, then `"mutations"` edit them in order (`{"path", "kind"
  "insert"|"delete_line"|"dup_line"|"replace"|"append", "line", "text", "find", "with"}`; a
  mutation of a file the repository does not have is ignored) and `"extras"` (`{path: text}`)
  and `"links"` (`{path: target}`) are created. Then `rtk_trim.main(["--root", "rtk"])` runs.
  `files` is the tree of `rtk/` with each file's text replaced by `sha256:<hex>`.
- `"prim"`: `"files"` (`{path: text}`) are written under `rtk/`, then `"ops"` run in order on
  `Trim("rtk")` until one raises `TrimError` (`{"op": "rm", "paths"}`, `{"op": "sub", "path",
  "old", "new", "done"}`, `{"op": "cut", "path", "old", "gone"}`, `{"op": "drop", "path", "sig",
  "gone"}`). Exit 0 with the `changed` entries one per line on stdout, or exit 1 with
  `rtk-trim: <error>` on stderr. `files` is the full tree of `rtk/`.
- `"lock-guard"`: `"before"` and `"after"` are written to `before.lock` and `after.lock`, then
  `main(["--lock-guard", ...])`. `"trimmed-conflicts"`: `main(["--trimmed-conflicts"])` with
  `"status"` on stdin. `"removed-paths"`: `main(["--removed-paths"])`.

Result: `{"exit", "stdout", "stderr"}` and, for `trim` and `prim`, `"files"` in the testkit
tree form.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import os
import stat
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def _load_trim():
    spec = importlib.util.spec_from_file_location(
        "difftest_rtk_trim_script", str(ROOT / "scripts" / "rtk_trim.py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


rtk_trim = _load_trim()


def touched_paths() -> list:
    """The files `sub`, `cut` and `drop` edit, in first-use order."""
    seen: list = []

    class Recorder(rtk_trim.Trim):
        def __init__(self):
            super().__init__(".")

        def _note(self, path):
            if path not in seen:
                seen.append(path)

        def rm(self, *paths):
            pass

        def sub(self, path, *args, **kwargs):
            self._note(path)

        def cut(self, path, *args, **kwargs):
            self._note(path)

        def drop(self, path, *args, **kwargs):
            self._note(path)

    rtk_trim.operations(Recorder())
    return seen


def mutate(text: str, m: dict) -> str:
    kind = m["kind"]
    if kind == "replace":
        return text.replace(m["find"], m["with"]) if m["find"] else text
    if kind == "append":
        return text + m["text"]
    lines = text.split("\n")
    i = m["line"]
    if kind == "insert":
        lines.insert(min(i, len(lines)), m["text"])
    elif kind == "delete_line":
        if i < len(lines):
            del lines[i]
    elif kind == "dup_line":
        if i < len(lines):
            lines.insert(i, lines[i])
    else:
        raise ValueError("unknown mutation kind %r" % (kind,))
    return "\n".join(lines)


def _ensure_dir(base: str, rel_parent: str) -> None:
    cur = base
    for part in [p for p in rel_parent.split("/") if p]:
        cur = os.path.join(cur, part)
        if not os.path.isdir(cur):
            os.mkdir(cur)
            os.chmod(cur, 0o755)


def _write(base: str, rel: str, text: str) -> None:
    _ensure_dir(base, rel.rpartition("/")[0])
    full = os.path.join(base, rel)
    with open(full, "wb") as f:
        f.write(text.encode("utf-8"))
    os.chmod(full, 0o644)


def _snapshot(base: str, hashed: bool) -> dict:
    out: dict = {}

    def walk(path: str, rel: str) -> None:
        for name in sorted(os.listdir(path)):
            full = os.path.join(path, name)
            key = rel + name
            st = os.lstat(full)
            if stat.S_ISLNK(st.st_mode):
                out[key] = {"type": "symlink", "target": os.readlink(full)}
            elif stat.S_ISDIR(st.st_mode):
                out[key] = {"type": "dir", "mode": "%04o" % (st.st_mode & 0o7777)}
                walk(full, key + "/")
            else:
                with open(full, "rb") as f:
                    data = f.read()
                entry = {"type": "file", "mode": "%04o" % (st.st_mode & 0o7777)}
                if hashed:
                    entry["text"] = "sha256:" + hashlib.sha256(data).hexdigest()
                else:
                    entry["text"] = data.decode("utf-8")
                out[key] = entry

    walk(base, "")
    return out


def _main(argv: list, stdin_text: str = "") -> tuple:
    out, err = io.StringIO(), io.StringIO()
    old_stdin = sys.stdin
    sys.stdin = io.StringIO(stdin_text)
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = rtk_trim.main(argv)
    finally:
        sys.stdin = old_stdin
    return code, out.getvalue(), err.getvalue()


def _prim(case: dict) -> tuple:
    t = rtk_trim.Trim("rtk")
    try:
        for op in case["ops"]:
            kind = op["op"]
            if kind == "rm":
                t.rm(*op["paths"])
            elif kind == "sub":
                t.sub(op["path"], op["old"], op["new"], op["done"])
            elif kind == "cut":
                t.cut(op["path"], op["old"], op["gone"])
            elif kind == "drop":
                t.drop(op["path"], op["sig"], op["gone"])
            else:
                raise ValueError("unknown op %r" % (kind,))
    except rtk_trim.TrimError as error:
        return 1, "", "rtk-trim: %s\n" % (error,)
    return 0, "".join(entry + "\n" for entry in t.changed), ""


def _run_in(tmp: str, case: dict) -> dict:
    mode = case["mode"]
    files = None
    if mode == "removed-paths":
        code, out, err = _main(["--removed-paths"])
    elif mode == "trimmed-conflicts":
        code, out, err = _main(["--trimmed-conflicts"], case["status"])
    elif mode == "lock-guard":
        for name in ("before", "after"):
            with open(os.path.join(tmp, name + ".lock"), "wb") as f:
                f.write(case[name].encode("utf-8"))
        code, out, err = _main(["--lock-guard", "before.lock", "after.lock"])
    elif mode == "prim":
        os.mkdir("rtk")
        for rel, text in case["files"].items():
            _write("rtk", rel, text)
        code, out, err = _prim(case)
        files = _snapshot("rtk", False)
    elif mode == "trim":
        os.mkdir("rtk")
        base = {}
        for rel in touched_paths():
            src = ROOT / "rtk" / rel
            if src.is_file():
                base[rel] = src.read_text(encoding="utf-8")
        for m in case["mutations"]:
            if m["path"] in base:
                base[m["path"]] = mutate(base[m["path"]], m)
        for rel, text in base.items():
            _write("rtk", rel, text)
        for rel, text in case["extras"].items():
            _write("rtk", rel, text)
        for rel, target in case["links"].items():
            _ensure_dir("rtk", rel.rpartition("/")[0])
            os.symlink(target, os.path.join("rtk", rel))
        code, out, err = _main(["--root", "rtk"])
        files = _snapshot("rtk", True)
    else:
        raise ValueError("unknown mode %r" % (mode,))
    result = {"exit": code, "stdout": out, "stderr": err}
    if files is not None:
        result["files"] = files
    return result


def run(case: dict) -> dict:
    here = os.getcwd()
    with tempfile.TemporaryDirectory() as tmp:
        os.chdir(tmp)
        try:
            return _run_in(tmp, case)
        finally:
            os.chdir(here)


if __name__ == "__main__":
    probe = run({"mode": "removed-paths"})
    assert probe["exit"] == 0 and "engine" in probe["stdout"].split("\n"), probe
    print("dev_rtk_trim oracle ok")
