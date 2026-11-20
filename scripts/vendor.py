#!/usr/bin/env python3
"""Reproducible vendoring: every vendored tree is generated from a pinned upstream.

    vendor.py fetch [NAME...]            shallow-fetch the pinned commits into .vendor-cache/
                                         and copy each repo's license file to vendor/licenses/
    vendor.py build [NAME] [--out DIR]   pristine -> transforms -> patch -> target
    vendor.py check                      rebuild everything in a temp dir and diff it against
                                         the committed trees and licenses; non-zero on any difference
    vendor.py sync NAME --ref REF        re-pin, rebuild, report patch rejects; never commits
    vendor.py diff NAME                  pristine+transforms vs shipped, as a patch

Needs Python 3.9+ (the macOS system python3), the standard library, and `git`
(for fetch/sync and for applying patches). Pins live in vendor/sources.json.

Fetching is a function `fetch(entry) -> Path` returning a pristine checkout of the
upstream repo, so tests can stub it with local fixture trees.
"""
from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE_DIRNAME = ".vendor-cache"
SOURCES = Path("vendor") / "sources.json"
LICENSES = Path("vendor") / "licenses"
ALLOWED_LICENSES = {"MIT", "Apache-2.0"}
SUBTREE = "subtree"
IGNORED_EXTRAS = {".DS_Store"}

SHA1 = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
NAME = re.compile(r"^[a-z0-9][a-z0-9-]*$")


class VendorError(Exception):
    pass


# --------------------------------------------------------------------------- sources.json

def load_sources(root: Path) -> list:
    data = json.loads((Path(root) / SOURCES).read_text(encoding="utf-8"))
    return data["sources"]


def save_sources(root: Path, entries: list) -> None:
    text = json.dumps({"sources": entries}, indent=2, ensure_ascii=False) + "\n"
    (Path(root) / SOURCES).write_text(text, encoding="utf-8")


def is_subtree(entry: dict) -> bool:
    return entry.get("managed_by") == SUBTREE


def vendored(entries: list) -> list:
    """Entries this tool builds (subtree-managed ones, like rtk, are skipped)."""
    return [e for e in entries if not is_subtree(e)]


def source_slug(entry: dict) -> str:
    """`vendor/licenses/<slug>.txt`: owner-repo for git sources, else the entry name."""
    repo = entry.get("repo")
    if repo:
        parts = [p for p in repo.rstrip("/").split("/") if p]
        if parts[-1].endswith(".git"):
            parts[-1] = parts[-1][:-4]
        slug = "-".join(parts[-2:])
    else:
        slug = entry["name"]
    return re.sub(r"[^A-Za-z0-9._-]", "-", slug).lower()


def _safe_relpath(value, allow_empty: bool) -> bool:
    if not isinstance(value, str):
        return False
    if value == "":
        return allow_empty
    if value.startswith("/") or "\\" in value:
        return False
    return ".." not in value.split("/")


def _check_transforms(label: str, entry: dict, errors: list) -> None:
    transforms = entry.get("transform")
    if not isinstance(transforms, list):
        errors.append(f"{label}: transform must be a list")
        return
    for t in transforms:
        if isinstance(t, str):
            t = {"name": t}
        if not isinstance(t, dict):
            errors.append(f"{label}: bad transform {t!r}")
        elif "patch" in t:
            if not _safe_relpath(t["patch"], False):
                errors.append(f"{label}: bad patch path {t['patch']!r}")
        elif t.get("name") not in TRANSFORMS:
            errors.append(f"{label}: unknown transform {t.get('name')!r}")
        elif t["name"] == "rename-plugin":
            if not (isinstance(t.get("old"), str) and isinstance(t.get("new"), str)
                    and t["old"] and t["new"]):
                errors.append(f"{label}: rename-plugin needs old and new")
            if not entry.get("plugin_version"):
                errors.append(f"{label}: rename-plugin needs plugin_version")


def validate_sources(entries: list) -> list:
    """Return a list of problems (empty when sources.json is valid)."""
    errors: list = []
    names: dict = {}
    targets: list = []
    license_files: dict = {}
    for i, e in enumerate(entries):
        label = e.get("name", f"entry {i}") if isinstance(e, dict) else f"entry {i}"
        if not isinstance(e, dict):
            errors.append(f"{label}: not an object")
            continue
        if not isinstance(e.get("name"), str) or not NAME.match(e["name"]):
            errors.append(f"{label}: name must be lowercase letters, digits and dashes")
        elif e["name"] in names:
            errors.append(f"{label}: duplicate name")
        names[e.get("name")] = e
        if e.get("license") not in ALLOWED_LICENSES:
            errors.append(f"{label}: license {e.get('license')!r} is not in "
                          f"{sorted(ALLOWED_LICENSES)}; the owner must decide")
        if not _safe_relpath(e.get("target"), False):
            errors.append(f"{label}: target must be a relative path inside the repo")
        else:
            targets.append((label, e["target"].rstrip("/")))
        if is_subtree(e):
            continue
        for field in ("path", "ref", "transform"):
            if field not in e:
                errors.append(f"{label}: missing {field}")
        if "path" in e and not _safe_relpath(e["path"], True):
            errors.append(f"{label}: path must stay inside the upstream repo")
        if not (e.get("repo") or e.get("url")):
            errors.append(f"{label}: needs repo or url")
        if e.get("repo") and not re.match(r"^https://[A-Za-z0-9.-]+/[A-Za-z0-9._/-]+$", e["repo"]):
            errors.append(f"{label}: repo must be an https URL")
        commit, checksum = e.get("commit"), e.get("checksum")
        if commit is not None and not (isinstance(commit, str) and SHA1.match(commit)):
            errors.append(f"{label}: commit must be 40 lowercase hex characters")
        elif checksum is not None and not (isinstance(checksum, str) and SHA256.match(checksum)):
            errors.append(f"{label}: checksum must be 64 lowercase hex characters")
        elif commit is None and checksum is None:
            errors.append(f"{label}: needs a 40-hex commit or a checksum")
        if e.get("repo"):
            if "license_file" not in e:
                errors.append(f"{label}: missing license_file (null needs a license_note)")
            elif e["license_file"] is None:
                if not e.get("license_note"):
                    errors.append(f"{label}: license_file null needs a license_note")
            elif not _safe_relpath(e["license_file"], False):
                errors.append(f"{label}: bad license_file")
            slug = source_slug(e)
            if slug in license_files and license_files[slug] != e.get("license_file"):
                errors.append(f"{label}: entries from one repo must agree on license_file")
            license_files.setdefault(slug, e.get("license_file"))
        _check_transforms(label, e, errors)
    for i, (a_label, a) in enumerate(targets):
        for b_label, b in targets[i + 1:]:
            if a == b or a.startswith(b + "/") or b.startswith(a + "/"):
                errors.append(f"{a_label}: target overlaps {b_label}")
    return errors


def _require_valid(entries: list) -> None:
    errors = validate_sources(entries)
    if errors:
        raise VendorError("vendor/sources.json is invalid:\n  " + "\n  ".join(errors))


def find_entry(entries: list, name: str) -> dict:
    for e in entries:
        if e.get("name") == name:
            if is_subtree(e):
                raise VendorError(f"{name} is managed_by subtree, not by vendor.py")
            return e
    raise VendorError(f"no such source: {name}")


# --------------------------------------------------------------------------- transforms

def rename_plugin(tree: Path, old: str, new: str, plugin_version: str) -> dict:
    """Rename a Claude Code plugin and the `old:<skill>` references to its skills.

    - `.claude-plugin/plugin.json`: `name` -> new, `version` -> plugin_version.
    - Every text file: the token `old:` followed by `[a-z0-9-]` becomes `new:`, unless
      the token is part of a longer name (`anthology-old:`), a path or URL (`/old:`,
      `.old:`), or is not directly followed by a skill name (`old:` in prose).
    """
    tree = Path(tree)
    manifest = tree / ".claude-plugin" / "plugin.json"
    if not manifest.is_file():
        raise VendorError("rename-plugin: .claude-plugin/plugin.json not found")
    data = json.loads(manifest.read_text(encoding="utf-8"))
    if data.get("name") != old:
        raise VendorError(f"rename-plugin: plugin.json name is {data.get('name')!r}, expected {old!r}")
    data["name"] = new
    data["version"] = plugin_version
    manifest.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    token = re.compile(rb"(?<![\w./-])" + re.escape(old.encode()) + rb":(?=[a-z0-9-])")
    replacement = new.encode() + b":"
    files = references = 0
    for path in sorted(p for p in tree.rglob("*") if p.is_file() and not p.is_symlink()):
        raw = path.read_bytes()
        if b"\0" in raw:
            continue
        changed, count = token.subn(replacement, raw)
        if count:
            path.write_bytes(changed)  # keeps the file's mode
            files += 1
            references += count
    return {"files": files, "references": references}


def _run_rename_plugin(tree: Path, entry: dict, spec: dict) -> dict:
    return rename_plugin(tree, spec["old"], spec["new"], entry["plugin_version"])


TRANSFORMS = {"rename-plugin": _run_rename_plugin}


# --------------------------------------------------------------------------- tree helpers

def _norm_mode(path: Path) -> int:
    return 0o755 if os.stat(path).st_mode & 0o111 else 0o644


def copy_tree(src: Path, dst: Path) -> None:
    """Copy src to dst without any `.git` directory; modes normalized to 644/755."""
    dst.mkdir(parents=True, exist_ok=True)
    for dirpath, dirnames, filenames in os.walk(src):
        dirnames[:] = sorted(d for d in dirnames if d != ".git")
        rel = Path(dirpath).relative_to(src)
        (dst / rel).mkdir(parents=True, exist_ok=True)
        for name in sorted(filenames):
            s, d = Path(dirpath) / name, dst / rel / name
            if s.is_symlink():
                os.symlink(os.readlink(s), d)
            else:
                shutil.copyfile(s, d)
                d.chmod(_norm_mode(s))


def snapshot(tree: Path) -> dict:
    """rel path -> ('link', target) | ('file', exec bit, bytes)."""
    out: dict = {}
    for dirpath, dirnames, filenames in os.walk(tree):
        dirnames.sort()
        for name in filenames:
            p = Path(dirpath) / name
            rel = p.relative_to(tree).as_posix()
            if p.is_symlink():
                out[rel] = ("link", os.readlink(p))
            else:
                out[rel] = ("file", bool(os.stat(p).st_mode & 0o111), p.read_bytes())
    return out


def compare_trees(expected: Path, actual: Path, label: str) -> list:
    if not actual.is_dir():
        return [f"{label}: committed target is missing"]
    want, have = snapshot(expected), snapshot(actual)
    problems = []
    for rel in sorted(set(want) | set(have)):
        w, h = want.get(rel), have.get(rel)
        if h is None:
            problems.append(f"{label}: missing from the committed tree: {rel}")
        elif w is None:
            if Path(rel).name not in IGNORED_EXTRAS:
                problems.append(f"{label}: extra file not in the build: {rel}")
        elif w[0] != h[0] or (w[0] == "link" and w[1] != h[1]):
            problems.append(f"{label}: {rel} differs (file type or link target)")
        elif w[0] == "file":
            if w[2] != h[2]:
                problems.append(f"{label}: {rel} differs from the build")
            elif w[1] != h[1]:
                problems.append(f"{label}: {rel} mode differs (executable bit "
                                f"{'set' if w[1] else 'clear'} in the build)")
    return problems


# --------------------------------------------------------------------------- git

def _git(args: list, cwd=None) -> subprocess.CompletedProcess:
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GIT_CONFIG_NOSYSTEM="1")
    try:
        return subprocess.run(["git", "-c", "core.autocrlf=false"] + args, cwd=cwd, env=env,
                              capture_output=True, text=True)
    except FileNotFoundError:
        raise VendorError("git is required for this command (fetch/sync/patches)")


def _git_ok(args: list, cwd=None) -> str:
    result = _git(args, cwd)
    if result.returncode != 0:
        raise VendorError(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def git_fetcher(cache: Path):
    """Return fetch(entry) -> pristine checkout of the repo at the pinned commit."""
    cache = Path(cache)

    def fetch(entry: dict) -> Path:
        repo, commit = entry.get("repo"), entry.get("commit")
        if not repo or not commit:
            raise VendorError(f"{entry['name']}: only git sources with a commit can be fetched")
        dest = cache / f"{source_slug(entry)}-{commit}"
        if (dest / ".git").is_dir():
            return dest
        cache.mkdir(parents=True, exist_ok=True)
        tmp = cache / f".tmp-{source_slug(entry)}-{commit}-{os.getpid()}"
        shutil.rmtree(tmp, ignore_errors=True)
        try:
            tmp.mkdir()
            _git_ok(["init", "-q"], tmp)
            _git_ok(["remote", "add", "origin", repo], tmp)
            _git_ok(["fetch", "-q", "--depth", "1", "origin", commit], tmp)
            _git_ok(["-c", "advice.detachedHead=false", "checkout", "-q", "--detach", "FETCH_HEAD"], tmp)
            head = _git_ok(["rev-parse", "HEAD"], tmp).strip()
            if head != commit:
                raise VendorError(f"{entry['name']}: fetched {head}, expected {commit}")
            shutil.rmtree(dest, ignore_errors=True)
            tmp.rename(dest)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
        return dest

    return fetch


def git_resolve_ref(entry: dict, ref: str) -> str:
    """Resolve a tag or branch of the entry's repo to a commit sha."""
    out = _git_ok(["ls-remote", entry["repo"], f"refs/tags/{ref}", f"refs/tags/{ref}^{{}}",
                   f"refs/heads/{ref}"])
    found = {}
    for line in out.splitlines():
        sha, _, name = line.partition("\t")
        found[name] = sha
    for name in (f"refs/tags/{ref}^{{}}", f"refs/tags/{ref}", f"refs/heads/{ref}"):
        if name in found:
            return found[name]
    raise VendorError(f"{entry['name']}: ref {ref!r} not found in {entry['repo']}")


def apply_patch(tree: Path, patch: Path, reject: bool = False) -> list:
    """Apply a patch (-p1) to tree. Returns the rejected-hunk files (reject mode only)."""
    args = ["apply", "-p1", "--whitespace=nowarn"] + (["--reject"] if reject else [])
    result = _git(args + [str(patch.resolve())], cwd=tree)
    if result.returncode == 0:
        return []
    if not reject:
        raise VendorError(f"patch {patch.name} does not apply:\n{result.stderr.strip()}")
    rejects = sorted(p.relative_to(tree).as_posix() for p in tree.rglob("*.rej"))
    return rejects or [f"{patch.name}: {result.stderr.strip()}"]


# --------------------------------------------------------------------------- build

def _pristine_subtree(entry: dict, pristine: Path) -> Path:
    src = (Path(pristine) / entry["path"]).resolve()
    if not src.is_dir() or Path(pristine).resolve() not in (src, *src.parents):
        raise VendorError(f"{entry['name']}: path {entry['path']!r} is not a directory upstream")
    return src


def build_tree(root: Path, entry: dict, pristine: Path, dest: Path,
               patches: bool = True, reject: bool = False) -> dict:
    """pristine -> transforms -> patches, written to dest. Returns stats and rejects."""
    stats: dict = {"transforms": [], "rejects": []}
    copy_tree(_pristine_subtree(entry, pristine), dest)
    for t in entry["transform"]:
        spec = {"name": t} if isinstance(t, str) else t
        if "patch" in spec:
            if patches:
                patch = Path(root) / spec["patch"]
                if not patch.is_file():
                    raise VendorError(f"{entry['name']}: patch {spec['patch']} not found")
                stats["rejects"] += apply_patch(dest, patch, reject=reject)
        else:
            stats["transforms"].append((spec["name"], TRANSFORMS[spec["name"]](dest, entry, spec)))
    return stats


def install_tree(src: Path, dest: Path) -> None:
    if dest.exists() or dest.is_symlink():
        shutil.rmtree(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    copy_tree(src, dest)


def _fetcher(root: Path, fetch):
    return fetch or git_fetcher(Path(root) / CACHE_DIRNAME)


def build(root: Path, name: str, out=None, fetch=None, reject: bool = False) -> dict:
    """Build one source into <out or root>/<target>."""
    root = Path(root)
    entries = load_sources(root)
    _require_valid(entries)
    entry = find_entry(entries, name)
    pristine = _fetcher(root, fetch)(entry)
    with tempfile.TemporaryDirectory(prefix="vendor-build-") as tmp:
        built = Path(tmp) / "tree"
        stats = build_tree(root, entry, pristine, built, reject=reject)
        install_tree(built, Path(out or root) / entry["target"])
    return stats


# --------------------------------------------------------------------------- licenses

def fixed_license_problems(root: Path, entry: dict) -> list:
    path = Path(root) / LICENSES / f"{source_slug(entry)}.txt"
    if not path.is_file():
        return [f"{LICENSES / path.name}: missing (a fixed license text for {entry['name']})"]
    text = path.read_text(encoding="utf-8")
    first = text.splitlines()[0] if text.strip() else ""
    problems = []
    if entry["license_note"] not in first:
        problems.append(f"{LICENSES / path.name}: first line must note the license is "
                        f"declared in the upstream {entry['license_note']}")
    if "License" not in text:
        problems.append(f"{LICENSES / path.name}: does not contain a license text")
    return problems


def _upstream_license(entry: dict, pristine: Path) -> bytes:
    src = Path(pristine) / entry["license_file"]
    if not src.is_file():
        raise VendorError(f"{entry['name']}: upstream license file {entry['license_file']} not found")
    data = src.read_bytes()
    if not data.strip():
        raise VendorError(f"{entry['name']}: upstream license file {entry['license_file']} is empty")
    return data


def fetch_licenses(root: Path, names=None, fetch=None) -> list:
    """Copy each upstream repo's license file, unchanged, to vendor/licenses/<slug>.txt."""
    root = Path(root)
    entries = load_sources(root)
    _require_valid(entries)
    fetch = _fetcher(root, fetch)
    written = []
    for e in vendored(entries):
        if (names and e["name"] not in names) or not e.get("license_file"):
            continue  # fixed licenses (license_file null) are written by hand, never fetched
        dest = root / LICENSES / f"{source_slug(e)}.txt"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(_upstream_license(e, fetch(e)))
        if dest not in written:
            written.append(dest)
    return written


def check_licenses(root: Path, entries: list, fetch) -> list:
    problems, seen = [], set()
    for e in vendored(entries):
        slug = source_slug(e)
        if slug in seen:
            continue
        seen.add(slug)
        path = Path(root) / LICENSES / f"{slug}.txt"
        rel = LICENSES / path.name
        if not e.get("license_file"):
            problems += fixed_license_problems(root, e)
        elif not path.is_file():
            problems.append(f"{rel}: missing")
        elif not path.read_bytes().strip():
            problems.append(f"{rel}: empty")
        elif path.read_bytes() != _upstream_license(e, fetch(e)):
            problems.append(f"{rel}: differs from the upstream {e['license_file']}")
    return problems


# --------------------------------------------------------------------------- check, sync, diff

def check(root: Path, fetch=None) -> list:
    """Rebuild everything in a temp dir and diff it against the committed trees."""
    root = Path(root)
    entries = load_sources(root)
    errors = validate_sources(entries)
    if errors:
        return [f"sources.json: {m}" for m in errors]
    fetch = _fetcher(root, fetch)
    problems = []
    with tempfile.TemporaryDirectory(prefix="vendor-check-") as tmp:
        for e in vendored(entries):
            built = Path(tmp) / e["name"]
            try:
                build_tree(root, e, fetch(e), built)
            except VendorError as exc:
                problems.append(f"{e['name']}: {exc}")
                continue
            problems += compare_trees(built, root / e["target"], e["target"])
    try:
        problems += check_licenses(root, entries, fetch)
    except VendorError as exc:
        problems.append(str(exc))
    return problems


def sync(root: Path, name: str, ref: str, fetch=None, resolve=None) -> dict:
    """Re-pin to ref, rebuild the target and refresh the license. Never commits."""
    root = Path(root)
    entries = load_sources(root)
    _require_valid(entries)
    entry = find_entry(entries, name)
    commit = ref if SHA1.match(ref) else (resolve or git_resolve_ref)(entry, ref)
    old = {"ref": entry["ref"], "commit": entry["commit"]}
    entry["ref"], entry["commit"] = ref, commit
    _require_valid(entries)
    fetch = _fetcher(root, fetch)
    pristine = fetch(entry)
    manifest = Path(pristine) / ".claude-plugin" / "plugin.json"
    if "upstream_version" in entry and manifest.is_file():
        entry["upstream_version"] = json.loads(manifest.read_text(encoding="utf-8")).get(
            "version", entry["upstream_version"])
    with tempfile.TemporaryDirectory(prefix="vendor-sync-") as tmp:
        built = Path(tmp) / "tree"
        stats = build_tree(root, entry, pristine, built, reject=True)
        install_tree(built, root / entry["target"])
    save_sources(root, entries)
    fetch_licenses(root, names=[name], fetch=fetch)
    return {"name": name, "old": old, "new": {"ref": ref, "commit": commit},
            "transforms": stats["transforms"], "rejects": stats["rejects"]}


def _unified(rel: str, old: bytes | None, new: bytes | None) -> str:
    try:
        a = [] if old is None else old.decode("utf-8").splitlines(keepends=True)
        b = [] if new is None else new.decode("utf-8").splitlines(keepends=True)
    except UnicodeDecodeError:
        raise VendorError(f"{rel}: binary changes cannot be expressed as a patch")
    lines = difflib.unified_diff(
        a, b, fromfile="/dev/null" if old is None else f"a/{rel}",
        tofile="/dev/null" if new is None else f"b/{rel}", lineterm="\n")
    out = []
    for line in lines:
        out.append(line if line.endswith("\n") else line + "\n\\ No newline at end of file\n")
    return "".join(out)


def diff_text(expected: Path, actual: Path) -> str:
    want, have = snapshot(expected), snapshot(actual)
    chunks = []
    for rel in sorted(set(want) | set(have)):
        w, h = want.get(rel), have.get(rel)
        if (w and w[0] != "file") or (h and h[0] != "file"):
            raise VendorError(f"{rel}: links cannot be expressed as a patch")
        if w and h and w[1] != h[1]:
            raise VendorError(f"{rel}: mode changes cannot be expressed as a patch")
        if h and not w and h[1]:
            raise VendorError(f"{rel}: new executable files cannot be expressed as a patch")
        if w and h and w[2] == h[2]:
            continue
        chunks.append(_unified(rel, w[2] if w else None, h[2] if h else None))
    return "".join(chunks)


def diff_entry(root: Path, name: str, fetch=None) -> str:
    """Pristine + transforms (no patches) vs the shipped tree, as a `git apply -p1` patch."""
    root = Path(root)
    entries = load_sources(root)
    _require_valid(entries)
    entry = find_entry(entries, name)
    pristine = _fetcher(root, fetch)(entry)
    with tempfile.TemporaryDirectory(prefix="vendor-diff-") as tmp:
        built = Path(tmp) / "tree"
        build_tree(root, entry, pristine, built, patches=False)
        shipped = root / entry["target"]
        if not shipped.is_dir():
            raise VendorError(f"{entry['target']} does not exist; run build first")
        return diff_text(built, shipped)


# --------------------------------------------------------------------------- CLI

def _names(root: Path, requested: list) -> list:
    entries = vendored(load_sources(root))
    if not requested:
        return [e["name"] for e in entries]
    for n in requested:
        find_entry(entries, n)
    return requested


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--root", type=Path, default=ROOT, help=argparse.SUPPRESS)
    parser.add_argument("--cache", type=Path, help=f"pristine cache (default: <root>/{CACHE_DIRNAME})")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("fetch", help="fetch pinned upstreams and copy their license files")
    p.add_argument("names", nargs="*")
    p = sub.add_parser("build", help="pristine -> transforms -> patch -> target")
    p.add_argument("name", nargs="?", help="default: every source")
    p.add_argument("--out", type=Path, help="write <out>/<target> instead of the repo")
    sub.add_parser("check", help="rebuild all into a temp dir and diff against the repo")
    p = sub.add_parser("sync", help="re-pin to REF and rebuild; never commits")
    p.add_argument("name")
    p.add_argument("--ref", required=True)
    p = sub.add_parser("diff", help="pristine+transforms vs shipped, as a patch")
    p.add_argument("name")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    fetch = git_fetcher(args.cache or root / CACHE_DIRNAME)
    try:
        if args.command == "fetch":
            names = _names(root, args.names)
            for n in names:
                print(f"fetched {n}: {fetch(find_entry(load_sources(root), n))}")
            for path in fetch_licenses(root, names=names, fetch=fetch):
                print(f"license {path.relative_to(root)}")
        elif args.command == "build":
            for n in _names(root, [args.name] if args.name else []):
                stats = build(root, n, out=args.out, fetch=fetch)
                detail = ", ".join(f"{t}: {s}" for t, s in stats["transforms"]) or "no transforms"
                print(f"built {n} ({detail})")
        elif args.command == "check":
            problems = check(root, fetch=fetch)
            for line in problems:
                print(line, file=sys.stderr)
            print("vendor check: " + (f"{len(problems)} problem(s)" if problems else "clean"))
            return 1 if problems else 0
        elif args.command == "sync":
            report = sync(root, args.name, args.ref, fetch=fetch)
            print(f"re-pinned {args.name}: {report['old']['ref']} -> {args.ref} "
                  f"({report['new']['commit']})")
            for r in report["rejects"]:
                print(f"REJECT {r}", file=sys.stderr)
            print("Rebuilt the target and refreshed the license. Nothing was committed.\n"
                  "Review the diff; if the output changed, bump plugin_version in sources.json "
                  "where it applies.")
            return 1 if report["rejects"] else 0
        elif args.command == "diff":
            sys.stdout.write(diff_entry(root, args.name, fetch=fetch))
    except VendorError as exc:
        print(f"vendor.py: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
