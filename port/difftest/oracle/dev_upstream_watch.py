"""DV3 oracle for the dev.upstream-watch difftest surface (plan DV3).

Run by F4's driver (`python3 -I port/difftest/oracle/_driver.py`), which loads this module by
file path; `json.py` beside this file shadows the standard library's `json`, so this folder is
never on `sys.path`.

Library surface: `run(case: dict) -> dict`. The case layout is documented in
`crates/difftest/src/surfaces/dev_upstream_watch.rs`. In short:

- `{"op": "main", "args": [...], "env": {k: v}, "files": {rel: text}, "refs": {...},
  "http": {...}, "gh": null | {...}, "show": [rel]}` writes the files into a fresh temporary
  folder, makes it the working folder, and calls `upstream_watch.main(args, env=env, net=FakeNet,
  gh=FakeGitHub or None, stdout=...)` with `sys.argv[0]` set to `upstream_watch.py`. The fakes
  serve the case's refs, HTTP bodies and issues, and raise an exception of the named class (or a
  `WatchError`) where the case says so; a key the case does not name raises `KeyError`, as the
  Python tests' fakes do. The result is the exit code, stdout, then `=== calls ===` and each
  GitHub call as one JSON line, then each `show` file (`=== <rel> ===` and its text, or
  `<absent>`), and stderr. An exception `main` does not catch is written
  `anthology-dev upstream-watch: <Type>: <message>`, what the Rust port prints (DV2 ruling
  R10), and marked D-1.
- Library ops print one JSON value: `sanitise` {text, limit}, `ls_remote` {text}, `manifest`
  {hex}, `latest` {pairs, pin}, `without_budget` {text}, `render` {change, extra},
  `classify` {entry}.

F-17 (plan GC-15) is applied to Python's result before the comparison: the commands an issue
names are `scripts/dev.sh vendor ...` and `scripts/dev.sh startup-budget ...`, where the
Python writes `python3 scripts/vendor.py ...` and `python3 scripts/startup_budget.py ...`. The
oracle rewrites them where the Python builds them (every `Change.command`, and the Startup
budget section's command line), so no other text is touched.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import dataclasses
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_UW = None


def _tools():
    """DV2's oracle helpers (`_result`, `_rooted`, `_exc_line`), loaded by path."""
    spec = importlib.util.spec_from_file_location(
        "difftest_oracle_dev_tools_for_dv3", str(Path(__file__).resolve().parent / "dev_tools.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


_T = _tools()


def _uw():
    """scripts/upstream_watch.py with F-17 applied where it builds the commands."""
    global _UW
    if _UW is None:
        spec = importlib.util.spec_from_file_location(
            "upstream_watch", str(ROOT / "scripts" / "upstream_watch.py"))
        uw = importlib.util.module_from_spec(spec)
        sys.modules["upstream_watch"] = uw  # its dataclass resolves annotations through it
        spec.loader.exec_module(uw)

        @dataclasses.dataclass
        class F17Change(uw.Change):
            def __post_init__(self):
                self.command = self.command.replace(
                    "python3 scripts/vendor.py ", "scripts/dev.sh vendor ")

        render = uw.render_startup_budget

        def render_startup_budget(key, rows):
            return render(key, rows).replace(
                "```sh\npython3 scripts/startup_budget.py ", "```sh\nscripts/dev.sh startup-budget ", 1)

        uw.Change = F17Change
        uw.render_startup_budget = render_startup_budget
        _UW = uw
    return _UW


def _raise(spec):
    if "watch" in spec:
        raise _uw().WatchError(spec["watch"])
    raise type(spec["raise"], (Exception,), {})("raised by the case")


class FakeNet:
    def __init__(self, refs, http):
        self.refs = refs or {}
        self.http = http or {}

    def ls_remote(self, repo):
        spec = self.refs[repo]
        if "pairs" in spec:
            return dict((k, v) for k, v in spec["pairs"])
        _raise(spec)

    def http_get(self, url, headers=None):
        spec = self.http[url]
        if "text" in spec:
            return spec["text"].encode("utf-8")
        if "hex" in spec:
            return bytes.fromhex(spec["hex"])
        _raise(spec)


class FakeGitHub:
    def __init__(self, spec):
        self.spec = spec
        self.calls = []

    def list_issues(self):
        self.calls.append(["list"])
        if self.spec.get("raise"):
            _raise({"raise": self.spec["raise"]})
        return list(self.spec.get("issues") or [])

    def create_issue(self, title, body):
        fail = self.spec.get("fail_title")
        if fail and fail in title:
            _raise({"raise": "OSError"})
        self.calls.append(["create", title, body])
        if "create_returns" in self.spec:
            return self.spec["create_returns"]
        return {"number": 900 + len(self.calls)}

    def update_issue(self, number, title, body):
        self.calls.append(["update", number, title, body])
        return {"number": number}


def _main(case):
    uw = _uw()
    tmp = tempfile.mkdtemp(prefix="dv3-oracle-")
    old_cwd = os.getcwd()
    old_argv = sys.argv
    out, err = io.StringIO(), io.StringIO()
    code, mark = 0, None
    gh = FakeGitHub(case["gh"]) if case.get("gh") is not None else None
    try:
        for rel, text in (case.get("files") or {}).items():
            path = Path(tmp) / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode("utf-8"))
        os.chdir(tmp)
        sys.argv = ["upstream_watch.py"]
        net = FakeNet(case.get("refs"), case.get("http"))
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = uw.main(list(case.get("args") or []), env=dict(case.get("env") or {}),
                               net=net, gh=gh, stdout=out)
            except SystemExit as exc:
                code = exc.code if isinstance(exc.code, int) else 1
            except Exception as exc:  # noqa: BLE001 — Python's traceback: exit 1
                print(f"anthology-dev upstream-watch: {_T._exc_line(exc)}", file=err)
                code, mark = 1, "D-1"
        shown = ["=== calls ===\n"]
        for call in (gh.calls if gh is not None else []):
            shown.append(json.dumps(call) + "\n")
        for rel in case.get("show") or []:
            path = Path(tmp) / rel
            shown.append(f"=== {rel} ===\n")
            shown.append(path.read_text(encoding="utf-8") if path.is_file() else "<absent>\n")
        text = out.getvalue() + "".join(shown)
        return _T._result(code or 0, _T._rooted(text, tmp), _T._rooted(err.getvalue(), tmp), mark)
    finally:
        sys.argv = old_argv
        os.chdir(old_cwd)
        shutil.rmtree(tmp, ignore_errors=True)


def _lib(case):
    uw = _uw()
    op = case["op"]
    try:
        if op == "sanitise":
            value = [uw.sanitise_path(case["text"]), uw.sanitise_prose(case["text"], case["limit"])]
        elif op == "ls_remote":
            value = list(uw.parse_ls_remote(case["text"]).items())
        elif op == "manifest":
            value = sorted(uw.parse_manifest(bytes.fromhex(case["hex"])).items())
        elif op == "latest":
            value = uw.latest_stable_tag(dict((k, v) for k, v in case["pairs"]), case["pin"])
        elif op == "without_budget":
            value = uw._without_budget(case["text"])
        elif op == "render":
            c = case["change"]
            change = uw.Change(
                key=c["key"], kind=c["kind"], old=c["old"], new=c["new"],
                new_target=c["new_target"], command=c["command"],
                details=[tuple(d) for d in c["details"]], files=list(c["files"]),
                files_total=c["files_total"], files_heading=c["files_heading"],
                notes=list(c["notes"]))
            value = [uw.render_title(change), uw.render_body(change, [tuple(e) for e in case["extra"]]),
                     change.to_json()]
        elif op == "classify":
            value = uw.classify(case["entry"])
        else:
            raise ValueError(f"unknown op {op!r}")
    except uw.WatchError as exc:
        return _T._result(1, "", f"WatchError: {exc}\n")
    return _T._result(0, json.dumps(value) + "\n", "")


def run(case):
    if case.get("op") == "main":
        return _main(case)
    return _lib(case)
