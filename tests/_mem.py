"""Shared helpers for the memory tests (isolated environment, temp git repos, note files)."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent  # not _repo: that needs PyYAML, these tests do not

SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
BIN = ROOT / "plugins" / "anthology" / "bin"
sys.path.insert(0, str(SCRIPTS))

HAVE_GIT = shutil.which("git") is not None
needs_git = unittest.skipUnless(HAVE_GIT, "git is not installed")

GIT_ID = ["-c", "user.name=t", "-c", "user.email=t@example.invalid",
          "-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always"]


def git(cwd, *args, check=True):
    return subprocess.run(["git"] + GIT_ID + list(args), cwd=str(cwd), check=check,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def note_text(name, type_="project", desc="A note", updated=None, body="Body.\n", relations=None,
              top_level_type=False):
    """Frontmatter shaped like the notes Claude Code writes (metadata.type / metadata.modified)."""
    lines = ["---", f"name: {name}", f"description: {desc}"]
    if top_level_type:
        lines.append(f"type: {type_}")
        if updated:
            lines.append(f"updated: {updated}")
    else:
        lines.append("metadata:")
        lines.append(f"  type: {type_}")
        if updated:
            lines.append(f"  modified: {updated}")
        if relations:
            lines.append("  relations:")
            for kind, targets in relations.items():
                lines.append(f"    {kind}: [{', '.join(targets)}]")
    lines += ["---", "", body]
    return "\n".join(lines)


def write(path, text="", mode=None):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    if mode is not None:
        path.chmod(mode)
    return path


def snapshot(root):
    """{relative path: (bytes, mtime_ns)} of every file under root except .git."""
    out = {}
    for p in sorted(Path(root).rglob("*")):
        if ".git" in p.relative_to(root).parts or not p.is_file():
            continue
        out[str(p.relative_to(root))] = (p.read_bytes(), p.stat().st_mtime_ns)
    return out


class MemCase(unittest.TestCase):
    """A temp HOME/CLAUDE_CONFIG_DIR and a git environment that ignores the host's config."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.home = self.tmp / "home"
        self.config = self.home / ".claude"
        self.anth = self.config / "anthology"
        self.data = self.tmp / "data"
        self.home.mkdir()
        self.data.mkdir()
        patcher = mock.patch.dict(os.environ, {
            "HOME": str(self.home), "CLAUDE_CONFIG_DIR": str(self.config),
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        })
        patcher.start()
        self.addCleanup(patcher.stop)
        for var in ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "CLAUDE_PLUGIN_DATA", "GIT_DIR",
                    "GIT_WORK_TREE", "GIT_INDEX_FILE"):
            os.environ.pop(var, None)

    def make_repo(self, name="repo", remote=None):
        root = self.tmp / name
        root.mkdir(parents=True)
        git(root, "init", "-q")
        if remote:
            git(root, "remote", "add", "origin", remote)
        return Path(os.path.realpath(root))

    def commit_all(self, root, msg="c"):
        git(root, "add", "-A")
        git(root, "commit", "-q", "-m", msg)
