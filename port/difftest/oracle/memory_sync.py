"""MS2 oracle for the `memory.sync` difftest surface (plan MS2, GC-12).

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the
standard library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict`.

A case is one scenario in a fresh temporary folder `<tmp>` (its realpath), laid out as
`<tmp>/home` (`HOME`), `<tmp>/home/.claude` (`CLAUDE_CONFIG_DIR`) and `<tmp>/repo` (a work tree,
made without git as the tests' `fake_repo` makes it: `repo_root`, `repo_key`, `_main_root`,
`settings_tracked` and `ensure_ignored` are replaced, so no git runs):

- `dirs`: folders to create (relative to `<tmp>`); `files`: `{rel: text}`; `legacy`:
  `{name: text}` in the repo's old per-path folder `<config>/projects/<encoded repo>/memory`;
  `links`: `{rel: target rel}` symlinks to `<tmp>/<target rel>`; then, when `approved`, an
  `approved-repos.json` that approves `key`.
- `in_repo` (the cwd is inside the work tree), `tracked` (`settings.local.json` is tracked),
  `env` (added to `HOME` and `CLAUDE_CONFIG_DIR`), `cwd` (`repo` or `plain`).
- `op`:
  - `run`: `memory_sync.run(cwd, env)` -> `{"context", "messages"}`;
  - `migrate`: `migrate(<legacy dir>, <repo>/.claude/memory or None, <personal>, ledger
    <anthology>/migrated.json or None, today, promote, home=<tmp>/home)` -> its dict;
  - `sweep`: `sweep_user_notes(<tmp>/<src>, <personal>, today)` -> its dict;
  - `save`: `save_note(<tmp>/repo, ...)` with `env` -> `{"action", "relative", "root",
    "index_warning"}` or `{"error": str(SaveError)}`.

Result: `{"exit": 0, "stdout": json.dumps(result, sort_keys=True), "stderr": "", "files": tree}`
where `tree` is `<tmp>` in the testkit `Tree` JSON form. `<tmp>` is written as `<tmp>` and
today's date as `<today>` everywhere (stdout, file names, file texts, link targets). An
exception is exit 1 with stderr `"<type>: <str(exc)>"`.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import datetime
import importlib.util
import json
import os
import shutil
import stat
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"


def _load():
    # memory_sync imports memory_index and memory_privacy by name: the scripts folder goes at
    # the END of sys.path, so the standard library always wins.
    if str(SCRIPTS) not in sys.path:
        sys.path.append(str(SCRIPTS))
    spec = importlib.util.spec_from_file_location("memory_sync", str(SCRIPTS / "memory_sync.py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules["memory_sync"] = mod
    spec.loader.exec_module(mod)
    return mod


MS = _load()
PATCHED = ("repo_root", "repo_key", "_main_root", "settings_tracked", "ensure_ignored")


def _encode(path):
    import re
    return re.sub(r"[^A-Za-z0-9]", "-", os.path.realpath(str(path)))


def _norm(text, tmp, today):
    """`<tmp>` for the temporary folder (also in its encoded per-path form) and `<today>`."""
    return (text.replace(tmp, "<tmp>").replace(_encode(tmp), "<tmp-encoded>")
            .replace(today, "<today>"))


def _snapshot(base, tmp, today):
    out = {}

    def walk(path, rel):
        for name in sorted(os.listdir(path)):
            full = os.path.join(path, name)
            key = _norm(rel + name, tmp, today)
            st = os.lstat(full)
            if stat.S_ISLNK(st.st_mode):
                target = os.readlink(full)
                if target == tmp or target.startswith(tmp + "/"):
                    target = "<ROOT>" + target[len(tmp):]
                out[key] = {"type": "symlink", "target": _norm(target, tmp, today)}
            elif stat.S_ISDIR(st.st_mode):
                out[key] = {"type": "dir", "mode": "%04o" % (st.st_mode & 0o7777)}
                walk(full, rel + name + "/")
            elif stat.S_ISREG(st.st_mode):
                with open(full, "rb") as f:
                    data = f.read()
                entry = {"type": "file", "mode": "%04o" % (st.st_mode & 0o7777)}
                try:
                    entry["text"] = _norm(data.decode("utf-8"), tmp, today)
                except UnicodeDecodeError:
                    entry["hex"] = data.hex()
                out[key] = entry
            else:
                out[key] = {"type": "other", "kind": "fifo" if stat.S_ISFIFO(st.st_mode) else "unknown"}

    walk(base, "")
    return out


def _setup(case, tmp):
    home = os.path.join(tmp, "home")
    config = os.path.join(home, ".claude")
    root = os.path.join(tmp, "repo")
    os.makedirs(home)
    os.makedirs(root)
    for rel in case.get("dirs", []):
        os.makedirs(os.path.join(tmp, rel), exist_ok=True)
    for rel, text in case.get("files", {}).items():
        p = os.path.join(tmp, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)
    legacy = os.path.join(config, "projects", _encode(root), "memory")
    for name, text in case.get("legacy", {}).items():
        os.makedirs(legacy, exist_ok=True)
        with open(os.path.join(legacy, name), "w", encoding="utf-8") as f:
            f.write(text)
    for rel, target in case.get("links", {}).items():
        p = os.path.join(tmp, rel)
        if os.path.lexists(p):
            continue  # a file or folder of the case is already there
        os.makedirs(os.path.dirname(p), exist_ok=True)
        os.symlink(os.path.join(tmp, target), p)
    if case.get("approved"):
        anth = os.path.join(config, "anthology")
        os.makedirs(anth, exist_ok=True)
        data = {"version": 1, "repos": {case["key"]: {"approved": "2026-01-01"}}}
        with open(os.path.join(anth, "approved-repos.json"), "w", encoding="utf-8") as f:
            f.write(json.dumps(data, indent=2, sort_keys=True) + "\n")
    return home, config, root, legacy


def _op(case, tmp, home, config, root, legacy, env):
    op = case["op"]
    personal = Path(config) / "anthology" / "personal-memory"
    if op == "run":
        cwd = root if case.get("cwd", "repo") == "repo" else os.path.join(tmp, "plain")
        ctx, msgs = MS.run(Path(cwd), env)
        return {"context": ctx, "messages": msgs}
    if op == "migrate":
        res = MS.migrate(Path(legacy), Path(root) / ".claude" / "memory" if case.get("repo_mem") else None,
                         personal,
                         ledger=Path(config) / "anthology" / "migrated.json" if case.get("ledger") else None,
                         today=case["today"], promote=set(case.get("promote", [])), home=home)
        return res
    if op == "sweep":
        return MS.sweep_user_notes(Path(tmp) / case["src"], personal, today=case["today"])
    if op == "save":
        s = case["save"]
        try:
            out = MS.save_note(Path(root), s["agent"], s["type"], s["slug"], s["name"],
                               s["description"], s["body"], replace=s["replace"],
                               today=case["today"], env=env)
        except MS.SaveError as exc:
            return {"error": str(exc)}
        return {"action": out["action"], "relative": out["relative"], "root": str(out["root"]),
                "index_warning": out["index_warning"]}
    raise KeyError("unknown op %r" % (op,))


def run(case):
    """What Rust must produce for `case` (module docstring)."""
    tmp = os.path.realpath(tempfile.mkdtemp(prefix="memory-sync-"))
    today = datetime.date.today().isoformat()
    saved = {name: getattr(MS, name) for name in PATCHED}
    saved_home = os.environ.get("HOME")
    try:
        home, config, root, legacy = _setup(case, tmp)
        os.makedirs(os.path.join(tmp, "plain"), exist_ok=True)
        rootp = Path(root)
        in_repo, key, tracked = case.get("in_repo", True), case.get("key", ""), case.get("tracked", False)
        MS.repo_root = lambda cwd, env=None: rootp if in_repo else None
        MS.repo_key = lambda r, env=None: key
        MS._main_root = lambda r, env=None: rootp
        MS.settings_tracked = lambda r, env=None: tracked
        MS.ensure_ignored = lambda r, env=None: False
        os.environ["HOME"] = home
        env = dict(case.get("env", {}), HOME=home, CLAUDE_CONFIG_DIR=config)
        try:
            result = _op(case, tmp, home, config, root, legacy, env)
        except Exception as exc:  # the oracle reports what Python raised
            return {"exit": 1, "stdout": "",
                    "stderr": _norm("%s: %s" % (type(exc).__name__, exc), tmp, today),
                    "files": _snapshot(tmp, tmp, today)}
        out = _norm(json.dumps(result, sort_keys=True), tmp, today)
        return {"exit": 0, "stdout": out, "stderr": "", "files": _snapshot(tmp, tmp, today)}
    finally:
        for name, value in saved.items():
            setattr(MS, name, value)
        if saved_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = saved_home
        shutil.rmtree(tmp, ignore_errors=True)
