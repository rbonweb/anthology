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

A source may name a `portmap` (a json under vendor/patches/): the script rule, the keep/port/
drop action of every upstream script and the Chrome-launch pattern. build/check/sync fail when
upstream adds a script the map does not list, a drop script ships, a keep script differs from
upstream, or any shipped file or path matches the pattern.

build also ships each source's license inside its target (after transforms and patches), so
the installed plugin carries the text: the upstream license_file unless it already lies in the
source folder, the fixed vendor/licenses text as LICENSE when license_file is null, and each
upstream path listed in the optional `license_extra` (e.g. a NOTICE), all unchanged. A NOTICE in
the upstream root or on the path to the folder must ship, or build fails.

A source with `url` + `checksum` (no git) is a downloaded skill folder. Its pristine copy
is committed in vendor/snapshots/<name>/ and re-verified on every build/check/diff, so CI
never contacts the host; only `vendor.py fetch NAME` downloads and refreshes it.
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import posixpath
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE_DIRNAME = ".vendor-cache"
SOURCES = Path("vendor") / "sources.json"
LICENSES = Path("vendor") / "licenses"
SNAPSHOTS = Path("vendor") / "snapshots"
CHECKSUMS_NAME = "CHECKSUMS.sha256"
MAX_DOWNLOAD = 10 * 1024 * 1024
ALLOWED_LICENSES = {"MIT", "Apache-2.0"}
SUBTREE = "subtree"
VENDORED_PLUGIN = "plugins/anthology-superpowers"
KNOWLEDGE_DIR = "plugins/anthology/knowledge"
IGNORED_EXTRAS = {".DS_Store"}
PORTMAP_DIR = "vendor/patches/"
NOTICE_NAMES = ("NOTICE", "NOTICE.txt", "NOTICE.md")
FIXED_LICENSE_NAME = "LICENSE"

SHA1 = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
NAME = re.compile(r"^[a-z0-9][a-z0-9-]*$")
SOURCE_URL = re.compile(r"^https://[A-Za-z0-9.-]+(?:/[A-Za-z0-9._~-]+)+/$")
CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  (.+)$")


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
    folder = value[len(KNOWLEDGE_DIR) + 1:]
    return value.startswith(KNOWLEDGE_DIR + "/") and bool(NAME.match(folder))


def _safe_portmap(value) -> bool:
    return _safe_relpath(value, False) and value.startswith(PORTMAP_DIR) and value.endswith(".json")


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


def _check_license_extra(label: str, entry: dict, errors: list) -> None:
    """`license_extra`: upstream files (NOTICE, a second license) shipped unchanged next to
    the source folder, as `<target>/<basename>`."""
    extra = entry["license_extra"]
    if entry.get("url"):
        errors.append(f"{label}: a url source has no license_extra (its snapshot is the folder)")
        return
    if not (isinstance(extra, list) and all(_safe_relpath(v, False) for v in extra)):
        errors.append(f"{label}: license_extra must be a list of upstream repo-relative paths")
        return
    license_file = entry.get("license_file")
    taken = {posixpath.basename(license_file) if isinstance(license_file, str)
             else FIXED_LICENSE_NAME}
    for value in extra:
        base = posixpath.basename(value)
        if base in taken:
            errors.append(f"{label}: license_extra {value!r} clashes with another shipped "
                          f"license file named {base}")
        taken.add(base)


def _check_url_entry(label: str, entry: dict, errors: list) -> None:
    """A downloaded skill folder: https URL ending in a slash, a pinned checksum of its
    CHECKSUMS.sha256, no git fields, and a license file inside the snapshot."""
    url = entry["url"]
    if not (isinstance(url, str) and SOURCE_URL.match(url)
            and all(part not in (".", "..") for part in url.split("/")[3:])):
        errors.append(f"{label}: url must be an https URL ending in a slash")
    if entry.get("repo") or entry.get("commit") is not None:
        errors.append(f"{label}: a url source has no repo or commit")
    if "checksum" not in entry:
        errors.append(f"{label}: a url source needs a checksum")
    if entry.get("path") != "":
        errors.append(f"{label}: a url source has an empty path")
    if not _safe_relpath(entry.get("license_file"), False):
        errors.append(f"{label}: a url source needs a license_file inside the snapshot")


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
        if "license_extra" in e:
            _check_license_extra(label, e, errors)
        if "url" in e:
            _check_url_entry(label, e, errors)
        _check_transforms(label, e, errors)
        if "portmap" in e and not _safe_portmap(e["portmap"]):
            errors.append(f"{label}: portmap must be a .json file under {PORTMAP_DIR}")
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
    - `.claude-plugin/marketplace.json` (upstream's dev marketplace) is deleted: its entry
      version would disagree with plugin.json and `claude plugin validate` would check it
      instead of the plugin.
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
    (tree / ".claude-plugin" / "marketplace.json").unlink(missing_ok=True)

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
MD_FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")
# What stays when the connector setup around it goes: "If no tool is connected", "If tools are
# not connected", "Without the tools". Heading, paragraph, list item or bold lead-in.
FALLBACK = re.compile(
    r"^\s*(?:[-*+]\s+)?(?:\*\*)?(?:if\s+(?:no|none|neither)\b.*\bconnected\b"
    r"|if\b.*\bnot\s+connected\b|without\b.*\btools?\b)", re.IGNORECASE)
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
    if FALLBACK.match(text):
        return False
    return bool(CONNECTOR_HEADING.match(text.strip())) or "~~" in text


def _fence_end(lines: list, start: int, marker: str) -> int:
    """Index of the line closing the fence opened at `start` (the last line if unclosed)."""
    for j in range(start + 1, len(lines)):
        m = MD_FENCE.match(lines[j].rstrip())
        if m and m.group(1)[0] == marker[0] and len(m.group(1)) >= len(marker) \
                and not m.group(2).strip():
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


def _fallbacks_in(lines: list, start: int, end: int, level: int) -> list:
    """The fallback text inside a connector section that is about to be removed.

    Returns chunks of lines (verbatim): every nested subsection whose heading is a fallback
    ("### If no tools are connected"), and every paragraph or list item that starts with a
    fallback lead-in ("If these tools are not connected, ...")."""
    chunks: list = []
    j = start + 1
    while j < end:
        line = lines[j].rstrip()
        m = MD_FENCE.match(line)
        if m:
            j = _fence_end(lines, j, m.group(1)) + 1
            continue
        h = MD_HEADING.match(line)
        if h and len(h.group(1)) > level and FALLBACK.match(h.group(2)):
            stop = min(_section_end(lines, j, len(h.group(1))), end)
        elif not h and FALLBACK.match(line):
            stop = min(_if_block_end(lines, j), end)
        else:
            j += 1
            continue
        chunk = lines[j:stop]
        while chunk and not chunk[-1].strip():
            chunk.pop()
        chunks.append(chunk)
        j = stop
    return chunks


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
            end = _section_end(lines, i, len(h.group(1)))
            kept = _fallbacks_in(lines, i, end, len(h.group(1)))
            if kept:  # the section goes, its fallbacks stay under the section's own heading
                out.append(line)
                out.append("")
                for chunk in kept:
                    out.extend(chunk)
                    out.append("")
            i = end
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
                                      f"transform does not handle (failing on purpose, so nothing "
                                      f"slips through; extend strip-connectors): {line.strip()[:100]}")
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
    head, found = re.subn(r"^name:[ \t]*(.*?)[ \t]*$", lambda m: _renamed(m, old, new), text[:end],
                          count=1, flags=re.MULTILINE)
    if not found:
        raise VendorError("rename-skill: SKILL.md frontmatter has no name")
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


# --------------------------------------------------------------------------- url sources

def parse_checksums(data: bytes) -> dict:
    """`<sha256>  <path>` lines (sha256sum format) -> {path: sha256}. Paths must be plain
    relative paths: they become file names on disk and in URLs."""
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        raise VendorError(f"{CHECKSUMS_NAME} is not UTF-8")
    listed: dict = {}
    for number, line in enumerate(text.splitlines(), 1):
        m = CHECKSUM_LINE.match(line)
        if not m:
            raise VendorError(f"{CHECKSUMS_NAME}:{number}: not a `<sha256>  <path>` line")
        digest, rel = m.groups()
        if (not _safe_relpath(rel, False) or rel == CHECKSUMS_NAME
                or any(ord(c) < 32 or ord(c) == 127 for c in rel)):
            raise VendorError(f"{CHECKSUMS_NAME}:{number}: unsafe path {rel!r}")
        if rel in listed:
            raise VendorError(f"{CHECKSUMS_NAME}:{number}: {rel} is listed twice")
        listed[rel] = digest
    if not listed:
        raise VendorError(f"{CHECKSUMS_NAME} lists no files")
    return listed


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _snapshot_files(tree: Path) -> list:
    """Relative paths of every file; symlinks are not allowed in a snapshot."""
    found = []
    for dirpath, dirnames, filenames in os.walk(tree):
        dirnames.sort()
        _reject_dir_symlinks(Path(dirpath), dirnames, tree)
        for name in filenames:
            p = Path(dirpath) / name
            if p.is_symlink():
                raise VendorError(f"{p.relative_to(tree).as_posix()}: symlinks are not allowed in a snapshot")
            found.append(p.relative_to(tree).as_posix())
    return sorted(found)


def verify_snapshot(entry: dict, tree: Path) -> None:
    """The tree must be exactly the files listed in CHECKSUMS.sha256 plus that file, every
    file must match its digest, and the CHECKSUMS.sha256 itself must match the pin."""
    name, tree = entry["name"], Path(tree)
    sums = tree / CHECKSUMS_NAME
    if not sums.is_file() or sums.is_symlink():
        raise VendorError(f"{name}: no snapshot at {SNAPSHOTS / name}; run `vendor.py fetch {name}`")
    raw = sums.read_bytes()
    if _sha256(raw) != entry.get("checksum"):
        raise VendorError(f"{name}: {CHECKSUMS_NAME} does not match the pinned checksum in "
                          f"vendor/sources.json")
    listed = parse_checksums(raw)
    present = set(_snapshot_files(tree)) - {CHECKSUMS_NAME}
    problems = [f"missing {rel}" for rel in sorted(set(listed) - present)]
    problems += [f"unlisted file {rel}" for rel in sorted(present - set(listed))]
    problems += [f"{rel} does not match its checksum" for rel in sorted(present & set(listed))
                 if _sha256((tree / rel).read_bytes()) != listed[rel]]
    if problems:
        raise VendorError(f"{name}: snapshot {SNAPSHOTS / name} is not what {CHECKSUMS_NAME} lists: "
                          + "; ".join(problems))


def snapshot_fetcher(root: Path):
    """Return fetch(entry) -> the committed snapshot of a url source, verified (no network)."""
    def fetch(entry: dict) -> Path:
        if not entry.get("url"):
            raise VendorError(f"{entry['name']}: not a url source")
        tree = Path(root) / SNAPSHOTS / entry["name"]
        verify_snapshot(entry, tree)
        return tree
    return fetch


class _HttpsOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.lower().startswith("https://"):
            raise VendorError(f"refusing a redirect to a non-https URL: {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def http_get(url: str, timeout: int = 30) -> bytes:
    """GET over https only (no other scheme, no redirect to one); the body is size-limited."""
    if not url.startswith("https://"):
        raise VendorError(f"refusing a non-https URL: {url}")
    request = urllib.request.Request(url, headers={"User-Agent": "anthology-vendor"})
    try:
        with urllib.request.build_opener(_HttpsOnly).open(request, timeout=timeout) as response:
            data = response.read(MAX_DOWNLOAD + 1)
    except (OSError, ValueError) as exc:  # URLError and HTTPError are OSErrors
        raise VendorError(f"cannot download {url}: {exc}")
    if len(data) > MAX_DOWNLOAD:
        raise VendorError(f"{url} is larger than {MAX_DOWNLOAD} bytes")
    return data


def _index_files(entry: dict, get) -> set:
    index_url = entry["url"].rsplit("/", 2)[0] + "/index.json"
    try:
        skills = json.loads(get(index_url).decode("utf-8"))["skills"]
        files = next(s["files"] for s in skills if s.get("name") == entry["name"])
        if not (isinstance(files, list) and all(isinstance(f, str) for f in files)):
            raise ValueError("files is not a list of paths")
    except (ValueError, KeyError, TypeError, StopIteration):
        raise VendorError(f"{entry['name']}: {index_url} has no usable file list for this skill")
    return set(files)


def download_snapshot(root: Path, entry: dict, get=None) -> Path:
    """Download a url source into vendor/snapshots/<name>/.

    The CHECKSUMS.sha256 must match the pin and the file list of the host's index.json;
    every file must match its digest. The old snapshot is replaced only after all of that."""
    errors = validate_sources([entry])
    if errors:
        raise VendorError("invalid source:\n  " + "\n  ".join(errors))
    get = get or http_get
    name, base = entry["name"], entry["url"]
    raw = get(base + CHECKSUMS_NAME)
    if _sha256(raw) != entry.get("checksum"):
        raise VendorError(f"{name}: downloaded {CHECKSUMS_NAME} does not match the pinned "
                          f"checksum; nothing was written")
    listed = parse_checksums(raw)
    advertised = _index_files(entry, get)
    expected = set(listed) | {CHECKSUMS_NAME}
    if advertised != expected:
        diff = sorted(advertised ^ expected)
        raise VendorError(f"{name}: index.json and {CHECKSUMS_NAME} disagree about: {', '.join(diff)}")
    dest = Path(root) / SNAPSHOTS / name
    parent = dest.parent
    parent.mkdir(parents=True, exist_ok=True)
    tmp, old = parent / f".tmp-{name}-{os.getpid()}", parent / f".old-{name}-{os.getpid()}"
    shutil.rmtree(tmp, ignore_errors=True)
    shutil.rmtree(old, ignore_errors=True)
    try:
        tmp.mkdir()
        for rel, digest in sorted(listed.items()):
            data = get(base + urllib.parse.quote(rel, safe="/"))
            if _sha256(data) != digest:
                raise VendorError(f"{name}: {rel} does not match its checksum; nothing was written")
            (tmp / rel).parent.mkdir(parents=True, exist_ok=True)
            (tmp / rel).write_bytes(data)
            (tmp / rel).chmod(0o644)
        (tmp / CHECKSUMS_NAME).write_bytes(raw)
        (tmp / CHECKSUMS_NAME).chmod(0o644)
        verify_snapshot(entry, tmp)
        if dest.exists():
            dest.rename(old)
        try:
            tmp.rename(dest)
        except OSError:
            if old.exists():
                old.rename(dest)
            raise
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        shutil.rmtree(old, ignore_errors=True)
    return dest


def fetch_entry(root: Path, entry: dict, fetch, get=None) -> Path:
    """What `vendor.py fetch` does for one source: url sources are downloaded into their
    snapshot, git sources are fetched into the cache."""
    return download_snapshot(root, entry, get=get) if entry.get("url") else fetch(entry)


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


# --------------------------------------------------------------------------- port maps

def load_portmap(root: Path, entry: dict) -> dict:
    """The entry's port map (`portmap` field), checked for the keys the rules need."""
    path = Path(root) / entry["portmap"]
    if not path.is_file():
        raise VendorError(f"{entry['name']}: port map {entry['portmap']} not found")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        rule = data["script_rule"]
        if not (isinstance(data["scripts"], dict) and isinstance(data.get("added", {}), dict)
                and isinstance(rule["extensions"], list)):
            raise TypeError("scripts, added or extensions have the wrong type")
        re.compile(rule["js_node_regex"])
        re.compile(data["chrome_regex"])
    except (ValueError, KeyError, TypeError, re.error) as exc:
        raise VendorError(f"{entry['name']}: port map {entry['portmap']} is not usable: {exc}")
    return data


def portmap_scripts(tree: Path, rule: dict) -> set:
    """The files of a tree the port map's script_rule counts as scripts: a listed extension
    (any case), a `#!` first line, or a `.js` file that reads like Node (not browser code)."""
    extensions = {e.lower() for e in rule["extensions"]}
    node_js = re.compile(rule["js_node_regex"])
    found = set()
    for dirpath, dirnames, filenames in os.walk(tree):
        dirnames[:] = sorted(d for d in dirnames if d != ".git")
        for name in filenames:
            path = Path(dirpath) / name
            rel = path.relative_to(tree).as_posix()
            ext = path.suffix.lower()
            if ext in extensions:
                found.add(rel)
                continue
            try:
                if ext == ".js":
                    raw = path.read_bytes()
                else:
                    with path.open("rb") as handle:
                        raw = handle.read(2)
            except OSError:
                continue
            if (rule.get("shebang") and raw[:2] == b"#!") or (
                    ext == ".js" and node_js.search(raw.decode("utf-8", "replace"))):
                found.add(rel)
    return found


def portmap_problems(portmap: dict, pristine: Path, shipped: Path) -> list:
    """Check a built tree against its port map. `pristine` is the upstream folder before any
    patch. Every problem is one line."""
    pristine, shipped = Path(pristine), Path(shipped)
    rule, scripts, added = portmap["script_rule"], portmap["scripts"], portmap.get("added", {})
    problems = []
    upstream = portmap_scripts(pristine, rule)
    for rel in sorted(upstream - set(scripts)):
        problems.append(f"{rel}: upstream script is not in the port map (add it as keep, port or drop)")
    for rel in sorted(set(scripts) - upstream):
        problems.append(f"{rel}: the port map lists it, but it is not upstream")
    expected, dropped = set(added), set()
    for rel, spec in sorted(scripts.items()):
        action = spec.get("action") if isinstance(spec, dict) else None
        if action == "drop":
            if (shipped / rel).exists():
                dropped.add(rel)
                problems.append(f"{rel}: drop script is shipped")
        elif action in ("keep", "port"):
            expected.add(rel)
            if not (shipped / rel).is_file():
                problems.append(f"{rel}: {action} script is not shipped")
            elif action == "keep" and (not (pristine / rel).is_file()
                                       or (shipped / rel).read_bytes() != (pristine / rel).read_bytes()):
                problems.append(f"{rel}: keep script is not byte-identical to upstream")
        else:
            problems.append(f"{rel}: action must be keep, port or drop")
    for rel in sorted(added):
        if not (shipped / rel).is_file():
            problems.append(f"{rel}: added script is not shipped")
        if rel in upstream or (pristine / rel).exists():
            problems.append(f"{rel}: added script already exists upstream")
    have = portmap_scripts(shipped, rule)
    for rel in sorted(have - expected - dropped):
        problems.append(f"{rel}: shipped script is not keep, port or added in the port map")
    for rel in sorted(expected - have):
        if (shipped / rel).is_file():
            problems.append(f"{rel}: shipped, but the script rule does not count it as a script")
    chrome = re.compile(portmap["chrome_regex"].encode("utf-8"), re.IGNORECASE)
    for dirpath, dirnames, filenames in os.walk(shipped):
        dirnames.sort()
        for name in sorted(filenames):
            path = Path(dirpath) / name
            rel = path.relative_to(shipped).as_posix()
            if chrome.search(rel.encode("utf-8")):
                problems.append(f"{rel}: the path matches the Chrome launch pattern")
            try:
                found = chrome.search(path.read_bytes())
            except OSError:
                continue
            if found:
                problems.append(f"{rel}: the file matches the Chrome launch pattern "
                                f"({found.group().decode('utf-8', 'replace')!r})")
    return problems


# --------------------------------------------------------------------------- build

def _pristine_subtree(entry: dict, pristine: Path) -> Path:
    src = (Path(pristine) / entry["path"]).resolve()
    if not src.is_dir() or Path(pristine).resolve() not in (src, *src.parents):
        raise VendorError(f"{entry['name']}: path {entry['path']!r} is not a directory upstream")
    return src


def _ship_file(dest: Path, name: str, data: bytes, entry: dict) -> None:
    """Write a license file to dest/name (mode 644). A file already there is fine when it has
    the same bytes (uxkin folders ship their own LICENSE) and an error when it differs."""
    target = Path(dest) / name
    if target.is_symlink() or target.exists():
        if target.is_symlink() or not target.is_file() or target.read_bytes() != data:
            raise VendorError(f"{entry['name']}: {name} already exists in the source folder "
                              f"and differs from the license text to ship")
        return
    target.write_bytes(data)
    target.chmod(0o644)


def _upstream_extra(entry: dict, pristine: Path, rel: str) -> bytes:
    src = Path(pristine) / rel
    if not src.is_file():
        raise VendorError(f"{entry['name']}: upstream license_extra file {rel} not found")
    data = src.read_bytes()
    if not data.strip():
        raise VendorError(f"{entry['name']}: upstream license_extra file {rel} is empty")
    return data


def _notice_problems(entry: dict, pristine: Path, dest: Path) -> list:
    """Every NOTICE in the upstream repo root or on the way down to the source folder must
    ship: inside the folder, or listed in license_extra (Apache-2.0 section 4(d))."""
    folder = entry["path"]
    parts = folder.split("/") if folder else []
    chain = ["/".join(parts[:i]) for i in range(len(parts) + 1)]
    listed = set(entry.get("license_extra", []))
    problems = []
    for directory in chain:
        here = Path(pristine) / directory if directory else Path(pristine)
        try:
            present = set(os.listdir(here))
        except OSError:
            continue
        for name in NOTICE_NAMES:
            rel = f"{directory}/{name}" if directory else name
            if name not in present or not (here / name).is_file():
                continue
            if directory == folder:
                if not (Path(dest) / name).is_file():
                    problems.append(f"{entry['name']}: upstream {rel} is not shipped "
                                    f"(a patch removed it)")
            elif rel not in listed:
                problems.append(f"{entry['name']}: upstream {rel} is not shipped; "
                                f"add it to license_extra")
    return problems


def ship_licenses(root: Path, entry: dict, pristine: Path, dest: Path) -> None:
    """Put the source's license text, and any upstream NOTICE, inside the built folder: Claude
    Code installs a plugin directory on its own, so the text must travel with it. Files are
    copied unchanged."""
    folder, license_file = entry["path"], entry.get("license_file")
    if license_file is None:
        fixed = Path(root) / LICENSES / f"{source_slug(entry)}.txt"
        if not fixed.is_file():
            raise VendorError(f"{entry['name']}: fixed license text {LICENSES / fixed.name} not found")
        _ship_file(dest, FIXED_LICENSE_NAME, fixed.read_bytes(), entry)
    else:
        prefix = folder + "/" if folder else ""
        if license_file.startswith(prefix):
            if not (Path(dest) / license_file[len(prefix):]).is_file():
                raise VendorError(f"{entry['name']}: license file {license_file} is not in "
                                  f"the built folder")
        else:
            _ship_file(dest, posixpath.basename(license_file),
                       _upstream_license(entry, pristine), entry)
    for rel in entry.get("license_extra", []):
        _ship_file(dest, posixpath.basename(rel), _upstream_extra(entry, pristine, rel), entry)
    if entry.get("repo"):
        problems = _notice_problems(entry, pristine, dest)
        if problems:
            raise VendorError("\n".join(problems))


def build_tree(root: Path, entry: dict, pristine: Path, dest: Path,
               patches: bool = True, reject: bool = False) -> dict:
    """pristine -> transforms -> patches -> licenses, written to dest. Returns stats and rejects."""
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
    ship_licenses(root, entry, pristine, dest)
    if patches and entry.get("portmap"):
        stats["portmap"] = portmap_problems(load_portmap(root, entry),
                                            _pristine_subtree(entry, pristine), dest)
    return stats


def install_tree(src: Path, dest: Path) -> None:
    if dest.exists() or dest.is_symlink():
        shutil.rmtree(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    copy_tree(src, dest)


def dispatch_fetcher(root: Path, git_fetch):
    """url sources come from their committed snapshot, everything else from git."""
    snapshot_fetch = snapshot_fetcher(root)
    return lambda entry: snapshot_fetch(entry) if entry.get("url") else git_fetch(entry)


def _fetcher(root: Path, fetch):
    return fetch or dispatch_fetcher(root, git_fetcher(Path(root) / CACHE_DIRNAME))


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
        if stats.get("portmap"):
            raise VendorError(f"{name}: the port map is not satisfied:\n  " + "\n  ".join(stats["portmap"]))
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
                stats = build_tree(root, e, fetch(e), built)
                problems += [f"{e['name']}: portmap: {m}" for m in stats.get("portmap", [])]
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
    if entry.get("url"):
        raise VendorError(f"{name} is a url source and has no ref to sync: update its checksum in "
                          f"vendor/sources.json, then run `vendor.py fetch {name}`")
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
            "transforms": stats["transforms"], "rejects": stats["rejects"],
            "portmap": stats.get("portmap", [])}


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
    fetch = dispatch_fetcher(root, git_fetcher(args.cache or root / CACHE_DIRNAME))
    try:
        if args.command == "fetch":
            names = _names(root, args.names)
            for n in names:
                print(f"fetched {n}: {fetch_entry(root, find_entry(load_sources(root), n), fetch)}")
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
            for r in report["portmap"]:
                print(f"PORTMAP {r}", file=sys.stderr)
            print("Rebuilt the target and refreshed the license. Nothing was committed.\n"
                  "Review the diff; if the output changed, bump plugin_version in sources.json "
                  "where it applies.")
            return 1 if report["rejects"] or report["portmap"] else 0
        elif args.command == "diff":
            sys.stdout.write(diff_entry(root, args.name, fetch=fetch))
    except VendorError as exc:
        print(f"vendor.py: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
