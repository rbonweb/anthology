"""DV1 oracle for the `dev.vendor` difftest surface: `scripts/vendor.py` is the oracle.

F4's driver loads this module by file path; run it as `python3 -I` (`json.py` beside this file
shadows the standard library's `json`, so this folder must never be on `sys.path`).

Library surface: `run(case: dict) -> dict`, one of these cases (`crates/difftest/src/surfaces/
dev_vendor.rs` builds the same ones):

- `{"op": "validate", "sources": [...]}`: `validate_sources(sources)`, one message per line.
- `{"op": "strip", "text": T}`: `_strip_markdown(T)`.
- `{"op": "rename_skill", "skill": T, "old": O, "new": N}`: `rename_skill` on a folder holding
  `SKILL.md` = T; stdout is the stats repr, `files` the folder afterwards.
- `{"op": "rename_plugin", "tree": TREE, "old": O, "new": N, "version": V}`: `rename_plugin`.
- `{"op": "checksums", "hex": H}`: `parse_checksums(bytes)`, `<path> <sha256>` per line.
- `{"op": "compare" | "diff", "want": TREE, "have": TREE | null}`: `compare_trees(want, have,
  "t")` (one problem per line) or `diff_text(want, have)`. A null `have` is a missing folder.
- `{"op": "cli", "argv": [...]}`: `main(["--root", R] + argv)` where R holds
  `vendor/sources.json` = `{"sources": []}`; exit code, stdout, stderr.

- `{"op": "pipeline", "entry": E, "root": TREE, "drift": [EDIT, ...]}`: a repo whose
  sources.json holds E (a url source, so its snapshot under `vendor/snapshots/` is the upstream
  and nothing touches the network): `build`, `fetch_licenses`, the EDITs (`{"edit": "write",
  "rel", "text"|"hex", "exec"}`, `{"edit": "delete", "rel"}`, `{"edit": "chmod", "rel",
  "exec"}`), `check`, `diff_entry`; each step's output or VendorError under a `<step>:` line,
  and `files` the repo afterwards.
- `{"op": "download", "entry": E, "root": TREE, "pages": {url: hex}}`: `download_snapshot`
  with an in-memory getter (a missing page is `HTTP 404: <url>`); the result, the URLs asked
  for in order, and the repo afterwards.

TREE is `{rel: {"text": T | "hex": H, "exec": bool} | {"link": TARGET}}`, written with mode
0755/0644 into a fresh temporary folder (parents 0755).

Result: `{"exit": 0, "stdout": ..., "stderr": "", "files"?: tree}`, or exit 1 with stderr
`VendorError: <message>`. Any other exception is reported as `<Type>: <message>` (exit 1), which
Rust never produces: the generator stays inside what vendor.py handles without raising.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_VENDOR = None


def _vendor():
    global _VENDOR
    if _VENDOR is None:
        spec = importlib.util.spec_from_file_location(
            "difftest_vendor_py", str(ROOT / "scripts" / "vendor.py"))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        _VENDOR = mod
    return _VENDOR


def _write_tree(base, tree):
    base.mkdir(parents=True, exist_ok=True)
    os.chmod(base, 0o755)
    for rel in sorted(tree):
        spec = tree[rel]
        path = base / rel
        for parent in reversed(path.relative_to(base).parents):
            d = base / parent
            if not d.exists():
                d.mkdir()
                os.chmod(d, 0o755)
        if "link" in spec:
            os.symlink(spec["link"], path)
        else:
            path.write_bytes(_data(spec))
            os.chmod(path, 0o755 if spec.get("exec") else 0o644)


def _data(spec):
    return bytes.fromhex(spec["hex"]) if "hex" in spec else spec["text"].encode("utf-8")


def _drift(root, edits):
    """Edits after the build: write (parents 0755), delete (if present), chmod (if a file)."""
    for e in edits:
        path = root / e["rel"]
        if e["edit"] == "write":
            _write_tree(root, {e["rel"]: e})
        elif e["edit"] == "delete":
            if os.path.lexists(path) and not path.is_dir():
                path.unlink()
        elif path.is_file() and not path.is_symlink():
            os.chmod(path, 0o755 if e.get("exec") else 0o644)


def _step(out, title, fn):
    v = _vendor()
    out.append(f"{title}:\n")
    try:
        out.append(fn())
    except v.VendorError as exc:
        out.append(f"VendorError: {exc}\n")


def _files(base):
    """The testkit `Tree` JSON form of `base`."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(base):
        for name in dirnames + filenames:
            p = Path(dirpath) / name
            rel = p.relative_to(base).as_posix()
            st = os.lstat(p)
            if os.path.islink(p):
                out[rel] = {"type": "symlink", "target": os.readlink(p)}
            elif p.is_dir():
                out[rel] = {"type": "dir", "mode": "%04o" % (st.st_mode & 0o7777)}
            else:
                data = p.read_bytes()
                entry = {"type": "file", "mode": "%04o" % (st.st_mode & 0o7777)}
                try:
                    entry["text"] = data.decode("utf-8")
                except UnicodeDecodeError:
                    entry["hex"] = data.hex()
                out[rel] = entry
    return dict(sorted(out.items()))


def _ok(stdout, files=None):
    out = {"exit": 0, "stdout": stdout, "stderr": ""}
    if files is not None:
        out["files"] = files
    return out


def _lines(items):
    return "".join(f"{m}\n" for m in items)


def _op(case, tmp):
    v = _vendor()
    op = case["op"]
    if op == "validate":
        return _ok(_lines(v.validate_sources(case["sources"])))
    if op == "strip":
        return _ok(v._strip_markdown(case["text"]))
    if op == "rename_skill":
        tree = tmp / "tree"
        _write_tree(tree, {"SKILL.md": {"text": case["skill"]}})
        stats = v.rename_skill(tree, case["old"], case["new"])
        return _ok(repr(stats) + "\n", _files(tree))
    if op == "rename_plugin":
        tree = tmp / "tree"
        _write_tree(tree, case["tree"])
        stats = v.rename_plugin(tree, case["old"], case["new"], case["version"])
        return _ok(repr(stats) + "\n", _files(tree))
    if op == "checksums":
        listed = v.parse_checksums(bytes.fromhex(case["hex"]))
        return _ok("".join(f"{rel} {digest}\n" for rel, digest in sorted(listed.items())))
    if op in ("compare", "diff"):
        want, have = tmp / "want", tmp / "have"
        _write_tree(want, case["want"])
        if case["have"] is not None:
            _write_tree(have, case["have"])
        if op == "compare":
            return _ok(_lines(v.compare_trees(want, have, "t")))
        return _ok(v.diff_text(want, have))
    if op == "cli":
        root = tmp / "root"
        (root / "vendor").mkdir(parents=True)
        (root / "vendor" / "sources.json").write_text('{"sources": []}\n', encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        code = 0
        # argparse names the program after sys.argv[0] and wraps help at $COLUMNS.
        saved = sys.argv[0], os.environ.get("COLUMNS")
        sys.argv[0], os.environ["COLUMNS"] = "vendor.py", "80"
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                try:
                    code = v.main(["--root", str(root)] + list(case["argv"]))
                except SystemExit as exc:
                    code = exc.code if isinstance(exc.code, int) else 1
        finally:
            sys.argv[0] = saved[0]
            if saved[1] is None:
                os.environ.pop("COLUMNS", None)
            else:
                os.environ["COLUMNS"] = saved[1]
        return {"exit": code, "stdout": out.getvalue(), "stderr": err.getvalue()}
    if op in ("pipeline", "download"):
        root = tmp / "root"
        _write_tree(root, case["root"])
        (root / "vendor").mkdir(exist_ok=True)
        (root / "vendor" / "sources.json").write_text(
            json.dumps({"sources": [case["entry"]]}, indent=2) + "\n", encoding="utf-8")
        name = case["entry"].get("name")
        out = []
        if op == "download":
            calls = []

            def get(url):
                calls.append(url)
                if url not in case["pages"]:
                    raise v.VendorError(f"HTTP 404: {url}")
                return bytes.fromhex(case["pages"][url])

            _step(out, "download", lambda: str(
                v.download_snapshot(root, case["entry"], get=get).relative_to(root)) + "\n")
            out.append("calls:\n" + _lines(calls))
            return _ok("".join(out), _files(root))
        _step(out, "build", lambda: "built ({})\n".format(", ".join(
            f"{t}: {st}" for t, st in v.build(root, name)["transforms"]) or "no transforms"))
        _step(out, "licenses", lambda: _lines(
            p.relative_to(root).as_posix() for p in v.fetch_licenses(root)))
        _drift(root, case["drift"])
        _step(out, "check", lambda: _lines(v.check(root)))
        _step(out, "diff", lambda: v.diff_entry(root, name))
        return _ok("".join(out), _files(root))
    raise ValueError(f"unknown op {op!r}")


def run(case):
    v = _vendor()
    cwd = os.getcwd()
    with tempfile.TemporaryDirectory(prefix="dev-vendor-") as tmp:
        tmp = Path(tmp).resolve()
        os.chdir(tmp)
        try:
            return _op(case, tmp)
        except v.VendorError as exc:
            return {"exit": 1, "stdout": "", "stderr": f"VendorError: {exc}"}
        except Exception as exc:  # noqa: BLE001 - reported, never matched by Rust
            return {"exit": 1, "stdout": "", "stderr": f"{type(exc).__name__}: {exc}"}
        finally:
            os.chdir(cwd)
