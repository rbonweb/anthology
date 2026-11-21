"""MX1 oracle for the `memory.portable` difftest surface (plan MX1, GC-12, GC-16).

Run by F4's driver (`python3 -I port/difftest/oracle/_driver.py`), which loads this module by
file path; this folder is never on `sys.path` (`json.py` here shadows the standard library's).
The plugin's `scripts/` folder is put there, for `memory_portable`'s own imports.

Library surface: `run(case: dict) -> dict`. The case layout and the transcript are documented
in `crates/difftest/src/surfaces/memory_portable.rs`; this file and the Rust side render the
same transcript. In short: a fresh folder `<ROOT>` holds two machines, `a/home` and `b/home`
(HOME; the config folder is `<m>/home/.claude`), and `plain/`, the working folder, which is
never in a git repository (the oracle image has no git at all), so every repo or agent note
takes the "not an approved repo" path. `files` (text) and `bytes` (hex) are written under
`<ROOT>`; archives built by the generator arrive as `bytes`, so both sides read the same file.
Each step is one call (`export`, `plan`, `apply`, `cli`) or a file change.

An exception that the called function does not turn into ImportRejected, ExportRefused,
ValueError or OSError (or, for `cli`, one that `main` does not catch: Python prints a
traceback) is printed as `error <Type>: <message>` (`cli`: stderr `Traceback: <Type>:
<message>`, exit 1) and marked D-1.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import copy
import datetime
import gzip
import hashlib
import io
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"


def _module():
    if str(SCRIPTS) not in sys.path:
        sys.path.insert(0, str(SCRIPTS))
    import memory_portable
    return memory_portable


def _env(tmp, machine):
    home = os.path.join(tmp, machine, "home")
    return {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": home, "LANG": "C.UTF-8"}


def _subst(value, tmp):
    """`<ROOT>` in every string of `value` replaced by the folder."""
    if isinstance(value, str):
        return value.replace("<ROOT>", tmp)
    if isinstance(value, list):
        return [_subst(v, tmp) for v in value]
    if isinstance(value, dict):
        return {k: _subst(v, tmp) for k, v in value.items()}
    return value


def _edit(plan, edits):
    """Each edit `[path, value]` sets, `[path]` deletes; a path that does not exist is skipped."""
    for edit in edits or []:
        path = edit[0]
        cur = plan
        try:
            for key in path[:-1]:
                cur = cur[key]
            if len(edit) > 1:
                cur[path[-1]] = edit[1]
            else:
                del cur[path[-1]]
        except (KeyError, IndexError, TypeError):
            continue
    return plan


def _write_files(tmp, files, binary):
    for rel, value in (files or {}).items():
        path = Path(tmp) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(bytes.fromhex(value) if binary else value.encode("utf-8"))


def _file_step(tmp, step):
    if "write" in step:
        _write_files(tmp, step["write"], False)
    elif "bytes" in step:
        _write_files(tmp, step["bytes"], True)
    elif "remove" in step:
        for rel in step["remove"]:
            with contextlib.suppress(OSError):
                os.unlink(os.path.join(tmp, rel))
    elif "symlink" in step:
        for rel, target in step["symlink"].items():
            path = Path(tmp) / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            os.symlink(target.replace("<ROOT>", tmp), str(path))
    elif "mkdir" in step:
        for rel in step["mkdir"]:
            os.makedirs(os.path.join(tmp, rel), exist_ok=True)
    else:
        return False
    return True


def _sha(data):
    return hashlib.sha256(data).hexdigest()


def _content(rel, data, today):
    if rel.startswith("in/"):
        return "sha256:" + _sha(data)
    if rel.endswith(".tar.gz"):
        try:
            tar = gzip.decompress(data)
        except Exception:  # noqa: BLE001 — not this side's own archive
            return "sha256:" + _sha(data)
        tar = tar.replace(today.encode(), b"<TODAY>")
        return "gzip:%s tar:%d:%s" % (data[:10].hex(), len(tar), _sha(tar))
    try:
        return "text:" + json.dumps(data.decode("utf-8"), ensure_ascii=False)
    except UnicodeDecodeError:
        return "hex:" + data.hex()


def _tree(tmp, today):
    lines = []
    entries = []
    for base, dirs, files in os.walk(tmp, followlinks=False):
        for name in dirs + files:
            entries.append(os.path.relpath(os.path.join(base, name), tmp))
    for rel in sorted(entries):
        path = os.path.join(tmp, rel)
        st = os.lstat(path)
        mode = "%04o" % (st.st_mode & 0o7777)
        if os.path.islink(path):
            lines.append("l %s -> %s" % (rel, os.readlink(path)))
        elif os.path.isdir(path):
            lines.append("d %s %s" % (mode, rel))
        elif os.path.isfile(path):
            with open(path, "rb") as fh:
                lines.append("f %s %s %s" % (mode, rel, _content(rel, fh.read(), today)))
        else:
            lines.append("o %s" % rel)
    return "".join(line + "\n" for line in lines)


def _call(fn, mp):
    """`(text, raised)`: the JSON result, or `error <Type>: <message>`; `raised` when the
    exception is not one the port names (D-1)."""
    try:
        res = fn()
    except (mp.ImportRejected, mp.ExportRefused, ValueError, OSError) as exc:
        return "error %s: %s\n" % (type(exc).__name__, exc), None, False
    except Exception as exc:  # noqa: BLE001 — Python raised: D-1
        return "error %s: %s\n" % (type(exc).__name__, exc), None, True
    return json.dumps(res, indent=2, sort_keys=True) + "\n", res, False


def run(case):
    mp = _module()
    tmp = os.path.realpath(tempfile.mkdtemp(prefix="mx1-oracle-"))
    old_cwd, old_env = os.getcwd(), dict(os.environ)
    today = datetime.date.today().isoformat()
    out, mark, last = [], None, None
    try:
        for d in ("a/home", "b/home", "plain"):
            os.makedirs(os.path.join(tmp, d))
        cwd = os.path.join(tmp, "plain")
        _write_files(tmp, case.get("files"), False)
        _write_files(tmp, case.get("bytes"), True)
        for n, step in enumerate(case.get("steps") or [], 1):
            if _file_step(tmp, step):
                continue
            machine = step.get("home", "a")
            env = _env(tmp, machine)
            if "export" in step:
                arg = step["export"]
                outdir = arg.get("out")
                outdir = None if outdir is None else os.path.join(tmp, outdir)
                text, _res, raised = _call(lambda: mp.export(cwd, outdir, today=arg.get("today"),
                                                             env=env), mp)
                out.append("## %d export\n%s" % (n, text))
            elif "plan" in step:
                arg = step["plan"]
                archive = os.path.join(tmp, arg["archive"])
                text, res, raised = _call(lambda: mp.plan_import(archive, cwd, today=arg.get("today"),
                                                                 env=env), mp)
                if res is not None:
                    last = res
                out.append("## %d plan\n%s" % (n, text))
            elif "apply" in step:
                arg = step["apply"]
                plan = _subst(arg["plan"], tmp) if "plan" in arg else copy.deepcopy(last)
                plan = _edit(plan, _subst(arg.get("edits"), tmp))
                text, _res, raised = _call(lambda: mp.apply_import(plan), mp)
                out.append("## %d apply\n%s" % (n, text))
            elif "cli" in step:
                args = _subst(list(step["cli"]), tmp)
                so, se = io.StringIO(), io.StringIO()
                raised = False
                os.chdir(cwd)
                os.environ.clear()
                os.environ.update(env)
                try:
                    with contextlib.redirect_stdout(so), contextlib.redirect_stderr(se):
                        try:
                            code = mp.main(args)
                        except SystemExit as exc:
                            code = exc.code if isinstance(exc.code, int) else 1
                        except Exception as exc:  # noqa: BLE001 — Python's traceback: exit 1
                            print("Traceback: %s: %s" % (type(exc).__name__, exc), file=se)
                            code, raised = 1, True
                finally:
                    os.chdir(old_cwd)
                    os.environ.clear()
                    os.environ.update(old_env)
                out.append("## %d $ %s\nexit %d\n--- stdout\n%s--- stderr\n%s"
                           % (n, " ".join(args), code, so.getvalue(), se.getvalue()))
            else:
                raise ValueError("unknown step %r" % (step,))
            if raised:
                mark = "D-1"
        out.append("=== tree ===\n" + _tree(tmp, today))
    finally:
        os.chdir(old_cwd)
        os.environ.clear()
        os.environ.update(old_env)
        shutil.rmtree(tmp, ignore_errors=True)
    text = "".join(out).replace(tmp, "<ROOT>").replace(today, "<TODAY>")
    result = {"exit": 0, "stdout": text, "stderr": ""}
    if mark:
        result["divergence"] = mark
    return result
