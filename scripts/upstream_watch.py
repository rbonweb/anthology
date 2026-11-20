#!/usr/bin/env python3
"""Upstream watcher: compare every pin against upstream and keep one issue per change.

Usage: upstream_watch.py [--dry-run] [--json OUT] [--root PATH]

Watches, generically, every entry of `vendor/sources.json` (git sources pinned to a tag or a
branch, url + checksum sources, subtree sources) plus the Claude Code version pinned in
`ci/claude-code/package.json`. For each upstream change it opens one GitHub issue (label
`upstream-watch`, hidden marker `<!-- upstream-watch:<key> -->` in the body) or updates the
open one. Only issues authored by the Actions bot are managed, and per key the newest decides:
a closed issue whose body still matches means the release was dismissed (nothing is written);
a closed issue with an older target gets a new issue. Issues are never closed here. Nothing is ever merged or vendored: the issue names
the command to run on a branch, and CI plus the evals are the gate.

`--dry-run` checks everything and prints the issues it would create or update; it makes no
write call. A real run needs GITHUB_TOKEN and GITHUB_REPOSITORY (GITHUB_API_URL optional).
When GITHUB_OUTPUT is set, a real run appends `claude_code_changed=true|false` (true only when
the Claude Code issue was created or updated this run).

Exit codes: 0 done, 1 at least one entry (or issue write) failed, 2 bad usage or missing
credentials.

Everything that comes from upstream (tags, shas, file paths) is untrusted: it is validated
against strict patterns, file paths are stripped of control characters and backticks and
rendered inside one fenced block, and commit messages or author names are never used. Issue
text only ever travels as JSON over the API, never through a shell.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import unicodedata
import urllib.request
from urllib.parse import urlparse
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

LABEL = "upstream-watch"
CLAUDE_CODE_KEY = "claude-code"
CLAUDE_CODE_PACKAGE = "@anthropic-ai/claude-code"
CLAUDE_CODE_LATEST_URL = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest"
CHANGELOG_URL = "https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md"
USER_AGENT = "anthology-upstream-watch"
GITHUB_API = "https://api.github.com"
HTTP_TIMEOUT = 30
MAX_RESPONSE_BYTES = 20 * 1024 * 1024
MAX_FILES_SHOWN = 50
MAX_PATH_LEN = 200
MAX_VERSION_LEN = 64
BOT_LOGIN = "github-actions[bot]"  # the only author whose issues the watcher manages
COMPARE_FILE_LIMIT = 300  # the compare API returns at most this many files

TAG_PIN_RE = re.compile(r"^(v?)([0-9]+)\.([0-9]+)(?:\.([0-9]+))?$")
SAFE_TAG_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$")
SAFE_BRANCH_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+/-]{0,127}$")
SAFE_NAME_RE = re.compile(r"^[A-Za-z0-9._-]+$")
SAFE_PATH_RE = re.compile(r"^[A-Za-z0-9._/-]*$")
SHA1_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
REPO_RE = re.compile(r"^https://[A-Za-z0-9.-]+(?::[0-9]+)?/[A-Za-z0-9._~/-]+$")
GITHUB_REPO_RE = re.compile(r"^https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+?)(?:\.git)?/?$")
HTTPS_URL_RE = re.compile(r"^https://[A-Za-z0-9.-]+(?::[0-9]+)?(?:/[A-Za-z0-9._~%+@=:-]*)*$")
MARKER_RE = re.compile(r"^<!-- upstream-watch:([A-Za-z0-9._-]+) -->")
MANIFEST_LINE_RE = re.compile(r"^([0-9a-fA-F]{64})[ \t]+\*?(.+)$")


class WatchError(Exception):
    """A failure attached to one watched key; the run continues with the others."""


def _load_semver():
    path = Path(__file__).resolve().parent / "check_version_bump.py"
    spec = importlib.util.spec_from_file_location("_anthology_check_version_bump", str(path))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_SEMVER = _load_semver()


# ---- untrusted text --------------------------------------------------------------------

def sanitise_path(text) -> str:
    """Strip control/format characters and backticks; cap the length. Safe inside a code fence."""
    out = []
    for ch in str(text):
        category = unicodedata.category(ch)
        if ch == "`" or category[0] == "C" or category in ("Zl", "Zp"):
            continue
        out.append(ch)
    return "".join(out).strip()[:MAX_PATH_LEN]


def sanitise_inline(text) -> str:
    """Like sanitise_path, for a value shown inside an inline code span."""
    return sanitise_path(text)


def _expect(pattern, value, what: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise WatchError(f"{what} is not in the expected format")
    return value


def validate_name(name) -> str:
    return _expect(SAFE_NAME_RE, name, "source name")


def validate_sha(value, what: str) -> str:
    return _expect(SHA1_RE, value, what)


# ---- data ------------------------------------------------------------------------------

@dataclass
class Change:
    """One detected upstream change. `old`/`new` are the short labels used in the title."""

    key: str
    kind: str
    old: str
    new: str
    new_target: str
    command: str
    details: List[Tuple[str, str]] = field(default_factory=list)
    files: List[str] = field(default_factory=list)
    files_total: int = 0
    files_heading: str = "Changed files"
    notes: List[str] = field(default_factory=list)

    def to_json(self) -> dict:
        return {
            "key": self.key, "kind": self.kind, "old": self.old, "new": self.new,
            "new_target": self.new_target, "title": render_title(self),
            "command": self.command, "details": [list(d) for d in self.details],
            "files": list(self.files), "files_total": max(self.files_total, len(self.files)),
            "notes": list(self.notes),
        }


# ---- rendering -------------------------------------------------------------------------

def render_title(change: Change) -> str:
    return f"Upstream: {change.key} {change.old} → {change.new}"


def render_body(change: Change, extra_sections: Sequence[Tuple[str, str]] = ()) -> str:
    """Render the issue body for `change`.

    `extra_sections` is a sequence of `(heading, markdown)` pairs appended, in order, after the
    summary and before the update command. It lets a caller attach more context to the same
    issue (Task 14b adds the startup-budget delta this way) without touching this function.
    """
    lines = [f"<!-- upstream-watch:{change.key} -->",
             f"## Upstream change: `{sanitise_inline(change.key)}`", ""]
    for label, value in change.details:
        lines.append(f"- **{sanitise_inline(label)}:** `{sanitise_inline(value)}`")
    summary: List[str] = [f"- {note}" for note in change.notes]
    files = [f for f in (sanitise_path(f) for f in change.files) if f]
    if files:
        total = max(change.files_total, len(files))
        # The heading is built by this module (its own backticks around the path stay).
        summary.append(f"- {' '.join(change.files_heading.split())} ({total}):")
        summary.append("")
        summary.append("```text")
        summary.extend(files[:MAX_FILES_SHOWN])
        summary.append("```")
        if total > MAX_FILES_SHOWN:
            summary.append(f"...and {total - MAX_FILES_SHOWN} more.")
    if summary:
        lines += ["", "### Summary", ""] + summary
    for heading, markdown in extra_sections:
        lines += ["", f"### {' '.join(str(heading).split())}", str(markdown).strip("\n")]
    lines += ["", "### Update", "",
              "Run this on a branch. CI and the evals are the gate: do not merge without them.",
              "", "```sh", change.command, "```", ""]
    return "\n".join(lines)


# ---- parsing helpers -------------------------------------------------------------------

def parse_ls_remote(text: str) -> Dict[str, str]:
    """`git ls-remote` output -> {refname: sha}; malformed lines are skipped."""
    refs: Dict[str, str] = {}
    for line in text.splitlines():
        sha, sep, ref = line.partition("\t")
        if sep and SHA1_RE.fullmatch(sha) and ref:
            refs[ref] = sha
    return refs


def _version_tuple(match) -> Tuple[int, int, int]:
    return (int(match.group(2)), int(match.group(3)), int(match.group(4) or 0))


def latest_stable_tag(refs: Dict[str, str], pinned_ref: str) -> Optional[Tuple[str, str]]:
    """Highest stable semver tag sharing the pin's `v` prefix convention, with its commit."""
    pin = TAG_PIN_RE.fullmatch(pinned_ref)
    best = None
    for ref in refs:
        if not ref.startswith("refs/tags/") or ref.endswith("^{}"):
            continue
        name = ref[len("refs/tags/"):]
        m = TAG_PIN_RE.fullmatch(name)
        if not m or m.group(1) != pin.group(1) or not SAFE_TAG_RE.fullmatch(name):
            continue
        version = _version_tuple(m)
        if best is None or version > best[0]:
            best = (version, name)
    if best is None:
        return None
    name = best[1]
    return name, refs.get("refs/tags/" + name + "^{}") or refs["refs/tags/" + name]


def classify(entry) -> str:
    """'git', 'url' or 'subtree'; anything else is an unknown shape."""
    if not isinstance(entry, dict):
        raise WatchError("unrecognised source shape: not an object")
    if isinstance(entry.get("upstream"), dict) or entry.get("managed_by") == "subtree":
        if not isinstance(entry.get("upstream"), dict):
            raise WatchError("unrecognised source shape: subtree entry without `upstream`")
        return "subtree"
    if "url" in entry and "checksum" in entry:
        return "url"
    if "repo" in entry:
        return "git"
    raise WatchError("unrecognised source shape: no repo, url or upstream")


def parse_manifest(data: bytes) -> Dict[str, str]:
    out: Dict[str, str] = {}
    for line in data.decode("utf-8", "replace").splitlines():
        m = MANIFEST_LINE_RE.fullmatch(line.strip())
        if m:
            out[m.group(2).strip()] = m.group(1).lower()
    return out


def _under(path: str, filename) -> bool:
    if not isinstance(filename, str):
        return False
    prefix = path.strip("/")
    return not prefix or filename == prefix or filename.startswith(prefix + "/")


# ---- I/O (real implementations) ----------------------------------------------------------

class _DropAuthOffHost(urllib.request.HTTPRedirectHandler):
    """Follow redirects, but never carry `Authorization` to another host or scheme."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is not None:
            old_parts, new_parts = urlparse(req.full_url), urlparse(new.full_url)
            if (old_parts.scheme, old_parts.netloc) != (new_parts.scheme, new_parts.netloc):
                for store in (new.headers, new.unredirected_hdrs):
                    for name in [n for n in store if n.lower() == "authorization"]:
                        del store[name]
        return new


def _open(request):
    """urlopen with a timeout and the redirect handler above (built per call: honours env proxies)."""
    return urllib.request.build_opener(_DropAuthOffHost()).open(request, timeout=HTTP_TIMEOUT)


def git_env() -> Dict[str, str]:
    """A minimal environment for git: no GITHUB_TOKEN/GH_TOKEN reaches third-party hosts."""
    env = {"LANG": "C", "GIT_TERMINAL_PROMPT": "0"}
    for name in ("PATH", "HOME"):
        if os.environ.get(name):
            env[name] = os.environ[name]
    return env


class RealNet:
    """git ls-remote through subprocess (no shell) and urllib with a timeout."""

    def ls_remote(self, repo: str) -> Dict[str, str]:
        proc = subprocess.run(
            ["git", "ls-remote", "--", repo, "refs/tags/*", "refs/heads/*"],
            capture_output=True, text=True, timeout=120, env=git_env(), check=False)
        if proc.returncode != 0:
            raise WatchError(f"git ls-remote failed (exit {proc.returncode})")
        return parse_ls_remote(proc.stdout)

    def http_get(self, url: str, headers: Optional[Dict[str, str]] = None) -> bytes:
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, **(headers or {})})
        with _open(request) as response:
            data = response.read(MAX_RESPONSE_BYTES + 1)
        if len(data) > MAX_RESPONSE_BYTES:
            raise WatchError("response too large")
        return data


class GitHubClient:
    """The few issue calls the watcher needs; JSON in, JSON out."""

    def __init__(self, repo: str, token: str, api_base: str = GITHUB_API):
        self.repo = repo
        self.token = token
        self.api_base = api_base.rstrip("/")

    def _request(self, method: str, path: str, payload=None):
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(self.api_base + path, data=data, method=method, headers={
            "Authorization": "Bearer " + self.token,
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": USER_AGENT,
            "Content-Type": "application/json",
        })
        with _open(request) as response:
            return json.loads(response.read(MAX_RESPONSE_BYTES).decode("utf-8"))

    def list_issues(self) -> List[dict]:
        """Every labelled issue, open and closed (pull requests included; callers skip them)."""
        issues: List[dict] = []
        for page in range(1, 51):
            batch = self._request(
                "GET", f"/repos/{self.repo}/issues?labels={LABEL}&state=all&per_page=100&page={page}")
            issues.extend(batch)
            if len(batch) < 100:
                break
        return issues

    def create_issue(self, title: str, body: str) -> dict:
        return self._request("POST", f"/repos/{self.repo}/issues",
                             {"title": title, "body": body, "labels": [LABEL]})

    def update_issue(self, number: int, title: str, body: str) -> dict:
        return self._request("PATCH", f"/repos/{self.repo}/issues/{int(number)}",
                             {"title": title, "body": body})


# ---- comparison ------------------------------------------------------------------------

class Checker:
    """Compares pins with upstream. All I/O goes through `net`, so tests inject fakes."""

    def __init__(self, root, net, token: Optional[str] = None):
        self.root = Path(root)
        self.net = net
        self.token = token
        self._refs_cache: Dict[str, object] = {}  # repo -> refs dict, or the WatchError it failed with
        self._compare_cache: Dict[Tuple[str, str, str], dict] = {}

    # -- plumbing

    def _refs(self, repo: str) -> Dict[str, str]:
        if repo not in self._refs_cache:
            try:
                self._refs_cache[repo] = self.net.ls_remote(repo)
            except WatchError as exc:
                self._refs_cache[repo] = exc
            except Exception as exc:
                self._refs_cache[repo] = WatchError(f"cannot list refs ({type(exc).__name__})")
        cached = self._refs_cache[repo]
        if isinstance(cached, WatchError):  # an unreachable repo is not retried for every entry
            raise cached
        return cached

    def _get_json(self, url: str, headers: Optional[Dict[str, str]] = None):
        try:
            return json.loads(self.net.http_get(url, headers).decode("utf-8"))
        except WatchError:
            raise
        except Exception as exc:
            raise WatchError(f"request failed ({type(exc).__name__})")

    def _compare(self, repo: str, old: str, new: str) -> dict:
        """GitHub compare API for `old...new`, cached per (repo, old, new)."""
        key = (repo, old, new)
        if key not in self._compare_cache:
            m = GITHUB_REPO_RE.fullmatch(repo)
            if not m:
                raise WatchError("not a GitHub repository")
            url = f"{GITHUB_API}/repos/{m.group(1)}/{m.group(2)}/compare/{old}...{new}"
            headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
            if self.token:
                headers["Authorization"] = "Bearer " + self.token
            data = self._get_json(url, headers)
            if not isinstance(data, dict) or not isinstance(data.get("files"), list):
                raise WatchError("unexpected compare response")
            self._compare_cache[key] = data
        return self._compare_cache[key]

    @staticmethod
    def _files_under(data: dict, path: str) -> Tuple[List[str], bool]:
        """(file names under `path`, truncated?) from a compare response."""
        files = [f for f in data["files"] if isinstance(f, dict)]
        truncated = len(data["files"]) >= COMPARE_FILE_LIMIT
        names = []
        for f in files:
            if _under(path, f.get("filename")) or _under(path, f.get("previous_filename")):
                name = sanitise_path(f.get("filename", ""))
                if name:
                    names.append(name)
        return names, truncated

    # -- git sources (and subtree upstreams)

    def check_entry(self, entry) -> Optional[Change]:
        kind = classify(entry)
        name = validate_name(entry.get("name"))
        if kind == "url":
            return self._check_url(name, entry)
        if kind == "subtree":
            return self._check_git(name, kind, entry["upstream"], "")
        path = entry.get("path", "")
        if not isinstance(path, str) or not SAFE_PATH_RE.fullmatch(path):
            raise WatchError("path is not in the expected format")
        return self._check_git(name, kind, entry, path)

    def _check_git(self, name: str, kind: str, spec: dict, path: str) -> Optional[Change]:
        repo = _expect(REPO_RE, spec.get("repo"), "repo")
        ref = spec.get("ref")
        pinned = validate_sha(spec.get("commit"), "pinned commit")
        refs = self._refs(repo)
        if isinstance(ref, str) and TAG_PIN_RE.fullmatch(ref):
            return self._check_tag(name, kind, repo, ref, pinned, path, refs)
        _expect(SAFE_BRANCH_RE, ref, "ref")
        return self._check_branch(name, kind, repo, ref, pinned, path, refs)

    @staticmethod
    def _command(name: str, kind: str, new_ref: str) -> str:
        if kind == "subtree":
            return f"scripts/sync-rtk.sh {new_ref}"
        return f"python3 scripts/vendor.py sync {name} --ref {new_ref}"

    @staticmethod
    def _repo_details(repo: str, path: str, old_commit: str, new_commit: str) -> List[Tuple[str, str]]:
        details = [("Source", repo)]
        if path:
            details.append(("Path", path))
        m = GITHUB_REPO_RE.fullmatch(repo)
        if m:
            details.append(("Compare", f"https://github.com/{m.group(1)}/{m.group(2)}/compare/"
                                       f"{old_commit}...{new_commit}"))
        return details

    def _check_tag(self, name, kind, repo, ref, pinned, path, refs) -> Optional[Change]:
        latest = latest_stable_tag(refs, ref)
        if latest is None:
            raise WatchError("no stable version tags found upstream")
        tag, new_commit = latest
        if _version_tuple(TAG_PIN_RE.fullmatch(tag)) <= _version_tuple(TAG_PIN_RE.fullmatch(ref)):
            return None
        validate_sha(new_commit, "upstream tag commit")
        change = Change(
            key=name, kind=kind, old=ref, new=tag, new_target=tag,
            command=self._command(name, kind, tag),
            details=[("Pinned", f"{ref} ({pinned})"), ("Latest stable", f"{tag} ({new_commit})")]
            + self._repo_details(repo, path, pinned, new_commit))
        if GITHUB_REPO_RE.fullmatch(repo):
            try:
                self._summarise(change, self._compare(repo, pinned, new_commit), path, tag_diff=True)
            except WatchError:
                change.notes.append("Diff summary unavailable (GitHub compare API request failed).")
        return change

    def _check_branch(self, name, kind, repo, ref, pinned, path, refs) -> Optional[Change]:
        head = refs.get("refs/heads/" + ref)
        if head is None:
            raise WatchError("branch not found upstream")
        validate_sha(head, "upstream head")
        if head == pinned:
            return None
        change = Change(
            key=name, kind=kind, old=pinned[:12], new=head[:12], new_target=head,
            command=self._command(name, kind, ref),
            details=[("Branch", ref), ("Pinned commit", pinned), ("Upstream head", head)]
            + self._repo_details(repo, path, pinned, head))
        if not GITHUB_REPO_RE.fullmatch(repo):
            change.notes.append("Not a GitHub repository: only the commit range is shown.")
            return change
        if not self._summarise(change, self._compare(repo, pinned, head), path, tag_diff=False):
            return None
        return change

    def _summarise(self, change: Change, data: dict, path: str, tag_diff: bool) -> bool:
        """Fill the change's summary from a compare response; False when nothing under `path`."""
        names, truncated = self._files_under(data, path)
        total = data.get("total_commits")
        if isinstance(total, int) and not isinstance(total, bool) and total >= 0:
            change.notes.append(f"{total} commit(s) between the pinned and the new commit.")
        if truncated:
            change.notes.append("The compare API truncated the file list: diff too large to summarise.")
            return True
        scope = f"under `{sanitise_path(path)}`" if path else "in the repository"
        change.files = names
        change.files_total = len(names)
        change.files_heading = "Changed files " + scope
        if not names:
            if not tag_diff:
                return False
            change.notes.append(f"No files {scope} changed between the pinned and the new tag.")
        return True

    # -- url sources

    def _check_url(self, name: str, entry: dict) -> Optional[Change]:
        url = _expect(HTTPS_URL_RE, entry.get("url"), "url")
        checksum = _expect(SHA256_RE, entry.get("checksum"), "checksum")
        manifest_url = url if url.endswith(".sha256") else url.rstrip("/") + "/CHECKSUMS.sha256"
        try:
            data = self.net.http_get(manifest_url, None)
        except Exception as exc:
            raise WatchError(f"cannot fetch the manifest ({type(exc).__name__})")
        new_sum = hashlib.sha256(data).hexdigest()
        if new_sum == checksum:
            return None
        change = Change(
            key=name, kind="url", old=checksum[:12], new=new_sum[:12], new_target=new_sum,
            command=(f'set "checksum" of {name} in vendor/sources.json to {new_sum}, then run: '
                     f"python3 scripts/vendor.py fetch {name} && python3 scripts/vendor.py build {name}"),
            details=[("Manifest", manifest_url), ("Pinned checksum", checksum),
                     ("New checksum", new_sum)])
        snapshot = self.root / "vendor" / "snapshots" / name / "CHECKSUMS.sha256"
        new_files = parse_manifest(data)
        if not snapshot.is_file():
            change.notes.append(f"No snapshot manifest at vendor/snapshots/{name}/CHECKSUMS.sha256: "
                                "file-level diff unavailable.")
        elif not new_files:
            change.notes.append("The new manifest is not in sha256sum format: file-level diff unavailable.")
        else:
            old_files = parse_manifest(snapshot.read_bytes())
            added = sorted(set(new_files) - set(old_files))
            removed = sorted(set(old_files) - set(new_files))
            changed = sorted(f for f in set(old_files) & set(new_files) if old_files[f] != new_files[f])
            lines = [f"+ {f}" for f in added] + [f"- {f}" for f in removed] + [f"~ {f}" for f in changed]
            change.files = [line for line in (sanitise_path(x) for x in lines) if line]
            change.files_total = len(change.files)
            change.files_heading = "Files added (+), removed (-) or changed (~)"
        return change

    # -- Claude Code

    def check_claude_code(self) -> Optional[Change]:
        try:
            pkg = json.loads((self.root / "ci" / "claude-code" / "package.json").read_text(encoding="utf-8"))
            pinned = pkg["dependencies"][CLAUDE_CODE_PACKAGE]
        except (OSError, ValueError, KeyError, TypeError):
            raise WatchError("cannot read the pinned Claude Code version")
        latest = self._get_json(CLAUDE_CODE_LATEST_URL)
        latest = latest.get("version") if isinstance(latest, dict) else None
        if isinstance(latest, str) and len(latest) > MAX_VERSION_LEN:
            raise WatchError("Claude Code version from the registry is too long")
        if _SEMVER.parse_semver(pinned) is None or _SEMVER.parse_semver(latest) is None:
            raise WatchError("Claude Code version is not valid semver")
        if _SEMVER.compare_semver(latest, pinned) <= 0:
            return None
        return Change(
            key=CLAUDE_CODE_KEY, kind="claude-code", old=pinned, new=latest, new_target=latest,
            command=(f'set "{CLAUDE_CODE_PACKAGE}" in ci/claude-code/package.json to {latest}, run '
                     "npm install --package-lock-only in ci/claude-code, and let CI (plugin-validate) "
                     "and evals run on the PR"),
            details=[("Pinned", pinned), ("Latest release", latest), ("Changelog", CHANGELOG_URL)])


def check_all(root, net, token: Optional[str] = None) -> Tuple[List[Change], List[dict]]:
    """Check every source and Claude Code. Failures are collected per key; the run continues."""
    checker = Checker(root, net, token)
    changes: List[Change] = []
    errors: List[dict] = []

    def attempt(key: str, fn):
        try:
            change = fn()
        except Exception as exc:  # one entry must never stop the others
            message = str(exc) if isinstance(exc, WatchError) else f"{type(exc).__name__}"
            errors.append({"key": key, "error": message})
            return
        if change is not None:
            changes.append(change)

    try:
        data = json.loads((Path(root) / "vendor" / "sources.json").read_text(encoding="utf-8"))
        entries = data["sources"]
        if not isinstance(entries, list):
            raise TypeError
    except (OSError, ValueError, KeyError, TypeError):
        errors.append({"key": "sources.json", "error": "cannot read vendor/sources.json"})
        entries = []
    for entry in entries:
        key = entry.get("name") if isinstance(entry, dict) and isinstance(entry.get("name"), str) else "?"
        attempt(sanitise_inline(key), lambda e=entry: checker.check_entry(e))
    attempt(CLAUDE_CODE_KEY, checker.check_claude_code)
    return changes, errors


# ---- issue sync ------------------------------------------------------------------------

def _normal(text) -> str:
    return (text or "").replace("\r\n", "\n").strip()


def _has_label(issue: dict) -> bool:
    labels = issue.get("labels")
    if not isinstance(labels, list):
        return False
    return any((l.get("name") if isinstance(l, dict) else l) == LABEL for l in labels)


def _is_ours(issue) -> bool:
    """An issue the watcher manages: bot-authored, labelled, not a pull request."""
    if not isinstance(issue, dict) or "pull_request" in issue or not _has_label(issue):
        return False
    user = issue.get("user")
    return isinstance(user, dict) and user.get("login") == BOT_LOGIN


def index_issues(issues) -> Dict[str, dict]:
    """The newest (highest number) managed issue per marker key, open or closed.

    Pull requests, issues without the label, issues not authored by the Actions bot and bodies
    that do not start with the marker are ignored.
    """
    index: Dict[str, dict] = {}
    for issue in sorted((i for i in issues if _is_ours(i)), key=lambda i: i.get("number", 0)):
        m = MARKER_RE.match(_normal(issue.get("body")))
        if m:
            index[m.group(1)] = issue  # ascending order: the last one is the newest
    return index


def sync_issues(changes: Sequence[Change], gh, dry_run: bool = False,
                extra_sections: Optional[Dict[str, Sequence[Tuple[str, str]]]] = None) -> List[dict]:
    """Create or update one issue per change. Returns one action record per change.

    Per key the newest managed issue decides. Open: identical -> unchanged, else update. Closed
    with an identical body: the release was dismissed, nothing is written ("dismissed"). Closed
    with a different body: a new issue is created. Other actions: error, and (dry run without a
    client) create-or-update. In a dry run the client is only read from, never written to.
    """
    actions: List[dict] = []
    index: Optional[Dict[str, dict]] = None
    if gh is not None:
        try:
            index = index_issues(gh.list_issues())
        except Exception as exc:
            return [{"key": c.key, "action": "error", "error": f"cannot list issues ({type(exc).__name__})"}
                    for c in changes]
    for change in changes:
        title = render_title(change)
        body = render_body(change, (extra_sections or {}).get(change.key, ()))
        record = {"key": change.key, "title": title, "body": body}
        if index is None:
            record["action"] = "create-or-update"
        else:
            existing = index.get(change.key)
            if existing is None:
                record["action"] = "create"
            elif existing.get("state") == "closed":
                if _normal(existing.get("body")) == _normal(body):
                    record.update(action="dismissed", number=existing["number"])
                else:
                    record["action"] = "create"
            elif _normal(existing.get("body")) == _normal(body) and existing.get("title") == title:
                record.update(action="unchanged", number=existing["number"])
            else:
                record.update(action="update", number=existing["number"])
        if not dry_run and record["action"] in ("create", "update"):
            try:
                if record["action"] == "create":
                    record["number"] = gh.create_issue(title, body).get("number")
                else:
                    gh.update_issue(record["number"], title, body)
            except Exception as exc:
                record.update(action="error", error=f"issue write failed ({type(exc).__name__})")
        actions.append(record)
    return actions


# ---- CLI -------------------------------------------------------------------------------

def _print_report(out, changes, actions, errors, dry_run) -> None:
    if not changes and not errors:
        print("No upstream changes.", file=out)
    by_key = {a["key"]: a for a in actions}
    for change in changes:
        action = by_key.get(change.key, {}).get("action", "?")
        verb = ("would " + action) if dry_run else action
        print(f"[{verb}] {render_title(change)}", file=out)
        if dry_run:
            print("-" * 72, file=out)
            print(by_key.get(change.key, {}).get("body") or render_body(change), file=out)
            print("-" * 72, file=out)
    for error in errors:
        print(f"ERROR {error['key']}: {error['error']}", file=out)


def main(argv: Optional[Sequence[str]] = None, env=None, net=None, gh=None, stdout=None) -> int:
    parser = argparse.ArgumentParser(description="Compare every upstream pin and keep one issue per change.")
    parser.add_argument("--dry-run", action="store_true",
                        help="print the issues that would be created or updated; write nothing")
    parser.add_argument("--json", metavar="OUT", help="write the changes and errors as JSON")
    parser.add_argument("--root", default=str(Path(__file__).resolve().parent.parent),
                        help="repository root (default: this checkout)")
    args = parser.parse_args(argv)
    env = os.environ if env is None else env
    out = stdout or sys.stdout
    token, repo = env.get("GITHUB_TOKEN"), env.get("GITHUB_REPOSITORY")

    if gh is None and token and repo:
        gh = GitHubClient(repo, token, env.get("GITHUB_API_URL") or GITHUB_API)
    if gh is None and not args.dry_run:
        print("GITHUB_TOKEN and GITHUB_REPOSITORY are required (or use --dry-run).", file=out)
        return 2

    changes, errors = check_all(args.root, net or RealNet(), token)
    actions = sync_issues(changes, gh, dry_run=args.dry_run)
    errors += [{"key": a["key"], "error": a["error"]} for a in actions if a["action"] == "error"]
    _print_report(out, changes, actions, errors, args.dry_run)

    if args.json:
        Path(args.json).write_text(json.dumps({
            "changes": [c.to_json() for c in changes], "errors": errors,
            "issues": [{k: v for k, v in a.items() if k != "body"} for a in actions],
        }, indent=2) + "\n", encoding="utf-8")
    if env.get("GITHUB_OUTPUT") and not args.dry_run:
        flagged = any(a["key"] == CLAUDE_CODE_KEY and a["action"] in ("create", "update") for a in actions)
        with open(env["GITHUB_OUTPUT"], "a", encoding="utf-8") as handle:
            handle.write(f"claude_code_changed={'true' if flagged else 'false'}\n")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
