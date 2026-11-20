#!/usr/bin/env python3
"""`/anthology:setup` orchestration: preview, apply and remove.

    anthology_setup.py preview --plugin-root R [--cwd C] [--json]
    anthology_setup.py apply   --plugin-root R --expect-sha256 H [--cwd C] [--skip-approve] [--promote F]...
    anthology_setup.py remove  --plugin-root R [--cwd C] [--preview] [--unapprove] [--unapprove-all]

`preview` writes nothing: it reports the environment checks, duplicate plugins, the rtk step,
the settings.json diff (with the sha256 of the text it was computed from) and the memory
steps, including the old path-keyed memory notes of this repo (`memory.legacy_notes`). `apply`
refuses if settings.json changed since the preview (checked again after the rtk download, right
before the write); after approving the repo it copies those legacy notes into personal memory,
except the ones named with `--promote`, which go into the repo's memory if the privacy scan
finds nothing (otherwise they stay personal and are listed in `memory.blocked`). `remove --preview` writes nothing and shows what remove
would change (settings diff, the rtk folder, state.json, approvals). `remove` undoes the
settings edits, deletes the downloaded rtk and `state.json`, and optionally drops repo
approvals; memory notes, `.gitattributes` and the personal-memory folder are never deleted.
Every command prints one JSON object. Exit codes: 0 ok, 1 refused or failed, 2 unsupported
platform. Python stdlib only (3.9+).
"""
from __future__ import annotations

import argparse
import datetime
import difflib
import hashlib
import json
import os
import platform
import shutil
import sys
from pathlib import Path

_SCRIPTS = os.path.dirname(os.path.abspath(__file__))
if _SCRIPTS not in sys.path:
    sys.path.insert(0, _SCRIPTS)

import doctor  # noqa: E402
import memory_sync  # noqa: E402
import settings_patch  # noqa: E402
import state  # noqa: E402

REFUSAL = "Anthology supports macOS and Linux only; it is inactive on Windows."
RTK_TIMEOUT = 60    # seconds per read (overall deadline about 4x): setup is where the first download happens


def _sha256(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _unsupported():
    return platform.system() == "Windows"


def _plugin_version(root):
    try:
        data = json.loads((Path(root) / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
        return str(data["version"])
    except (OSError, ValueError, KeyError, TypeError):
        return "unknown"


def _data_dir(env):
    """The plugin data folder: this process's CLAUDE_PLUGIN_DATA, else what session start recorded."""
    found = env.get("CLAUDE_PLUGIN_DATA") or state.read().get("data_dir")
    return str(found) if found else None


def _lock_path(root):
    return Path(root) / "rtk.lock"


# -- rtk adapters: the only places that name Task 11's `rtk_install` interface ------------------


def _rtk_preview(root, data_dir):
    try:
        import rtk_install
    except ImportError:
        return {"skipped": "rtk wiring not present"}
    try:
        lock = rtk_install.load_lock(_lock_path(root))
    except Exception:
        return {"skipped": "rtk wiring not present (no rtk.lock)"}
    try:
        target = rtk_install.target(platform.system(), platform.machine())
    except Exception as exc:
        return {"error": str(exc) or type(exc).__name__}
    version = lock["version"]
    dest = Path(data_dir) / "rtk" / version / "rtk" if data_dir else None
    return {"version": version, "target": target, "dest": str(dest) if dest else None}


def _rtk_apply(root, data_dir):
    """`(status, warning)`: reconcile (download and verify when missing or stale)."""
    try:
        import rtk_install
    except ImportError:
        return "skipped: rtk wiring not present", None
    if not data_dir:
        return "skipped: plugin data folder unknown", (
            "rtk was not installed: the plugin data folder is not known yet; it is installed at the "
            "next session start")
    try:
        lock = rtk_install.load_lock(_lock_path(root))
    except Exception as exc:
        return "failed", "rtk install failed: %s" % (str(exc) or type(exc).__name__)[:200]
    try:
        result = rtk_install.reconcile(Path(data_dir), lock, None, RTK_TIMEOUT)
    except Exception as exc:
        return "failed", "rtk install failed: %s" % (str(exc) or type(exc).__name__)[:200]
    warning = result.get("warning") if isinstance(result, dict) else None
    if isinstance(warning, str) and warning:
        return "warning", warning
    installed = isinstance(result, dict) and result.get("installed")
    return ("installed" if installed else "up to date"), None


# -- settings -------------------------------------------------------------------------------------


def _settings_plan(root, env):
    """`(path, text, new_text, diff, ops_to_store, ops_version)`; ValueError / FileNotFoundError on
    bad input."""
    path = settings_patch.settings_path(env)
    text = settings_patch.read_text(path)
    ops = settings_patch.setup_ops(root, env=env)
    new_text, diff, stored = settings_patch.reconcile(text, ops, state.read().get("settings_ops") or [])
    return path, text, new_text, diff, stored, settings_patch.ops_version(ops)


def _settings_error(exc):
    if isinstance(exc, FileNotFoundError):
        return ("the anthology-guardrails guard (scripts/guard.py) was not found where Claude Code runs it; "
                "reinstall or repair the anthology-guardrails plugin")
    return str(exc)


# -- memory ---------------------------------------------------------------------------------------


def _personal_dir(env):
    return memory_sync.anthology_home(env) / "personal-memory"


def _repo_info(cwd, env):
    root = memory_sync.repo_root(cwd, env=env)
    if root is None:
        return None
    return root, memory_sync.repo_key(root, env=env)


def _repo_mem(root):
    """`<root>/.claude/memory`, or None if it or `.claude` is a symlink or not a folder."""
    claude = Path(str(root)) / ".claude"
    mem = claude / "memory"
    if memory_sync._unsafe_dir(claude) or memory_sync._unsafe_dir(mem):
        return None
    return mem


def _diff_lines(data):
    lines = data.decode("utf-8", errors="replace").splitlines(True)
    return [l if l.endswith("\n") else l + "\n" for l in lines]


def legacy_notes(root, env, today=None):
    """The old path-keyed auto-memory notes of the repo at `root` (both legacy folders), with
    what moving each into `<root>/.claude/memory/` would do. Writes nothing. The default for
    every note is personal memory; `movable` is False for `type: user` notes and for notes
    with privacy findings (`reason` says why)."""
    date = today or datetime.date.today().isoformat()
    mem = _repo_mem(root)
    out = []
    for old in memory_sync.legacy_dirs(root, memory_sync.config_dir(env), env=env):
        for name, data in memory_sync.legacy_entries(old):
            info = memory_sync.note_info(name, data)
            findings, reason = memory_sync.promotion_block(info, data, home=env.get("HOME"))
            if reason is None and mem is None:
                reason = "repo memory is a symlink or not a folder"
            note = {"file": name, "name": info["name"], "description": info["description"],
                    "type": info["type"], "source": str(old), "default": "personal",
                    "findings": findings, "movable": reason is None}
            if reason:
                note["reason"] = reason
            status = memory_sync.placement_status(mem, name, data) if mem is not None else "new"
            note["status"] = status
            if status == "new":
                diff = difflib.unified_diff([], _diff_lines(data), "/dev/null",
                                            "b/.claude/memory/" + name)
            elif status == "conflict":
                note["would_create"] = memory_sync.side_by_side_name(mem, name, date)
                try:
                    existing = memory_sync._read_regular(mem / name)
                except memory_sync.UnsafeTarget:
                    existing = b""
                diff = difflib.unified_diff(_diff_lines(existing), _diff_lines(data),
                                            "a/.claude/memory/" + name,
                                            "b/.claude/memory/" + note["would_create"])
            else:
                diff = []
            note["diff"] = "".join(diff)
            out.append(note)
    return out


def _migrate_legacy(root, env, personal, promote):
    """Copy this repo's legacy notes: the promoted clean ones into repo memory, everything
    else into personal memory; the ledger records all of them."""
    mem = _repo_mem(root)
    home = memory_sync.anthology_home(env)
    out = {"repo": [], "personal": [], "skipped": [], "blocked": []}
    for old in memory_sync.legacy_dirs(root, memory_sync.config_dir(env), env=env):
        res = memory_sync.migrate(old, mem, personal, ledger=home / "migrated.json",
                                  promote=set(promote), home=env.get("HOME"))
        out["repo"] += res["promoted"]
        out["personal"] += [n for n in res["copied"] + res["conflicts"] if n not in res["promoted"]]
        out["skipped"] += [n for n in res["skipped"] if n not in res["promoted"]]
        out["blocked"] += res["blocked"]
    if out["repo"] and mem is not None:
        memory_sync.regenerate_index(mem, exclude_types=memory_sync.REPO_EXCLUDE)
    return out


def _promote_error(promote, skip_approve, cwd, env):
    """Why `--promote` cannot be honoured, or None (checked before anything is written)."""
    if not promote:
        return None
    if skip_approve:
        return ("--promote moves notes into this repo's memory, which needs the repo approved; "
                "it cannot be combined with --skip-approve. Nothing was written.")
    info = _repo_info(cwd, env)
    if info is None:
        return "--promote needs a git repo (this folder is not one). Nothing was written."
    listed = {n["file"] for n in legacy_notes(info[0], env)}
    unknown = sorted({p for p in promote if p not in listed})
    if unknown:
        return ("--promote takes the file name of one of this repo's legacy notes "
                "(memory.legacy_notes in the preview); not one: %s. Nothing was written."
                % ", ".join(unknown))
    return None


# -- commands -------------------------------------------------------------------------------------


def preview(plugin_root, cwd=None, env=None):
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"supported": False, "message": REFUSAL}
    cwd = cwd or os.getcwd()
    data_dir = _data_dir(env)
    text = settings_patch.read_text(settings_patch.settings_path(env))
    settings = {"path": str(settings_patch.settings_path(env)), "sha256": _sha256(text),
                "bypass_note": settings_patch.BYPASS_NOTE}
    try:
        _path, _text, new_text, diff, _stored, _version = _settings_plan(plugin_root, env)
        settings.update(changed=new_text != text, diff=diff)
    except (ValueError, FileNotFoundError) as exc:
        settings.update(changed=False, diff="", error=_settings_error(exc))
    info = _repo_info(cwd, env)
    if info is None:
        repo = {"skipped": "not a git repo"}
    else:
        repo = {"root": str(info[0]), "key": info[1], "approved": memory_sync.is_approved(info[1], env=env)}
    legacy = legacy_notes(info[0], env) if info is not None else []
    personal = _personal_dir(env)
    installed, installed_state = doctor.installed_plugins_state(env)
    return {
        "supported": True,
        "doctor": {"checks": doctor.check_env(env, lock=_lock_path(plugin_root), plugin_root=plugin_root,
                                         cwd=cwd)},
        "duplicates": doctor.find_duplicates(installed),
        "installed_plugins": installed_state,
        "rtk": _rtk_preview(plugin_root, data_dir),
        "settings": settings,
        "memory": {"repo": repo, "personal_dir": str(personal), "personal_exists": personal.is_dir(),
                   "legacy_notes": legacy},
    }


def apply(plugin_root, expect_sha256, cwd=None, skip_approve=False, env=None, promote=()):
    """Apply the previewed setup. `promote` names legacy notes (bare file names from the
    preview's `memory.legacy_notes`) to move into the repo's memory instead of personal memory;
    a note with privacy findings stays personal and is listed in `memory.blocked`."""
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"ok": False, "unsupported": True, "error": REFUSAL}
    cwd = cwd or os.getcwd()
    promote = list(promote or ())
    refused = _promote_error(promote, skip_approve, cwd, env)
    if refused:
        return {"ok": False, "error": refused}
    warnings = []
    path = settings_patch.settings_path(env)
    if _sha256(settings_patch.read_text(path)) != expect_sha256:
        return {"ok": False, "error": "settings.json changed since the preview; run the preview again"}
    try:
        _settings_plan(plugin_root, env)  # refuse early: nothing is downloaded for a bad plan
    except (ValueError, FileNotFoundError) as exc:
        return {"ok": False, "error": _settings_error(exc)}

    data_dir = _data_dir(env)
    rtk_status, warn = _rtk_apply(plugin_root, data_dir)
    if warn:
        warnings.append(warn)

    # The rtk step can take a while. Re-read settings.json now and recompute from what is on disk,
    # so an edit made meanwhile is either kept (we refuse) or never overwritten.
    stale = {"ok": False, "rtk": {"status": rtk_status}, "warnings": warnings,
             "error": "settings.json changed during setup; it was left untouched. Run the preview again."}
    if _sha256(settings_patch.read_text(path)) != expect_sha256:
        return stale
    try:
        _path, text, new_text, diff, stored, ops_version = _settings_plan(plugin_root, env)
    except (ValueError, FileNotFoundError) as exc:
        return {"ok": False, "error": _settings_error(exc)}
    before = state.read()
    created = bool(before.get("settings_created")) or not os.path.lexists(str(path))
    changed = new_text != text
    # Record what we are about to do before doing it, so remove can always undo it.
    state.write(settings_ops=stored, settings_created=created)
    if changed:
        if _sha256(settings_patch.read_text(path)) != expect_sha256:
            state.write(settings_ops=before.get("settings_ops") or [],
                        settings_created=bool(before.get("settings_created")))
            return stale
        settings_patch.write_text(path, new_text)

    memory = {"approved_key": None, "approve_error": None, "migrated": None, "blocked": []}
    info = None if skip_approve else _repo_info(cwd, env)
    personal = _personal_dir(env)
    fresh = not personal.is_dir()
    personal.mkdir(parents=True, exist_ok=True)
    if fresh:
        os.chmod(str(personal), 0o700)
    if info is not None:
        root, key = info
        memory["approved_key"] = key
        try:
            memory["approved_key"] = memory_sync.approve(root, env=env)
        except memory_sync.UnsafeTarget as exc:  # the approval is already recorded
            memory["approve_error"] = str(exc)
            warnings.append("repo memory is approved, but: %s" % exc)
        migrated = _migrate_legacy(root, env, personal, promote)
        memory["blocked"] = migrated.pop("blocked")
        memory["migrated"] = migrated
    memory["personal_dir"] = str(personal)

    # The op set this run applied: session start compares it with the current release's (I6).
    done = {"setup_done": _plugin_version(plugin_root), "settings_ops_version": ops_version}
    if data_dir:
        done["data_dir"] = data_dir
    state.write(**done)
    return {"ok": True, "settings": {"path": str(path), "changed": changed, "diff": diff},
            "rtk": {"status": rtk_status}, "memory": memory, "warnings": warnings}


def _rtk_target(env):
    """`(real path to delete or None, status, warning or None)` for the downloaded rtk folder.

    Only `realpath(data_dir)/rtk` is ever deleted, and only when data_dir sits under
    `<config>/plugins/data/` and the folder is not a symlink."""
    data_dir = _data_dir(env)
    if not data_dir:
        return None, "no plugin data folder recorded", None
    rtk_dir = Path(data_dir) / "rtk"
    if not os.path.lexists(str(rtk_dir)):
        return None, "nothing to delete", None
    if rtk_dir.is_symlink():
        return None, "left in place", "the rtk folder is a symlink; left in place"
    data_real = os.path.realpath(data_dir)
    data_root = os.path.join(os.path.realpath(str(memory_sync.config_dir(env))), "plugins", "data")
    real = os.path.realpath(str(rtk_dir))
    if not data_real.startswith(data_root + os.sep) or real != os.path.join(data_real, "rtk"):
        return None, "left in place", "the rtk folder is not under the plugin data folder; left in place"
    return real, "delete", None


def _ignore_vanished(func, path, exc_info):
    """shutil.rmtree onerror: a file that disappears mid-delete (a SQLite -wal/-shm) is fine."""
    if not issubclass(exc_info[0], FileNotFoundError):
        raise exc_info[1]


def _rtk_history_target():
    """`(real path to delete or None, status, warning or None)` for rtk's history folder.

    `state.rtk_data_dir()`: the tracking database (every command line the hook saw), the recall
    store and the tee spool. Deleted only when it is a real folder, not a symlink."""
    path = state.rtk_data_dir()
    if not os.path.lexists(str(path)):
        return None, "nothing to delete", None
    if path.is_symlink() or not path.is_dir():
        return None, "left in place", "the rtk history folder %s is not a plain folder; left in place" % path
    return str(path), "delete", None


def _remove_plan(env):
    """`(error, plan)`: what remove would do to settings.json, rtk and state.json."""
    st = state.read()
    ops = st.get("settings_ops") or []
    path = settings_patch.settings_path(env)
    text = settings_patch.read_text(path)
    new_text, diff = text, ""
    if ops:
        try:
            new_text, diff = settings_patch.revert(text, ops)
        except ValueError as exc:
            return "%s; left untouched. Fix the file and run remove again." % exc, None
    return None, {"st": st, "path": path, "text": text, "new_text": new_text, "diff": diff,
                  "rtk": _rtk_target(env), "rtk_history": _rtk_history_target()}


def remove(plugin_root, cwd=None, unapprove=False, unapprove_all=False, env=None, preview=False):
    """Undo setup. With `preview=True` nothing is written: it reports the settings diff, the rtk
    folder and state.json that would be deleted, and what each approval flag would drop."""
    env = dict(os.environ) if env is None else dict(env)
    if _unsupported():
        return {"ok": False, "unsupported": True, "error": REFUSAL}
    cwd = cwd or os.getcwd()
    error, plan = _remove_plan(env)
    if error:
        return {"ok": False, "error": error}
    path, st = plan["path"], plan["st"]
    changed = plan["new_text"] != plan["text"]
    rtk_real, rtk_status, rtk_warn = plan["rtk"]
    history_real, history_status, history_warn = plan["rtk_history"]
    warnings = [w for w in (rtk_warn, history_warn) if w]
    approved = sorted(memory_sync._load_approved(env)["repos"])
    info = _repo_info(cwd, env)
    this_repo = info[1] if info is not None and info[1] in approved else None

    if preview:
        return {"ok": True, "preview": True,
                "settings": {"path": str(path), "changed": changed, "diff": plan["diff"]},
                "rtk": {"dir": rtk_real, "status": rtk_status},
                "rtk_history": {"dir": str(state.rtk_data_dir()), "status": history_status},
                "state": {"path": str(state.state_path()), "exists": state.state_path().exists()},
                "memory": {"notes": "kept (memory notes are never deleted)",
                           "this_repo_approved": this_repo,
                           "approved_total": len(approved),
                           "unapprove": [this_repo] if this_repo else [],
                           "unapprove_all": approved},
                "warnings": warnings}

    if changed:
        if plan["new_text"] == "" and st.get("settings_created"):
            os.unlink(str(path))
        else:
            settings_patch.write_text(path, plan["new_text"])
    # state.json goes first: without its rtk_path, bin/rtk-hook exits before running rtk, so no
    # hook in another session recreates rtk's history while it is being deleted.
    try:
        os.unlink(str(state.state_path()))
    except FileNotFoundError:
        pass
    if rtk_real:
        shutil.rmtree(rtk_real, onerror=_ignore_vanished)
        rtk_status = "deleted"
    if history_real:
        shutil.rmtree(history_real, onerror=_ignore_vanished)
        history_status = "deleted"
    unapproved = []
    if unapprove_all:
        for key in approved:
            if memory_sync.unapprove(key, env=env):
                unapproved.append(key)
    elif unapprove and this_repo:
        if memory_sync.unapprove(this_repo, env=env):
            unapproved.append(this_repo)
    return {"ok": True, "settings": {"path": str(path), "changed": changed, "diff": plan["diff"]},
            "rtk": {"status": rtk_status}, "rtk_history": {"status": history_status},
            "memory": {"unapproved": unapproved,
                                                       "notes": "kept (memory notes are never deleted)"},
            "warnings": warnings}


def main(argv=None):
    parser = argparse.ArgumentParser(description="/anthology:setup: preview, apply and remove")
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("preview", "apply", "remove"):
        p = sub.add_parser(name)
        p.add_argument("--plugin-root", required=True)
        p.add_argument("--cwd")
        p.add_argument("--json", action="store_true", help="accepted for clarity; output is always JSON")
        if name == "apply":
            p.add_argument("--expect-sha256", required=True)
            p.add_argument("--skip-approve", action="store_true")
            p.add_argument("--promote", action="append", default=[], metavar="FILE",
                           help="move this legacy note into the repo's memory (repeatable)")
        if name == "remove":
            p.add_argument("--unapprove", action="store_true")
            p.add_argument("--unapprove-all", action="store_true")
            p.add_argument("--preview", action="store_true",
                           help="write nothing; show what remove would change")
    args = parser.parse_args(argv)
    try:
        if args.command == "preview":
            result = preview(args.plugin_root, cwd=args.cwd)
        elif args.command == "apply":
            result = apply(args.plugin_root, args.expect_sha256, cwd=args.cwd, skip_approve=args.skip_approve,
                           promote=args.promote)
        else:
            result = remove(args.plugin_root, cwd=args.cwd, unapprove=args.unapprove,
                            unapprove_all=args.unapprove_all, preview=args.preview)
    except Exception as exc:
        result = {"ok": False, "error": "%s: %s" % (type(exc).__name__, exc)}
    print(json.dumps(result, indent=2))
    if result.get("supported") is False or result.get("unsupported"):
        return 2
    return 0 if result.get("ok", True) else 1


if __name__ == "__main__":
    sys.exit(main())
