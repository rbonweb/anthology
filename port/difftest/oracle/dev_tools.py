"""DV2 oracle for the dev.version-bump, dev.release-edit, dev.memory-diff, dev.startup-budget and
dev.eval-baseline difftest surfaces (plan DV2).

Run by F4's driver (`python3 -I port/difftest/oracle/_driver.py`), which loads this module by
file path; `json.py` beside this file shadows the standard library's `json`, so this folder is
never on `sys.path`.

Library surface: `run(case: dict) -> dict`. The case layout is documented in
`crates/difftest/src/surfaces/dev_dv2.rs`. In short:

- `{"op": "cli", "tool": T, "args": [...], "files": {rel: text}, "env": {k: v}, "show": [rel]}`
  writes the files into a fresh temporary folder, makes it the working folder, sets the
  environment to PATH, HOME (the folder), LANG=C.UTF-8 plus `env`, and runs the script's
  `main(args)` in this process with `sys.argv[0]` set to the script's name (argparse's `prog`).
  The result is the exit code, stdout and stderr; then, for each `show` path, a line
  `=== <rel> ===` and the file's text (or `<absent>`). An exception the script does not catch
  (Python prints a traceback) becomes stderr `anthology-dev <tool>: <Type>: <message>`, what the
  Rust port prints (ruling R10), marked D-1. eval-baseline's `cannot read the ...: <strerror>`
  is written `...: errno N`, the Rust port's text until P1's errno tables (ruling R3), marked D-2.
- Library ops (`parse`, `compare`, `changed`, `verdict`, `range`, `parse_ns`, `escape`,
  `render`, `sections`, `split`) call one function and print a fixed rendering of its result
  (`repr`), or exit 1 with `<Type>: <message>`.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "scripts"

_FILES = {
    "version-bump": "check_version_bump",
    "release-edit": "release_edit",
    "memory-diff": "memory_diff",
    "startup-budget": "startup_budget",
    "eval-baseline": "eval_baseline",
}
_MODS = {}


def _mod(tool):
    name = _FILES[tool]
    mod = _MODS.get(name)
    if mod is None:
        spec = importlib.util.spec_from_file_location(name, str(SCRIPTS / (name + ".py")))
        mod = importlib.util.module_from_spec(spec)
        sys.modules[name] = mod  # release_edit imports check_version_bump by name
        spec.loader.exec_module(mod)
        _MODS[name] = mod
    return mod


def _exc_line(exc):
    """`<Type>: <str(exc)>` with the bare class name (`release_edit.UnsupportedRange` is
    `UnsupportedRange`), as the Rust side names it."""
    return f"{type(exc).__name__}: {exc}"


def _result(code, out, err, mark=None):
    r = {"exit": code}
    for key, text in (("stdout", out), ("stderr", err)):
        data = text.encode("utf-8", "surrogateescape")
        try:
            r[key] = data.decode("utf-8")
        except UnicodeDecodeError:
            r[key + "_hex"] = data.hex()
    if mark:
        r["divergence"] = mark
    return r


def _rooted(text, tmp):
    """`tmp` and its realpath (macOS: /var → /private/var) written `<ROOT>`."""
    for root in (os.path.realpath(tmp), tmp):  # the longer one first
        text = text.replace(root, "<ROOT>")
    return text


_CANNOT_READ = "eval_baseline: cannot read the "


def _strerror_numbered(err):
    """eval-baseline's `cannot read the <what> <path>: <strerror>` with the OS text written
    `errno N`, what the Rust port prints until P1 commits the errno tables (ruling R3). Returns
    the text and whether it changed (a D-2 mark)."""
    numbers = {}
    for n in range(1, 256):
        numbers.setdefault(os.strerror(n), n)
    lines, changed = [], False
    for line in err.split("\n"):
        head, sep, tail = line.rpartition(": ")
        if line.startswith(_CANNOT_READ) and sep and tail in numbers:
            line, changed = f"{head}: errno {numbers[tail]}", True
        lines.append(line)
    return "\n".join(lines), changed


def _cli(case):
    tool = case["tool"]
    mod = _mod(tool)
    tmp = tempfile.mkdtemp(prefix="dv2-oracle-")
    old_cwd = os.getcwd()
    old_env = dict(os.environ)
    old_argv = sys.argv
    out, err = io.StringIO(), io.StringIO()
    code, mark = 0, None
    try:
        for rel, text in (case.get("files") or {}).items():
            path = Path(tmp) / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode("utf-8"))
        os.chdir(tmp)
        os.environ.clear()
        os.environ.update({"PATH": old_env.get("PATH", "/usr/bin:/bin"), "HOME": tmp,
                           "LANG": "C.UTF-8"})
        os.environ.update(case.get("env") or {})
        sys.argv = [_FILES[tool] + ".py"]
        args = list(case.get("args") or [])
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                if tool == "memory-diff":
                    code = mod.main(args)
                elif tool == "startup-budget":
                    code = mod.main(args, stdout=out)
                else:
                    code = mod.main(args)
            except SystemExit as exc:
                code = exc.code if isinstance(exc.code, int) else 1
            except Exception as exc:  # noqa: BLE001 — Python's traceback: exit 1
                print(f"anthology-dev {tool}: {_exc_line(exc)}", file=err)
                code, mark = 1, "D-1"
        shown = []
        for rel in case.get("show") or []:
            path = Path(tmp) / rel
            shown.append(f"=== {rel} ===\n")
            shown.append(path.read_text(encoding="utf-8") if path.is_file() else "<absent>\n")
        text = out.getvalue() + "".join(shown)
        err_text = err.getvalue()
        if tool == "eval-baseline":
            err_text, numbered = _strerror_numbered(err_text)
            if numbered:
                mark = mark or "D-2"
        return _result(code or 0, _rooted(text, tmp), _rooted(err_text, tmp), mark)
    finally:
        sys.argv = old_argv
        os.environ.clear()
        os.environ.update(old_env)
        os.chdir(old_cwd)
        shutil.rmtree(tmp, ignore_errors=True)


_LIB_OPS = {"parse", "compare", "changed", "verdict", "range", "parse_ns", "escape", "render",
            "sections", "split"}


def _lib(case):
    op = case["op"]
    if op not in _LIB_OPS:
        raise ValueError(f"unknown op {op!r}")
    try:
        if op == "parse":
            value = repr(_mod("version-bump").parse_semver(case["v"]))
        elif op == "compare":
            value = repr(_mod("version-bump").compare_semver(case["a"], case["b"]))
        elif op == "changed":
            value = repr(_mod("version-bump").changed_plugins(case["paths"]))
        elif op == "verdict":
            value = repr(_mod("version-bump").verdict(case["plugin"], case["base"], case["head"],
                                                      case["base_exists"], case["head_exists"]))
        elif op == "range":
            value = repr(_mod("release-edit").range_accepts(case["rng"], case["v"]))
        elif op == "parse_ns":
            value = repr(_mod("memory-diff").parse_name_status(bytes.fromhex(case["hex"])))
        elif op == "escape":
            value = _mod("memory-diff").escape_name(case["s"])
        elif op == "render":
            value = _mod("memory-diff").render([(l, list(p)) for l, p in case["entries"]])
        elif op == "sections":
            value = repr(_mod("startup-budget").sections(case["body"]))
        elif op == "split":
            tmp = tempfile.mkdtemp(prefix="dv2-oracle-")
            try:
                path = Path(tmp) / "agent.md"
                path.write_bytes(case["text"].encode("utf-8"))
                value = repr(_mod("startup-budget").split_agent(path)).replace(tmp, "<ROOT>")
            except Exception as exc:  # noqa: BLE001
                return _result(1, "", _exc_line(exc).replace(tmp, "<ROOT>") + "\n")
            finally:
                shutil.rmtree(tmp, ignore_errors=True)
    except Exception as exc:  # noqa: BLE001
        return _result(1, "", _exc_line(exc) + "\n")
    return _result(0, value + "\n", "")


def run(case):
    if case.get("op") == "cli":
        return _cli(case)
    return _lib(case)
