"""MI1 oracle for the `mem` difftest surface (plan MI1, GC-12, D-5).

Run by F4's driver (`python3 -I port/difftest/oracle/_driver.py`), which loads this module by
file path; `json.py` beside this file shadows the standard library's `json`, so this folder is
never on `sys.path` (the plugin's `scripts/` folder is put there, for `memory_index`'s own
imports).

Library surface: `run(case: dict) -> dict`. The case layout is documented in
`crates/difftest/src/surfaces/mem.rs`. In short, `{"op": "cli", "data": D, "files": {rel: text},
"steps": [...]}`: a fresh temporary folder `<ROOT>` holds `home/` (HOME; the config folder is
`home/.claude`), `data/` and `plain/` (the working folder); `files` are written under it. `D` is
where the data folder is named: `state` (state.json's `data_dir`), `env` (CLAUDE_PLUGIN_DATA),
`none`, or `int` (`data_dir: 5`). Each step is `{"args": [...]}` (`memory_index.main(args)` in
this process), `{"write": {rel: text}}` or `{"remove": [rel]}`. stdout is a transcript: for
each run, `$ <args>`, `exit N`, `--- stdout`, its stdout, `--- stderr`, its stderr; then
`=== db ===` and the index database's content (its schema rows for the index's own tables,
then every row of `files` without `mtime`, `nodes`, `refs`, `edges` and `notes_fts`, sorted),
as Rust renders it. `PRAGMA user_version` is left out: it is 1 here and 2 in Rust (F-11).

An exception `main` does not catch (Python prints a traceback) becomes stderr
`anthology-mem: <Type>: <message>` and exit 1, what the Rust port prints, marked D-1.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import glob
import io
import os
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
TABLES = ("files", "nodes", "refs", "edges", "notes_fts", "edges_src", "edges_dst", "nodes_name")
DUMPS = (
    ("files", "SELECT path, size, sha256 FROM files ORDER BY path"),
    ("nodes", "SELECT id, name, type, scope, file, description, updated FROM nodes ORDER BY id"),
    ("refs", "SELECT src, target, kind FROM refs ORDER BY src, target, kind"),
    ("edges", "SELECT src, dst, kind FROM edges ORDER BY src, dst, kind"),
    ("fts", "SELECT id, name, description, body FROM notes_fts ORDER BY id"),
)


def _index():
    if str(SCRIPTS) not in sys.path:
        sys.path.insert(0, str(SCRIPTS))
    import memory_index
    # The Rust port always has FTS5; a sqlite3 without it would compare the LIKE fallback.
    if not memory_index._fts5_available(sqlite3.connect(":memory:")):
        raise RuntimeError("this python's sqlite3 has no FTS5")
    return memory_index


def _cell(v):
    if v is None:
        return "NULL"
    if isinstance(v, int):
        return str(v)
    return str(v).replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t")


def _dump(data):
    dbs = sorted(glob.glob(os.path.join(data, "memory-index", "*.db")))
    if not dbs:
        return "<no db>\n"
    lines = []
    for db in dbs:
        lines.append("db " + os.path.basename(db))
        conn = sqlite3.connect(db)
        try:
            marks = ",".join("?" * len(TABLES))
            for name, sql in conn.execute(
                    "SELECT name, sql FROM sqlite_master WHERE name IN (%s) ORDER BY name" % marks,
                    TABLES):
                lines.append("schema " + name + "\t" + _cell(sql))
            for label, sql in DUMPS:
                for row in conn.execute(sql):
                    lines.append(label + "\t" + "\t".join(_cell(v) for v in row))
        except sqlite3.Error as exc:
            lines.append("sqlite error: %s" % exc)
        finally:
            conn.close()
    return "\n".join(lines) + "\n"


def _write(tmp, files):
    for rel, text in (files or {}).items():
        path = Path(tmp) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode("utf-8"))


def _file_step(tmp, step):
    """Applies a `write`, `remove` or `touch` step; False for any other step."""
    if "write" in step:
        _write(tmp, step["write"])
    elif "remove" in step:
        for rel in step["remove"]:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(os.path.join(tmp, rel))
    elif "touch" in step:
        for rel in step["touch"]:
            path = os.path.join(tmp, rel)
            with contextlib.suppress(FileNotFoundError):
                st = os.stat(path)
                os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 10_000_000_000))
    else:
        return False
    return True


def _rows(label, rows, keys):
    return "".join("%s %s\n" % (label, "|".join(str(r[k]) for k in keys)) for r in rows)


def _build(case):
    """`build` and the queries, called directly with repo, agent and personal sources."""
    mod = _index()
    tmp = os.path.realpath(tempfile.mkdtemp(prefix="mi1-oracle-"))
    out = []
    try:
        _write(tmp, case.get("files"))
        sources = [(scope, Path(tmp) / rel) for scope, rel in case.get("sources") or []]
        db = Path(tmp) / (case.get("db") or "data/memory-index/test.db")
        for step in case.get("steps") or []:
            if _file_step(tmp, step):
                continue
            q = step["q"]
            try:
                if q[0] == "build":
                    st = mod.build(sources, db)
                    out.append("build added=%d updated=%d removed=%d rebuilt=%s\n" % (
                        st["added"], st["updated"], st["removed"], st["rebuilt"]))
                elif q[0] == "search":
                    out.append(_rows("hit", mod.search(q[1], db), ("name", "scope", "file", "snippet")))
                elif q[0] == "related":
                    out.append(_rows("rel", mod.related(q[1], q[2], db),
                                     ("distance", "name", "scope", "kind", "direction", "file")))
                elif q[0] == "orphans":
                    out.append(_rows("orphan", mod.orphans(db), ("name", "scope", "file")))
                elif q[0] == "stale":
                    out.append(_rows("stale", mod.stale(db, days=q[1], today=q[2]),
                                     ("name", "scope", "reason", "file")))
            except (ValueError, sqlite3.Error, OSError) as exc:
                out.append("error %s\n" % exc)
        out.append("=== db ===\n" + _dump(str(Path(tmp) / "data")))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return {"exit": 0, "stdout": "".join(out).replace(tmp, "<ROOT>"), "stderr": ""}


def _cli(case):
    mod = _index()
    tmp = os.path.realpath(tempfile.mkdtemp(prefix="mi1-oracle-"))
    old_cwd, old_env, old_argv = os.getcwd(), dict(os.environ), sys.argv
    mark = None
    transcript = []
    try:
        home, data, plain = (os.path.join(tmp, d) for d in ("home", "data", "plain"))
        for d in (home, data, plain):
            os.mkdir(d)
        anth = os.path.join(home, ".claude", "anthology")
        os.makedirs(anth)
        where = case.get("data", "state")
        if where == "state":
            Path(anth, "state.json").write_text('{"data_dir": "%s"}' % data, encoding="utf-8")
        elif where == "int":
            Path(anth, "state.json").write_text('{"data_dir": 5}', encoding="utf-8")
        _write(tmp, case.get("files"))
        os.chdir(plain)
        os.environ.clear()
        os.environ.update({"PATH": old_env.get("PATH", "/usr/bin:/bin"), "HOME": home,
                           "LANG": "C.UTF-8", "COLUMNS": "80"})
        if where == "env":
            os.environ["CLAUDE_PLUGIN_DATA"] = data
        sys.argv = ["memory_index.py"]
        for step in case.get("steps") or []:
            if _file_step(tmp, step):
                continue
            args = list(step.get("args") or [])
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                try:
                    code = mod.main(args)
                except SystemExit as exc:
                    code = exc.code if isinstance(exc.code, int) else 1
                except Exception as exc:  # noqa: BLE001 — Python's traceback: exit 1
                    print("anthology-mem: %s: %s" % (type(exc).__name__, exc), file=err)
                    code, mark = 1, "D-1"
            transcript.append("$ %s\nexit %d\n--- stdout\n%s--- stderr\n%s"
                              % (" ".join(args), code, out.getvalue(), err.getvalue()))
        transcript.append("=== db ===\n" + _dump(data))
    finally:
        os.chdir(old_cwd)
        os.environ.clear()
        os.environ.update(old_env)
        sys.argv = old_argv
        shutil.rmtree(tmp, ignore_errors=True)
    text = "".join(transcript).replace(tmp, "<ROOT>")
    result = {"exit": 0, "stdout": text, "stderr": ""}
    if mark:
        result["divergence"] = mark
    return result


def run(case):
    if case.get("op") == "cli":
        return _cli(case)
    if case.get("op") == "build":
        return _build(case)
    raise ValueError("unknown op %r" % (case.get("op"),))
