"""Anthology memory index: a derived SQLite graph + full-text search over the memory notes.

The Markdown notes are the source of truth; this database is a catalog built from them. It
lives in the plugin data dir (never in a repo), carries a schema version, and is rebuilt
whenever that version changes. It is also the home of the small frontmatter parser that
`memory_sync.py` shares.

Commands: search, related, orphans, stale, rebuild, and `save` (write an agent memory note
without a heredoc; needs no setup and no index).

Python stdlib only (3.9+). Run through `bin/anthology-mem`.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import sqlite3
import sys
from pathlib import Path

SCHEMA_VERSION = 1
RELATION_KINDS = ("supersedes", "depends_on", "relates_to")

# ---------------------------------------------------------------------------------------------
# Notes: frontmatter parsing
# ---------------------------------------------------------------------------------------------


def _scalar(value):
    """Strip one pair of matching quotes (a double-quoted `\\n` becomes a space-like newline)."""
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        inner = value[1:-1]
        if value[0] == "'":
            return inner.replace("''", "'")
        return inner.replace("\\n", "\n").replace('\\"', '"').replace("\\\\", "\\")
    return value


def _flow_list(value):
    """`[a, "b"]` -> ['a', 'b']. An unclosed bracket is kept as a plain string."""
    value = value.strip()
    if not value.endswith("]"):
        return value
    inner = value[1:-1]
    return [_scalar(p) for p in inner.split(",") if p.strip()]


def _is_item(text):
    return text == "-" or text.startswith("- ")


def _parse_block(lines, i, indent):
    """Parse a block of (indent, text) lines into a dict or a list; returns (value, next index)."""
    if _is_item(lines[i][1]):
        items = []
        while i < len(lines) and lines[i][0] == indent and _is_item(lines[i][1]):
            items.append(_scalar(lines[i][1][1:]))
            i += 1
        return items, i
    out = {}
    while i < len(lines):
        ind, text = lines[i]
        if ind < indent:
            break
        if ind > indent or _is_item(text):
            i += 1
            continue
        key, sep, val = text.partition(":")
        i += 1
        if not sep:
            continue
        key = _scalar(key)
        val = val.strip()
        if val == "":
            nxt = lines[i] if i < len(lines) else None
            if nxt and (nxt[0] > indent or (nxt[0] == indent and _is_item(nxt[1]))):
                out[key], i = _parse_block(lines, i, nxt[0])
            else:
                out[key] = ""
        elif val[0] in ">|" and not val[1:].strip("+-0123456789"):
            parts = []
            while i < len(lines) and lines[i][0] > indent:
                parts.append(lines[i][1])
                i += 1
            out[key] = " ".join(parts)
        elif val.startswith("["):
            out[key] = _flow_list(val)
        else:
            out[key] = _scalar(val)
    return out, i


def _parse_yaml(head):
    lines = []
    for raw in head.split("\n"):
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        lines.append((len(raw) - len(raw.lstrip(" ")), stripped))
    if not lines:
        return {}
    value, _ = _parse_block(lines, 0, lines[0][0])
    return value if isinstance(value, dict) else {}


def _text(value):
    return value.strip() if isinstance(value, str) else ""


def _targets(value):
    if isinstance(value, str):
        value = [value]
    if not isinstance(value, list):
        return []
    return [t for t in (_text(v) for v in value) if t]


_FRONT_END = re.compile(r"^---[ \t]*$", re.M)
_LINK = re.compile(r"\[\[([^\[\]|#\n]+)(?:[|#][^\[\]\n]*)?\]\]")


def extract_links(body):
    """Targets of `[[note]]` links in a note body (order kept, duplicates dropped)."""
    seen = []
    for m in _LINK.finditer(body):
        target = m.group(1).strip()
        if target and target not in seen:
            seen.append(target)
    return seen


def parse_text(text, stem="note"):
    """Parse a note's text. Never raises: broken frontmatter yields a note listed by stem."""
    info = {"name": stem, "description": "", "type": "other", "updated": "", "relations": {},
            "body": text, "stem": stem, "ok": False}
    if text[:1] == chr(0xFEFF):  # byte-order mark
        text = text[1:]
    text = text.replace("\r\n", "\n")
    info["body"] = text
    if not text.startswith("---\n"):
        return info
    rest = text[4:]
    end = _FRONT_END.search(rest)
    if not end:
        return info
    info["body"] = rest[end.end():].lstrip("\n")
    try:
        data = _parse_yaml(rest[:end.start()])
        meta = data.get("metadata")
        meta = meta if isinstance(meta, dict) else {}
        info["name"] = _text(data.get("name")) or stem
        info["description"] = _text(data.get("description"))
        info["type"] = _text(meta.get("type")) or _text(data.get("type")) or "other"
        info["updated"] = (_text(meta.get("modified")) or _text(meta.get("updated"))
                           or _text(data.get("modified")) or _text(data.get("updated")))
        relations = meta.get("relations")
        if isinstance(relations, dict):
            info["relations"] = {_text(k): _targets(v) for k, v in relations.items()
                                 if _targets(v)}
        info["ok"] = True
    except Exception:  # a malformed note must never break indexing
        info.update({"name": stem, "description": "", "type": "other", "updated": "",
                     "relations": {}})
    return info


def parse_note(path):
    """Parse the note at `path` into a dict (name, description, type, updated, relations, body,
    stem, ok, path). Raises OSError only if the file cannot be read."""
    path = Path(path)
    text = path.read_bytes().decode("utf-8", errors="replace")
    info = parse_text(text, path.stem)
    info["path"] = str(path)
    return info


# ---------------------------------------------------------------------------------------------
# Database
# ---------------------------------------------------------------------------------------------

_SCHEMA = """
CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER, size INTEGER, sha256 TEXT);
CREATE TABLE nodes(id TEXT PRIMARY KEY, name TEXT, type TEXT, scope TEXT, file TEXT,
                   description TEXT, updated TEXT);
CREATE TABLE refs(src TEXT, target TEXT, kind TEXT);
CREATE TABLE edges(src TEXT, dst TEXT, kind TEXT);
CREATE INDEX edges_src ON edges(src);
CREATE INDEX edges_dst ON edges(dst);
CREATE INDEX nodes_name ON nodes(name);
"""


def _fts5_available(conn):
    try:
        conn.execute("CREATE VIRTUAL TABLE temp._fts5_probe USING fts5(x)")
        conn.execute("DROP TABLE temp._fts5_probe")
        return True
    except sqlite3.Error:
        return False


def _create_schema(conn):
    conn.executescript(_SCHEMA)
    if _fts5_available(conn):
        conn.execute("CREATE VIRTUAL TABLE notes_fts USING fts5(name, description, body, "
                     "id UNINDEXED, tokenize = 'porter unicode61')")
    else:
        conn.execute("CREATE TABLE notes_fts(name, description, body, id)")
    conn.execute("PRAGMA user_version = %d" % int(SCHEMA_VERSION))
    conn.commit()


def _remove_db_files(db_path):
    for suffix in ("", "-wal", "-shm", "-journal"):
        try:
            os.unlink(str(db_path) + suffix)
        except FileNotFoundError:
            pass


def _has_current_schema(db_path):
    try:
        conn = sqlite3.connect(str(db_path))
        try:
            version = conn.execute("PRAGMA user_version").fetchone()[0]
            if version != SCHEMA_VERSION:
                return False
            conn.execute("SELECT count(*) FROM files").fetchone()
            return True
        finally:
            conn.close()
    except sqlite3.Error:
        return False


def _is_within(path, directory):
    return path == directory or path.startswith(directory.rstrip(os.sep) + os.sep)


def _check_db_location(sources, db_path):
    db = os.path.realpath(str(db_path))
    for scope, directory in sources:
        src = os.path.realpath(str(directory))
        if _is_within(db, src):
            raise ValueError("the index database must not live inside a notes folder: %s" % db_path)
        if scope == "personal":
            continue  # R23a: HOME may itself be a git work tree (dotfiles); only repo notes matter
        probe = src
        while True:
            if os.path.lexists(os.path.join(probe, ".git")):
                if _is_within(db, probe):
                    raise ValueError("the index database must not live inside a git work tree: "
                                     "%s" % db_path)
                break
            parent = os.path.dirname(probe)
            if parent == probe:
                break
            probe = parent


def _scan(sources):
    """{absolute path: (id, scope, mtime_ns, size)} for every note file in the sources."""
    found = {}
    for scope, directory in sources:
        directory = Path(directory)
        if not directory.is_dir():
            continue
        prefix = directory.name + "/" if scope == "agent" else ""
        for entry in sorted(os.scandir(str(directory)), key=lambda e: e.name):
            if not entry.name.endswith(".md") or entry.name == "MEMORY.md":
                continue
            try:
                if entry.is_symlink() or not entry.is_file(follow_symlinks=False):
                    continue
                st = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            path = os.path.abspath(entry.path)
            found[path] = ("%s:%s%s" % (scope, prefix, entry.name), scope, st.st_mtime_ns, st.st_size)
    return found


def _drop_node(conn, node_id):
    conn.execute("DELETE FROM notes_fts WHERE id = ?", (node_id,))
    conn.execute("DELETE FROM refs WHERE src = ?", (node_id,))
    conn.execute("DELETE FROM nodes WHERE id = ?", (node_id,))


def _store_node(conn, node_id, scope, path, info):
    _drop_node(conn, node_id)
    conn.execute("INSERT INTO nodes(id, name, type, scope, file, description, updated) "
                 "VALUES (?, ?, ?, ?, ?, ?, ?)",
                 (node_id, info["name"], info["type"], scope, path, info["description"],
                  info["updated"]))
    conn.execute("INSERT INTO notes_fts(name, description, body, id) VALUES (?, ?, ?, ?)",
                 (info["name"], info["description"], info["body"], node_id))
    for target in extract_links(info["body"]):
        conn.execute("INSERT INTO refs(src, target, kind) VALUES (?, ?, 'links')", (node_id, target))
    for kind in RELATION_KINDS:
        for target in info["relations"].get(kind, []):
            conn.execute("INSERT INTO refs(src, target, kind) VALUES (?, ?, ?)",
                         (node_id, target, kind))


def _rebuild_edges(conn):
    nodes = conn.execute("SELECT id, name, scope, file FROM nodes").fetchall()
    by_name, by_stem = {}, {}
    for node_id, name, scope, file in nodes:
        by_name.setdefault(name, []).append((node_id, scope))
        by_stem.setdefault(os.path.splitext(os.path.basename(file))[0], []).append((node_id, scope))
    scope_of = {n[0]: n[2] for n in nodes}

    def resolve(src, target):
        candidates = by_name.get(target) or by_stem.get(target)
        if not candidates:
            return None
        same = [c for c in candidates if c[1] == scope_of[src]]
        return sorted(same or candidates)[0][0]

    edges = set()
    for src, target, kind in conn.execute("SELECT src, target, kind FROM refs").fetchall():
        if src not in scope_of:
            continue
        dst = resolve(src, target)
        if dst == src:
            continue
        edges.add((src, dst if dst is not None else target, kind))
    conn.execute("DELETE FROM edges")
    conn.executemany("INSERT INTO edges(src, dst, kind) VALUES (?, ?, ?)", sorted(edges))


def build(sources, db_path):
    """Bring the index at `db_path` up to date with the notes in `sources`.

    `sources` is a list of `(scope, directory)` with scope `repo`, `agent` or `personal`
    (each agent folder passed separately). Incremental by mtime/size then sha256; fully
    rebuilt when the schema version changed or the file is not a usable database.
    Returns `{"added", "updated", "removed", "rebuilt"}`.
    """
    sources = [(scope, Path(d)) for scope, d in sources]
    _check_db_location(sources, db_path)
    db_path = Path(db_path)
    db_path.parent.mkdir(parents=True, exist_ok=True)
    rebuilt = False
    if db_path.exists() and not _has_current_schema(db_path):
        _remove_db_files(db_path)
        rebuilt = True
    stats = {"added": 0, "updated": 0, "removed": 0, "rebuilt": rebuilt}
    conn = sqlite3.connect(str(db_path))
    try:
        if not conn.execute("SELECT 1 FROM sqlite_master WHERE name = 'files'").fetchone():
            _create_schema(conn)
        disk = _scan(sources)
        known = {p: (m, s, h) for p, m, s, h in
                 conn.execute("SELECT path, mtime, size, sha256 FROM files")}
        with conn:
            for path in sorted(set(known) - set(disk)):
                for (node_id,) in conn.execute("SELECT id FROM nodes WHERE file = ?",
                                               (path,)).fetchall():
                    _drop_node(conn, node_id)
                conn.execute("DELETE FROM files WHERE path = ?", (path,))
                stats["removed"] += 1
            for path in sorted(disk):
                node_id, scope, mtime, size = disk[path]
                old = known.get(path)
                if old and old[0] == mtime and old[1] == size:
                    continue
                try:
                    data = Path(path).read_bytes()
                except OSError:
                    continue
                sha = hashlib.sha256(data).hexdigest()
                if old and old[2] == sha:
                    conn.execute("UPDATE files SET mtime = ?, size = ? WHERE path = ?",
                                 (mtime, size, path))
                    continue
                info = parse_text(data.decode("utf-8", errors="replace"),
                                  os.path.splitext(os.path.basename(path))[0])
                _store_node(conn, node_id, scope, path, info)
                conn.execute("INSERT OR REPLACE INTO files(path, mtime, size, sha256) "
                             "VALUES (?, ?, ?, ?)", (path, mtime, size, sha))
                stats["updated" if old else "added"] += 1
            if stats["added"] or stats["updated"] or stats["removed"]:
                _rebuild_edges(conn)
    finally:
        conn.close()
    return stats


# ---------------------------------------------------------------------------------------------
# Queries
# ---------------------------------------------------------------------------------------------


def _open(db_path):
    if not os.path.exists(str(db_path)):
        return None
    return sqlite3.connect(str(db_path))


def _is_virtual(conn):
    row = conn.execute("SELECT sql FROM sqlite_master WHERE name = 'notes_fts'").fetchone()
    return bool(row and "VIRTUAL" in (row[0] or "").upper())


def _like_escape(term):
    return term.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def _excerpt(text, terms, width=80):
    flat = " ".join(text.split())
    low = flat.lower()
    for term in terms:
        at = low.find(term.lower())
        if at >= 0:
            start = max(0, at - width // 2)
            return ("…" if start else "") + flat[start:start + width] + ("…" if start + width < len(flat) else "")
    return flat[:width]


def search(q, db_path, limit=20):
    """Notes matching every whitespace-separated term of `q` (best first). Each result is a
    dict with `name`, `scope`, `file`, `snippet`. User input is quoted, never FTS syntax."""
    terms = q.split()
    conn = _open(db_path)
    if conn is None or not terms:
        return []
    try:
        if _is_virtual(conn):
            match = " ".join('"%s"' % t.replace('"', '""') for t in terms)
            rows = conn.execute(
                "SELECT n.name, n.scope, n.file, snippet(notes_fts, -1, '', '', '…', 12) "
                "FROM notes_fts JOIN nodes n ON n.id = notes_fts.id "
                "WHERE notes_fts MATCH ? ORDER BY bm25(notes_fts), n.name, n.id LIMIT ?",
                (match, int(limit))).fetchall()
            return [{"name": r[0], "scope": r[1], "file": r[2],
                     "snippet": " ".join((r[3] or "").split())} for r in rows]
        where = " AND ".join("(f.name LIKE ? ESCAPE '\\' OR f.description LIKE ? ESCAPE '\\' "
                             "OR f.body LIKE ? ESCAPE '\\')" for _ in terms)
        args = []
        for t in terms:
            args += ["%" + _like_escape(t) + "%"] * 3
        rows = conn.execute(
            "SELECT n.name, n.scope, n.file, f.body, f.description, f.name "
            "FROM notes_fts f JOIN nodes n ON n.id = f.id WHERE " + where +
            " ORDER BY n.name, n.id LIMIT ?", args + [int(limit)]).fetchall()
        out = []
        for name, scope, file, body, desc, fname in rows:
            hay = body if any(t.lower() in body.lower() for t in terms) else (desc or fname)
            out.append({"name": name, "scope": scope, "file": file, "snippet": _excerpt(hay, terms)})
        return out
    finally:
        conn.close()


def related(name, depth=2, db_path=None):
    """Notes reachable from `name` within `depth` hops, following edges in both directions.
    Each result: `name`, `scope`, `file`, `kind` (of the edge that reached it), `distance`,
    `direction` (`out` if the walk followed the edge, `in` if it went against it)."""
    conn = _open(db_path)
    if conn is None or depth < 1:
        return []
    try:
        nodes = {r[0]: r for r in conn.execute("SELECT id, name, scope, file FROM nodes")}
        adj = {}
        for src, dst, kind in conn.execute("SELECT src, dst, kind FROM edges"):
            if src in nodes and dst in nodes:
                adj.setdefault(src, []).append((dst, kind, "out"))
                adj.setdefault(dst, []).append((src, kind, "in"))
        start = [i for i, n in nodes.items()
                 if n[1] == name or os.path.splitext(os.path.basename(n[3]))[0] == name]
        seen = {i: 0 for i in start}
        found = []
        frontier = sorted(start)
        for distance in range(1, int(depth) + 1):
            nxt = []
            for node in frontier:
                for other, kind, direction in sorted(adj.get(node, [])):
                    if other not in seen:
                        seen[other] = distance
                        nxt.append(other)
                        found.append({"name": nodes[other][1], "scope": nodes[other][2],
                                      "file": nodes[other][3], "kind": kind,
                                      "distance": distance, "direction": direction})
            frontier = nxt
        found.sort(key=lambda r: (r["distance"], r["name"], r["file"]))
        return found
    finally:
        conn.close()


def orphans(db_path):
    """Notes that no other note links to (no incoming resolved edge)."""
    conn = _open(db_path)
    if conn is None:
        return []
    try:
        rows = conn.execute(
            "SELECT n.name, n.scope, n.file FROM nodes n WHERE NOT EXISTS ("
            "SELECT 1 FROM edges e JOIN nodes s ON s.id = e.src WHERE e.dst = n.id) "
            "ORDER BY n.name, n.id").fetchall()
        return [{"name": r[0], "scope": r[1], "file": r[2]} for r in rows]
    finally:
        conn.close()


def _as_date(value):
    if value is None:
        return datetime.date.today()
    if isinstance(value, datetime.datetime):
        return value.date()
    if isinstance(value, datetime.date):
        return value
    return datetime.date.fromisoformat(str(value)[:10])


def stale(db_path, days=180, today=None):
    """Notes that a `supersedes` relation points at, or whose date is older than `days`.
    Each result: `name`, `scope`, `file`, `reason`."""
    conn = _open(db_path)
    if conn is None:
        return []
    try:
        cutoff = _as_date(today) - datetime.timedelta(days=int(days))
        out = {}
        for r in conn.execute(
                "SELECT DISTINCT d.id, d.name, d.scope, d.file FROM edges e "
                "JOIN nodes s ON s.id = e.src JOIN nodes d ON d.id = e.dst "
                "WHERE e.kind = 'supersedes'"):
            out[r[0]] = {"name": r[1], "scope": r[2], "file": r[3], "reason": "superseded"}
        for node_id, name, scope, file, updated in conn.execute(
                "SELECT id, name, scope, file, updated FROM nodes"):
            if node_id in out or not updated:
                continue
            try:
                when = datetime.date.fromisoformat(updated[:10])
            except ValueError:
                continue
            if when < cutoff:
                out[node_id] = {"name": name, "scope": scope, "file": file,
                                "reason": "older than %d days (%s)" % (days, updated[:10])}
        return sorted(out.values(), key=lambda r: (r["name"], r["file"]))
    finally:
        conn.close()


# ---------------------------------------------------------------------------------------------
# Locating the database, and the anthology-mem CLI
# ---------------------------------------------------------------------------------------------


def db_path_for(data_dir, key):
    """`<data_dir>/memory-index/<slug>-<sha256(key)[:12]>.db`; the slug is filesystem-safe."""
    parts = [p for p in key.split("/") if p]
    slug = re.sub(r"[^a-z0-9._-]", "-", "-".join(parts[-2:]).lower())[:40].lstrip(".-") or "repo"
    digest = hashlib.sha256(key.encode("utf-8")).hexdigest()[:12]
    return Path(data_dir) / "memory-index" / ("%s-%s.db" % (slug, digest))


def _print_rows(rows, as_json, line):
    if as_json:
        print(json.dumps(rows, indent=2, sort_keys=True))
        return
    if not rows:
        print("(none)")
    for row in rows:
        print(line(row))


def _parser():
    p = argparse.ArgumentParser(prog="anthology-mem",
                                description="Search and explore Anthology memory notes.")
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("search", help="full-text search over note bodies")
    s.add_argument("terms", nargs="+")
    r = sub.add_parser("related", help="notes linked to a note, to a depth")
    r.add_argument("note")
    r.add_argument("--depth", type=int, default=2)
    sub.add_parser("orphans", help="notes nothing links to")
    st = sub.add_parser("stale", help="superseded or old notes")
    st.add_argument("--days", type=int, default=180)
    sub.add_parser("rebuild", help="delete and rebuild the index")
    sv = sub.add_parser("save", allow_abbrev=False,
                        help="save an agent memory note (no heredoc needed)")
    for flag in ("agent", "type", "slug", "name", "description", "body"):
        sv.add_argument("--" + flag, required=True)
    sv.add_argument("--replace", action="store_true", help="overwrite an existing note")
    for sp in sub.choices.values():
        sp.add_argument("--json", action="store_true", help="machine-readable output")
    return p


def _save(args):
    import memory_sync

    root = memory_sync.repo_root(os.getcwd())
    if root is None:
        print("anthology-mem: not inside a git work tree; memory notes live in the repo",
              file=sys.stderr)
        return 1
    try:
        out = memory_sync.save_note(root, args.agent, args.type, args.slug, args.name,
                                    args.description, args.body, replace=args.replace)
    except memory_sync.SaveError as exc:
        print("anthology-mem: %s" % exc, file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps({"action": out["action"], "path": out["relative"],
                          "root": str(out["root"])}, sort_keys=True))
    else:
        print("%s %s in %s" % (out["action"], out["relative"], out["root"]))
    if out["index_warning"]:
        print("anthology-mem: note saved, but MEMORY.md was not regenerated: %s"
              % out["index_warning"], file=sys.stderr)
    return 0


def main(argv=None):
    args = _parser().parse_args(argv)
    if args.command == "save":  # needs no setup, data dir or index
        return _save(args)
    import state  # lazy: the CLI is the only caller
    import memory_sync

    data_dir = state.read().get("data_dir") or os.environ.get("CLAUDE_PLUGIN_DATA")
    if not data_dir:
        print("anthology-mem: run /anthology:setup first", file=sys.stderr)
        return 1
    home = memory_sync.anthology_home()
    root = memory_sync.repo_root(os.getcwd())
    if root and memory_sync.is_approved(memory_sync.repo_key(root)):
        key = memory_sync.repo_key(root)
        sources = memory_sync.note_sources(root, home)
    else:
        key = "personal"
        sources = memory_sync.note_sources(None, home)
    db = db_path_for(data_dir, key)
    try:
        if args.command == "rebuild":
            _remove_db_files(db)
        stats = build(sources, db)
        if args.command == "rebuild":
            if args.json:
                print(json.dumps(stats, sort_keys=True))
            else:
                print("rebuilt the index: %d notes" % stats["added"])
        elif args.command == "search":
            _print_rows(search(" ".join(args.terms), db), args.json,
                        lambda r: "%s [%s] %s\n    %s" % (r["name"], r["scope"], r["file"], r["snippet"]))
        elif args.command == "related":
            _print_rows(related(args.note, args.depth, db), args.json,
                        lambda r: "%d  %s [%s] (%s, %s) %s" % (r["distance"], r["name"], r["scope"],
                                                               r["kind"], r["direction"], r["file"]))
        elif args.command == "orphans":
            _print_rows(orphans(db), args.json, lambda r: "%s [%s] %s" % (r["name"], r["scope"], r["file"]))
        elif args.command == "stale":
            _print_rows(stale(db, days=args.days), args.json,
                        lambda r: "%s [%s] %s: %s" % (r["name"], r["scope"], r["reason"], r["file"]))
    except (ValueError, sqlite3.Error, OSError) as exc:
        print("anthology-mem: %s" % exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    import memory_index  # run the one shared module instance, not a second `__main__` copy

    sys.exit(memory_index.main())
