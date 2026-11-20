"""Anthology memory export/import: move notes between machines as a plain tar.gz.

Export packs the personal notes and, for an approved repo, the repo and agent notes. Import
is hostile-input safe: the archive is never extracted. Every member is validated before
anything is written, note content is read into memory (UTF-8 only) and written by this
module with exclusive creates, and only the three memory locations can be written. Nothing
is ever overwritten: a differing note is kept next to the existing one as
`<name>.imported-<date>.md`.

Python stdlib only (3.9+). Driven by the `export` / `import` skills.
"""
from __future__ import annotations

import argparse
import datetime
import gzip
import hashlib
import io
import json
import os
import re
import sys
import tarfile
import tempfile
from pathlib import Path

import memory_sync

MAX_ARCHIVE = 50 * 1024 * 1024      # the archive file
MAX_TOTAL = 50 * 1024 * 1024        # all member data together
MAX_MEMBERS = 10000
MAX_MANIFEST = 1024 * 1024
# What the gzip stream may expand to while it is read: member data, plus tar headers and
# padding for the most members allowed, plus slack. Bounds memory and time on a gzip bomb.
STREAM_LIMIT = MAX_TOTAL + MAX_MEMBERS * 1024 + 1024 * 1024
FORMAT = 1
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SCOPES = ("personal", "repo", "agent")


class ImportRejected(Exception):
    """The archive (or a plan) is unsafe or malformed; nothing was written."""


# ---------------------------------------------------------------------------------------------
# Secret detection (reports labels only, never the matched text)
# ---------------------------------------------------------------------------------------------

SECRET_PATTERNS = (
    ("private-key", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    ("aws-access-key", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("github-token", re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}")),
    ("github-token", re.compile(r"github_pat_[A-Za-z0-9_]{20,}")),
    ("slack-token", re.compile(r"xox[abprs]-[A-Za-z0-9-]{10,}")),
    ("api-key", re.compile(r"sk-(?:ant-)?[A-Za-z0-9_-]{20,}")),
    ("credential-assignment",
     re.compile(r"(password|passwd|secret|api[_-]?key|token)\s*[:=]\s*\S{8,}", re.I)),
)


def scan_secrets(text):
    """Sorted unique labels of the secret patterns found in `text`."""
    return sorted({label for label, rx in SECRET_PATTERNS if rx.search(text)})


# ---------------------------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------------------------


def _ok_note_name(filename):
    stem = filename[:-3] if filename.endswith(".md") else None
    return bool(stem) and NAME.match(stem) is not None and filename.lower() != "memory.md"


def _flat_notes(directory):
    """`[(filename, bytes)]` of the exportable notes in a folder (flat `*.md`, no symlinks,
    UTF-8, sane names)."""
    out = []
    directory = Path(directory)
    if not directory.is_dir() or directory.is_symlink():
        return out
    for entry in sorted(os.scandir(str(directory)), key=lambda e: e.name):
        if not _ok_note_name(entry.name):
            continue
        try:
            if entry.is_symlink() or not entry.is_file(follow_symlinks=False):
                continue
            data = Path(entry.path).read_bytes()
            data.decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        out.append((entry.name, data))
    return out


def _tarinfo(name, size):
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = 0o644
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    info.type = tarfile.REGTYPE
    return info


def export(cwd, out_dir, today=None, env=None):
    """Write `anthology-memory-<date>.tar.gz` into `out_dir`. Returns `{"path", "counts",
    "secrets", "repo_included"}`; `secrets` lists `{"file", "pattern"}` (labels only)."""
    env = dict(os.environ) if env is None else dict(env)
    date = _iso(today)
    home = memory_sync.anthology_home(env)
    root = memory_sync.repo_root(cwd, env=env)
    repo_ok = bool(root and memory_sync.is_approved(memory_sync.repo_key(root, env=env), env=env))
    files = {}  # member name -> bytes
    counts = {"personal": 0, "repo": 0, "agent": 0}
    for fname, data in _flat_notes(home / "personal-memory"):
        files["personal/" + fname] = data
        counts["personal"] += 1
    if repo_ok:
        for fname, data in _flat_notes(root / ".claude" / "memory"):
            files["repo/memory/" + fname] = data
            counts["repo"] += 1
        agents = root / ".claude" / "agent-memory"
        if agents.is_dir() and not agents.is_symlink():
            for d in sorted(agents.iterdir()):
                if NAME.match(d.name) and d.is_dir() and not d.is_symlink():
                    for fname, data in _flat_notes(d):
                        files["repo/agent-memory/%s/%s" % (d.name, fname)] = data
                        counts["agent"] += 1
    if sum(len(d) for d in files.values()) > MAX_TOTAL:
        raise ValueError("the notes are too large to export (limit %d MiB)" % (MAX_TOTAL >> 20))
    secrets = []
    for name in sorted(files):
        for label in scan_secrets(files[name].decode("utf-8")):
            secrets.append({"file": name, "pattern": label})
    manifest = json.dumps({"format": FORMAT, "date": date, "counts": counts},
                          indent=2, sort_keys=True).encode("utf-8") + b"\n"
    files["manifest.json"] = manifest
    buf = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=buf, mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
            for name in sorted(files):
                tar.addfile(_tarinfo(name, len(files[name])), io.BytesIO(files[name]))
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    n = 1
    while True:
        suffix = "" if n == 1 else "-%d" % n
        path = out / ("anthology-memory-%s%s.tar.gz" % (date, suffix))
        try:
            with open(str(path), "xb") as fh:  # never replaces an existing archive
                fh.write(buf.getvalue())
            break
        except FileExistsError:
            n += 1
    return {"path": str(path), "counts": counts, "secrets": secrets, "repo_included": repo_ok}


# ---------------------------------------------------------------------------------------------
# Import: validation (the archive is read, never extracted)
# ---------------------------------------------------------------------------------------------


class _CappedReader(object):
    """Seekable view of a gzip stream that refuses to go past `limit` decompressed bytes."""

    def __init__(self, gz, limit):
        self.gz, self.limit = gz, limit

    def read(self, n=-1):
        room = self.limit - self.gz.tell()
        if room < 0:
            raise ImportRejected("the archive expands beyond the size limit")
        if n is None or n < 0 or n > room + 1:
            n = room + 1  # one past the limit, to detect overflow
        data = self.gz.read(n)
        if self.gz.tell() > self.limit:
            raise ImportRejected("the archive expands beyond the size limit")
        return data

    def seek(self, pos, whence=0):
        if whence != 0 or pos > self.limit:
            raise ImportRejected("the archive expands beyond the size limit")
        return self.gz.seek(pos)

    def tell(self):
        return self.gz.tell()

    def close(self):
        self.gz.close()


def _check_name(name):
    """Return `(kind, parts)`: kind is `manifest`, `note`, or `dir`; raises on anything else."""
    if "\\" in name or "\x00" in name or name.startswith("/"):
        raise ImportRejected("unsafe member name: %r" % name)
    parts = name.split("/")
    if any(p in ("", ".", "..") for p in parts):
        raise ImportRejected("unsafe member name: %r" % name)
    if parts == ["manifest.json"]:
        return "manifest", parts
    if parts in (["personal"], ["repo"], ["repo", "memory"], ["repo", "agent-memory"]):
        return "dir", parts
    if len(parts) == 3 and parts[:2] == ["repo", "agent-memory"] and NAME.match(parts[2]):
        return "dir", parts
    if len(parts) == 2 and parts[0] == "personal" and _ok_note_name(parts[1]):
        return "note", parts
    if len(parts) == 3 and parts[:2] == ["repo", "memory"] and _ok_note_name(parts[2]):
        return "note", parts
    if (len(parts) == 4 and parts[:2] == ["repo", "agent-memory"] and NAME.match(parts[2])
            and _ok_note_name(parts[3])):
        return "note", parts
    raise ImportRejected("member outside the memory locations: %r" % name)


def _load_archive(archive):
    """Validate the whole archive, then return `(manifest dict, {member name: bytes})`.
    Raises ImportRejected (before anything is written; this function never writes)."""
    archive = str(archive)
    try:
        size = os.path.getsize(archive)
    except OSError as exc:
        raise ImportRejected("cannot read the archive: %s" % exc.strerror)
    if size > MAX_ARCHIVE:
        raise ImportRejected("the archive is larger than %d MiB" % (MAX_ARCHIVE >> 20))
    data_filter = getattr(tarfile, "data_filter", None)
    try:
        with open(archive, "rb") as raw:
            gz = gzip.GzipFile(fileobj=raw, mode="rb")
            with tarfile.open(fileobj=_CappedReader(gz, STREAM_LIMIT), mode="r:") as tar:
                members, names, total = [], set(), 0
                member = tar.next()
                while member is not None:
                    if len(members) >= MAX_MEMBERS:
                        raise ImportRejected("the archive has more than %d members" % MAX_MEMBERS)
                    kind, _parts = _check_name(member.name)
                    if member.name in names:
                        raise ImportRejected("duplicate member: %r" % member.name)
                    names.add(member.name)
                    if member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE):
                        raise ImportRejected("member %r is not a plain file or folder" % member.name)
                    if (kind == "dir") != member.isdir():
                        raise ImportRejected("member %r has the wrong type" % member.name)
                    if member.isdir() and member.size:
                        raise ImportRejected("folder member %r has data" % member.name)
                    if member.size < 0 or (kind == "manifest" and member.size > MAX_MANIFEST):
                        raise ImportRejected("member %r has a bad size" % member.name)
                    total += member.size
                    if total > MAX_TOTAL:
                        raise ImportRejected("the notes add up to more than %d MiB" % (MAX_TOTAL >> 20))
                    members.append((kind, member))
                    member = tar.next()
                if data_filter is not None:
                    with tempfile.TemporaryDirectory() as dest:
                        dest = os.path.realpath(dest)
                        for _kind, m in members:
                            try:
                                data_filter(m, dest)
                            except Exception as exc:
                                raise ImportRejected("member %r rejected by the tar filter: %s"
                                                     % (m.name, exc))
                contents = {}
                for kind, m in members:
                    if not m.isreg():
                        continue
                    fh = tar.extractfile(m)
                    data = fh.read(m.size + 1)
                    if len(data) != m.size:
                        raise ImportRejected("member %r is truncated" % m.name)
                    try:
                        data.decode("utf-8")
                    except UnicodeDecodeError:
                        raise ImportRejected("member %r is not valid UTF-8" % m.name)
                    contents[m.name] = data
    except ImportRejected:
        raise
    except (tarfile.TarError, OSError, EOFError, ValueError, MemoryError, OverflowError) as exc:
        raise ImportRejected("not a readable memory archive: %s" % (exc,))
    except Exception as exc:  # zlib.error and anything else a hostile file can provoke
        raise ImportRejected("not a readable memory archive: %s" % (exc,))
    if "manifest.json" not in contents:
        raise ImportRejected("manifest.json is missing")
    try:
        manifest = json.loads(contents["manifest.json"].decode("utf-8"))
    except ValueError:
        raise ImportRejected("manifest.json is not valid JSON")
    if not isinstance(manifest, dict) or manifest.get("format") != FORMAT:
        raise ImportRejected("unsupported archive format")
    return manifest, contents


# ---------------------------------------------------------------------------------------------
# Import: plan and apply
# ---------------------------------------------------------------------------------------------


def _iso(today):
    if today is None:
        return datetime.date.today().isoformat()
    if isinstance(today, datetime.date):
        return today.isoformat()
    return str(today)[:10]


def _file_sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _location(scope, agent, home, root):
    """`(base, parts)` for a scope: the trusted base dir and the folders below it."""
    if scope == "personal":
        return Path(home), ("personal-memory",)
    if scope == "repo":
        return Path(root), (".claude", "memory")
    return Path(root), (".claude", "agent-memory", agent)


def _entry_for(member):
    parts = member.split("/")
    if parts[0] == "personal":
        return {"member": member, "scope": "personal", "agent": None, "name": parts[1]}
    if parts[1] == "memory":
        return {"member": member, "scope": "repo", "agent": None, "name": parts[2]}
    return {"member": member, "scope": "agent", "agent": parts[2], "name": parts[3]}


def _is_plain_file(path):
    return os.path.isfile(path) and not os.path.islink(path)


def _same_as_existing(directory, name, data):
    """Identical to the note itself, or to an earlier `.imported-*` copy of it."""
    dest = os.path.join(directory, name)
    if _is_plain_file(dest) and Path(dest).read_bytes() == data:
        return True
    stem = name[:-3]
    try:
        for other in sorted(os.listdir(directory)):
            if other.startswith(stem + ".imported-") and other.endswith(".md"):
                if _is_plain_file(os.path.join(directory, other)) and \
                        Path(directory, other).read_bytes() == data:
                    return True
    except OSError:
        pass
    return False


def _imported_name(directory, name, date):
    stem = name[:-3]
    n = 1
    while True:
        candidate = "%s.imported-%s%s.md" % (stem, date, "" if n == 1 else "-%d" % n)
        if not os.path.lexists(os.path.join(directory, candidate)):
            return candidate
        n += 1


def plan_import(archive, cwd, today=None, env=None):
    """Validate `archive` and compare it with the notes already on this machine. Writes
    nothing. Returns a plan dict: `new`, `identical`, `conflicts` (each an entry with its
    `dest`), `secrets` (`{"file", "pattern"}`, labels only), `skipped_repo`, `messages`."""
    env = dict(os.environ) if env is None else dict(env)
    manifest, contents = _load_archive(archive)
    home = memory_sync.anthology_home(env)
    root = memory_sync.repo_root(cwd, env=env)
    repo_ok = bool(root and memory_sync.is_approved(memory_sync.repo_key(root, env=env), env=env))
    date = _iso(today)
    plan = {"archive": os.path.realpath(str(archive)), "sha256": _file_sha(str(archive)),
            "date": date, "home": str(home), "repo_root": str(root) if repo_ok else None,
            "new": [], "identical": [], "conflicts": [], "secrets": [], "skipped_repo": [],
            "messages": []}
    for member in sorted(contents):
        if member == "manifest.json":
            continue
        data = contents[member]
        for label in scan_secrets(data.decode("utf-8")):
            plan["secrets"].append({"file": member, "pattern": label})
        entry = _entry_for(member)
        if entry["scope"] != "personal" and not repo_ok:
            plan["skipped_repo"].append(member)
            continue
        base, parts = _location(entry["scope"], entry["agent"], home, root)
        directory = os.path.join(str(base), *parts)
        entry["dest"] = os.path.join(directory, entry["name"])
        if not os.path.lexists(entry["dest"]):
            plan["new"].append(entry)
        elif _same_as_existing(directory, entry["name"], data):
            plan["identical"].append(entry)
        else:
            entry["imported_as"] = os.path.join(directory, _imported_name(directory, entry["name"], date))
            plan["conflicts"].append(entry)
    if plan["skipped_repo"]:
        plan["messages"].append("this folder is not an approved repo (run /anthology:setup in it): "
                                "%d repo note(s) skipped" % len(plan["skipped_repo"]))
    return plan


def _check_chain(base, parts):
    """Refuse a symlink (or non-folder) anywhere below `base` on the way to the folder."""
    cur = Path(base)
    for part in parts:
        cur = cur / part
        if cur.is_symlink():
            raise ImportRejected("refusing to write through a symlink: %s" % cur)
        if os.path.lexists(str(cur)) and not cur.is_dir():
            raise ImportRejected("not a folder: %s" % cur)


def _make_dir(base, parts):
    base = Path(base)
    base.mkdir(parents=True, exist_ok=True)
    cur = base
    for part in parts:
        cur = cur / part
        if cur.is_symlink():
            raise ImportRejected("refusing to write through a symlink: %s" % cur)
        if not cur.exists():
            os.mkdir(str(cur))
            if cur.is_symlink():
                raise ImportRejected("refusing to write through a symlink: %s" % cur)
    if os.path.realpath(str(cur)) != os.path.join(os.path.realpath(str(base)), *parts):
        raise ImportRejected("the destination is not where it should be: %s" % cur)
    return cur


def _create(directory, name, data):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(os.path.join(str(directory), name), flags, 0o644)
    with os.fdopen(fd, "wb") as fh:
        fh.write(data)


def _checked_entry(entry):
    """The fields apply uses, re-validated; `dest` in a plan is informational only."""
    if not isinstance(entry, dict) or entry.get("scope") not in SCOPES:
        raise ImportRejected("malformed plan entry")
    name, agent, scope = entry.get("name"), entry.get("agent"), entry["scope"]
    if not isinstance(name, str) or not _ok_note_name(name):
        raise ImportRejected("malformed plan entry name")
    if scope == "agent" and not (isinstance(agent, str) and NAME.match(agent)):
        raise ImportRejected("malformed plan entry folder")
    if scope != "agent":
        agent = None
    expected = {"personal": "personal/%s", "repo": "repo/memory/%s"}.get(scope)
    member = (expected % name) if expected else "repo/agent-memory/%s/%s" % (agent, name)
    if entry.get("member") != member:
        raise ImportRejected("malformed plan entry member")
    return scope, agent, name, member


def apply_import(plan):
    """Write what `plan_import` planned. The archive is validated again (and must be the
    file that was planned); new notes are created exclusively, differing ones are kept as
    `<name>.imported-<date>.md`, nothing is overwritten. Returns `{"written", "conflicts",
    "skipped", "indexes"}`."""
    if not isinstance(plan, dict):
        raise ImportRejected("malformed plan")
    home, root, archive = plan.get("home"), plan.get("repo_root"), plan.get("archive")
    if not (isinstance(home, str) and isinstance(archive, str)) or Path(home).name != "anthology":
        raise ImportRejected("malformed plan")
    manifest, contents = _load_archive(archive)
    try:
        unchanged = _file_sha(archive) == plan.get("sha256")
    except OSError:
        unchanged = False
    if not unchanged:
        raise ImportRejected("the archive changed after it was planned")
    penv = {"CLAUDE_CONFIG_DIR": str(Path(home).parent)}  # approvals live next to `home`
    date = _iso(plan.get("date"))
    work = []
    for key in ("new", "conflicts", "identical"):
        for entry in plan.get(key, []):
            scope, agent, name, member = _checked_entry(entry)
            if member not in contents:
                raise ImportRejected("plan entry not in the archive: %s" % member)
            work.append((key, scope, agent, name, contents[member]))
    if any(w[1] != "personal" for w in work):
        if not isinstance(root, str) or not os.path.isdir(root) \
                or memory_sync.repo_root(root, env=penv) != Path(os.path.realpath(root)) \
                or not memory_sync.is_approved(memory_sync.repo_key(Path(root), env=penv), env=penv):
            raise ImportRejected("the repo is not an approved repo")
    # Pre-flight every destination before the first write.
    targets = {}
    for _key, scope, agent, _name, _data in work:
        base, parts = _location(scope, agent, home, root)
        _check_chain(base, parts)
        targets[(scope, agent)] = (base, parts)
    result = {"written": [], "conflicts": [], "skipped": [], "indexes": []}
    dirs = {}
    for key, scope, agent, name, data in work:
        if key == "identical":
            result["skipped"].append(name)
            continue
        base, parts = targets[(scope, agent)]
        directory = dirs.get((scope, agent))
        if directory is None:
            directory = dirs[(scope, agent)] = _make_dir(base, parts)
        if key == "new":
            try:
                _create(directory, name, data)
                result["written"].append(str(directory / name))
                continue
            except FileExistsError:
                pass  # appeared since planning: treat as a conflict below
        if _same_as_existing(str(directory), name, data):
            result["skipped"].append(name)
            continue
        while True:
            imported = _imported_name(str(directory), name, date)
            try:
                _create(directory, imported, data)
                break
            except FileExistsError:
                continue
        result["conflicts"].append(str(directory / imported))
    for directory in dirs.values():
        memory_sync.regenerate_index(directory)
        result["indexes"].append(str(directory))
    return result


# ---------------------------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------------------------


def _print_plan(plan):
    print("Import plan for %s" % os.path.basename(plan["archive"]))
    for key, title in (("new", "new"), ("identical", "identical (skipped)"),
                       ("conflicts", "conflicts (kept side by side as <name>.imported-%s.md)" % plan["date"])):
        print("  %s: %d" % (title, len(plan[key])))
        for e in plan[key]:
            print("    - %s" % e["member"])
    for message in plan["messages"]:
        print("  note: %s" % message)
    if plan["secrets"]:
        print("  secret-looking content found (labels only, contents not shown):")
        for s in plan["secrets"]:
            print("    - %s: %s" % (s["file"], s["pattern"]))


def main(argv=None):
    parser = argparse.ArgumentParser(prog="memory_portable.py",
                                     description="Export or import Anthology memory notes.")
    sub = parser.add_subparsers(dest="command", required=True)
    e = sub.add_parser("export", help="write anthology-memory-<date>.tar.gz")
    e.add_argument("--out", default=None, help="folder for the archive (default: current folder)")
    e.add_argument("--json", action="store_true")
    i = sub.add_parser("import", help="import an archive (prints the plan unless --yes)")
    i.add_argument("file")
    i.add_argument("--dry-run", action="store_true", help="only print the plan")
    i.add_argument("--yes", action="store_true", help="apply the plan")
    i.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    cwd = os.getcwd()
    try:
        if args.command == "export":
            res = export(cwd, args.out or cwd)
            if args.json:
                print(json.dumps(res, indent=2, sort_keys=True))
            else:
                c = res["counts"]
                print("Wrote %s" % res["path"])
                print("  personal: %d, repo: %d, agent: %d" % (c["personal"], c["repo"], c["agent"]))
                if not res["repo_included"]:
                    print("  (this folder is not an approved repo: repo notes not included)")
                for s in res["secrets"]:
                    print("  secret-looking content (%s) in %s" % (s["pattern"], s["file"]))
                print("The archive contains your personal notes; keep it private.")
            return 0
        plan = plan_import(args.file, cwd)
        result = None
        if args.yes and not args.dry_run:
            result = apply_import(plan)
        if args.json:
            print(json.dumps({"plan": plan, "result": result}, indent=2, sort_keys=True))
        else:
            _print_plan(plan)
            if result is None:
                print("Nothing written. Re-run with --yes to apply.")
            else:
                print("Wrote %d note(s); %d kept side by side:" % (len(result["written"]),
                                                                   len(result["conflicts"])))
                for path in result["conflicts"]:
                    print("    %s" % path)
        return 0
    except ImportRejected as exc:
        print("memory_portable: rejected: %s" % exc, file=sys.stderr)
        return 2
    except (ValueError, OSError) as exc:
        print("memory_portable: %s" % exc, file=sys.stderr)
        return 1


if __name__ == "__main__":
    import memory_portable  # run the one shared module instance, not a second `__main__` copy

    sys.exit(memory_portable.main())
