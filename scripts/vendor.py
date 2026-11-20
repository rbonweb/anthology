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
VENDORED_PLUGIN = "plugins/anthology-superpowers"
KNOWLEDGE_DIR = "plugins/anthology/knowledge"
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
    # No "..", no "." and no empty components ("a//b", "./", "a/"): a target like "." or
    # "plugins/." would otherwise name the repo root or a parent of what build replaces.
    return all(part not in ("", ".", "..") for part in value.split("/"))


def _safe_target(value, subtree: bool) -> bool:
    """build/sync replace the target with rmtree, so a vendored target is exactly the
    vendored plugin or one folder of the knowledge directory: never a first-party plugin
    directory (`plugins/anthology`), the knowledge directory itself, or anything nested."""
    if not _safe_relpath(value, False):
        return False
    if subtree:
        return True
    if value == VENDORED_PLUGIN:
        return True
    return value.startswith(KNOWLEDGE_DIR + "/") and "/" not in value[len(KNOWLEDGE_DIR) + 1:]


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
        elif t["name"] == "rename-skill":
            if not all(isinstance(t.get(k), str) and NAME.match(t[k]) for k in ("old", "new")):
                errors.append(f"{label}: rename-skill needs old and new skill names")


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
        if not _safe_target(e.get("target"), is_subtree(e)):
            errors.append(f"{label}: target must be {VENDORED_PLUGIN} or "
                          f"{KNOWLEDGE_DIR}/<name> (build deletes and recreates it)")
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


# Connector setup sections: "Connectors", "If Connectors Available", "Tool Integration",
# "Connect <Product> (optional)", "... Connected Tools", and any heading holding a `~~` placeholder.
CONNECTOR_HEADING = re.compile(
    r"^(?:if connectors available|connectors?|tool integrations?|connect \w+(?: \(optional\))?"
    r"|.*\bconnected tools)$", re.IGNORECASE)
MD_HEADING = re.compile(r"^(#{1,6})[ \t]+(.*?)(?:[ \t]+#+)?$")
MD_FENCE = re.compile(r"^\s*(```|~~~)")
LIST_MARKER = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s")
# `If ... connected` lead-ins: a `~~<category>` placeholder, or the tools themselves ("If the
# UXKIN tools are connected", "... weren't connected"). "If no tool is connected" and "If tools
# are not connected" describe the fallback behaviour and stay.
IF_PLACEHOLDER_CONNECTED = re.compile(r"^\s*(?:\*\*)?If\b.*~~.*\bconnected\b")
IF_TOOLS_CONNECTED = re.compile(
    r"^\s*(?:[-*+]\s+)?(?:\*\*)?If\s+(?!no\b).*\btools?\s+(?:is|are|weren't|were)\s+connected\b")
LIST_IF_CONNECTED = re.compile(r"^\s*[-*+]\s.*~~.*\(if connected\)")
CONNECT_VIA_MCP = re.compile(r"^\s*[-*+]\s.*\bconnect\b.*\bMCP\b")
SUPERCHARGED_BOX = re.compile(r"SUPERCHARGED.*\bconnect\b")
CONNECTORS_FILE = "CONNECTORS.md"


def _is_connector_heading(text: str) -> bool:
    return bool(CONNECTOR_HEADING.match(text.strip())) or "~~" in text


def _fence_end(lines: list, start: int, marker: str) -> int:
    """Index of the line closing the fence opened at `start` (the last line if unclosed)."""
    for j in range(start + 1, len(lines)):
        m = MD_FENCE.match(lines[j].rstrip())
        if m and m.group(1) == marker:
            return j
    return len(lines) - 1


def _section_end(lines: list, start: int, level: int) -> int:
    """Index of the next heading (outside code fences) at `level` or higher, else the end."""
    j = start + 1
    while j < len(lines):
        line = lines[j].rstrip()
        m = MD_FENCE.match(line)
        if m:
            j = _fence_end(lines, j, m.group(1)) + 1
            continue
        h = MD_HEADING.match(line)
        if h and len(h.group(1)) <= level:
            return j
        j += 1
    return len(lines)


def _if_block_end(lines: list, i: int) -> int:
    """End of an `If ... connected` block: a list item is one line; a lead-in ending in a colon
    owns the list under it (also after one blank line); anything else is its paragraph."""
    line = lines[i].rstrip()
    if LIST_MARKER.match(line):
        return i + 1
    j = i + 1
    if line.endswith((":", ":**")) and j + 1 < len(lines) and not lines[j].strip() \
            and LIST_MARKER.match(lines[j + 1]):
        j += 1
    while j < len(lines) and lines[j].strip():
        j += 1
    return j


def _strip_markdown(text: str) -> str:
    lines = text.split("\n")
    out: list = []
    i, n = 0, len(lines)

    def skip_blanks(j: int) -> int:
        # a removed block takes its trailing blank lines unless text stands right before it
        if out and out[-1].strip():
            return j
        while j < n and not lines[j].strip():
            j += 1
        return j

    def trim_blanks() -> int:
        k = len(out)
        while k and not out[k - 1].strip():
            k -= 1
        return k

    while i < n:
        line = lines[i]
        probe = line.rstrip()
        m = MD_FENCE.match(probe)
        if m:
            end = _fence_end(lines, i, m.group(1))
            block = lines[i:end + 1]
            if not any(SUPERCHARGED_BOX.search(l) for l in block):
                out.extend(block)
                i = end + 1
                continue
            # the "STANDALONE / SUPERCHARGED (when you connect your tools)" box goes, with a
            # line that only introduces it and a heading it leaves without any content
            k = trim_blanks()
            if k and out[k - 1].rstrip().endswith(":") and not MD_HEADING.match(out[k - 1].rstrip()):
                del out[k - 1:]
            j = end + 1
            while j < n and not lines[j].strip():
                j += 1
            k = trim_blanks()
            above = MD_HEADING.match(out[k - 1].rstrip()) if k else None
            below = MD_HEADING.match(lines[j].rstrip()) if j < n else None
            if above and (j >= n or (below and len(below.group(1)) <= len(above.group(1)))):
                del out[k - 1:]
            i = j if out and not out[-1].strip() else end + 1
            continue
        h = MD_HEADING.match(probe)
        if h and _is_connector_heading(h.group(2)):
            i = _section_end(lines, i, len(h.group(1)))
            continue
        if probe.lstrip().startswith(">"):
            j = i
            while j < n and lines[j].lstrip().startswith(">"):
                j += 1
            if any(CONNECTORS_FILE in l for l in lines[i:j]):
                i = skip_blanks(j)
                continue
        if IF_PLACEHOLDER_CONNECTED.match(probe) or IF_TOOLS_CONNECTED.match(probe):
            i = skip_blanks(_if_block_end(lines, i))
            continue
        if LIST_IF_CONNECTED.match(probe) or CONNECT_VIA_MCP.match(probe):
            i += 1
            continue
        out.append(line)
        i += 1
    if out == lines:
        return text
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out) + "\n"


def strip_connectors(tree: Path) -> dict:
    """Remove everything that tells an agent to use connected tools, so the folders are connector-free.

    In every `.md` file, outside code fences:
    - a connector setup section: a heading that is "Connectors", "If Connectors Available",
      "Tool Integration", "Connect <Product> (optional)", ends in "Connected Tools" or holds a
      `~~` placeholder, together with its body, up to the next heading of the same or a higher level;
    - an `If ... connected` block: a lead-in with a `~~<category>` placeholder, or one saying the
      tools "are/is/weren't connected" (not "If no tool is connected" or "If tools are not
      connected", the fallbacks). A one-line list item is just the line; a lead-in ending with a
      colon takes the list under it; otherwise it takes its paragraph;
    - a list item `~~<category> (if connected): ...`;
    - a list item telling the user to connect a tool via MCP;
    - a fenced "SUPERCHARGED (when you connect your tools)" box, a line that only introduces it
      and a heading left without content;
    - a blockquote paragraph that points to `CONNECTORS.md`.
    A file named `CONNECTORS.md` is deleted. Nothing else changes. If a `~~` placeholder or
    a `CONNECTORS.md` reference is left over, this fails rather than guessing a rewrite.
    """
    tree = Path(tree)
    stats = {"files": 0, "deleted": 0}
    for path in sorted(p for p in tree.rglob("*") if not p.is_symlink() and p.is_file()):
        if path.name == CONNECTORS_FILE:
            path.unlink()
            stats["deleted"] += 1
        elif path.suffix.lower() == ".md":
            rel = path.relative_to(tree).as_posix()
            try:
                text = path.read_bytes().decode("utf-8")
            except UnicodeDecodeError:
                raise VendorError(f"strip-connectors: {rel} is not UTF-8")
            stripped = _strip_markdown(text)
            for number, line in enumerate(stripped.split("\n"), 1):
                if "~~" in line or CONNECTORS_FILE in line:
                    raise VendorError(f"strip-connectors: {rel}:{number}: connector markup the "
                                      f"transform does not handle: {line.strip()[:100]}")
            if stripped != text:
                path.write_bytes(stripped.encode("utf-8"))  # keeps the file's mode
                stats["files"] += 1
    return stats


def rename_skill(tree: Path, old: str, new: str) -> dict:
    """Rename a skill: the frontmatter `name:` goes from old to new (the folder is the entry's
    target), and `/old` in the body becomes `new`, so nothing points at a built-in `/old`
    command. A longer name such as `/old-checklist` is not matched."""
    path = Path(tree) / "SKILL.md"
    if not path.is_file():
        raise VendorError("rename-skill: SKILL.md not found")
    text = path.read_text(encoding="utf-8")
    end = text.find("\n---", 4) if text.startswith("---\n") else -1
    if end < 0:
        raise VendorError("rename-skill: SKILL.md has no frontmatter")
    head = re.sub(r"^name:[ \t]*(.*?)[ \t]*$", lambda m: _renamed(m, old, new), text[:end],
                  count=1, flags=re.MULTILINE)
    body, count = re.subn(r"(?<![\w])/" + re.escape(old) + r"(?![\w-])", new, text[end:])
    path.write_text(head + body, encoding="utf-8")
    return {"name": new, "references": count}


def _renamed(match, old: str, new: str) -> str:
    if match.group(1).strip("'\"") != old:
        raise VendorError(f"rename-skill: the skill is named {match.group(1)!r}, expected {old!r}")
    return f"name: {new}"


def _run_strip_connectors(tree: Path, entry: dict, spec: dict) -> dict:
    return strip_connectors(tree)


def _run_rename_skill(tree: Path, entry: dict, spec: dict) -> dict:
    return rename_skill(tree, spec["old"], spec["new"])


TRANSFORMS = {"rename-plugin": _run_rename_plugin, "strip-connectors": _run_strip_connectors,
              "rename-skill": _run_rename_skill}


# --------------------------------------------------------------------------- tree helpers

def _norm_mode(path: Path) -> int:
    return 0o755 if os.stat(path).st_mode & 0o111 else 0o644


def _reject_dir_symlinks(dirpath: Path, dirnames: list, base: Path) -> None:
    """os.walk does not descend into symlinks to directories, so they would be dropped
    silently (and `check` could not notice). They are not supported: fail loudly."""
    for d in dirnames:
        if (dirpath / d).is_symlink():
            rel = (dirpath / d).relative_to(base).as_posix()
            raise VendorError(f"{rel}: symlinks to directories are not supported "
                              f"(not followed, not copied); remove it or handle it with a transform")


def copy_tree(src: Path, dst: Path) -> None:
    """Copy src to dst without any `.git` directory; modes normalized to 644/755."""
    dst.mkdir(parents=True, exist_ok=True)
    for dirpath, dirnames, filenames in os.walk(src):
        dirnames[:] = sorted(d for d in dirnames if d != ".git")
        _reject_dir_symlinks(Path(dirpath), dirnames, src)
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
        _reject_dir_symlinks(Path(dirpath), dirnames, tree)
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
                problems += compare_trees(built, root / e["target"], e["target"])
            except VendorError as exc:
                problems.append(f"{e['name']}: {exc}")
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
