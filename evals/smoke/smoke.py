#!/usr/bin/env python3
"""Anthology smoke cases U1-U9: the multi-session and no-model checks `claude plugin eval` cannot do.

Usage: smoke.py [--claude BIN] [--budget-usd X] [--total-budget-usd X] [--out PATH] [--list] [NAME ...]

Runs the named smokes (default: all). Each returns PASS, FAIL or SKIP with a one-line reason;
one line per smoke is printed, a JSON summary goes to --out, and the exit status is 1 if any
smoke FAILED, else 0. `scripts/run-evals.sh` runs this after the evals; CI runs `cli-syntax`.
Spend: every `claude -p` gets --max-budget-usd (--budget-usd); the cost each one reports is summed,
and once --total-budget-usd (default 10) is spent no more runs start: the remaining smokes FAIL with
"smoke budget exhausted". A binary given with --claude or $CLAUDE_BIN that is missing is a FAIL.

Isolation: every `claude` run gets a fresh temp dir with its own HOME and CLAUDE_CONFIG_DIR and an
environment built from scratch (see build_env): the real ~/.claude is never read or written, and
only ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN and ANTHROPIC_BASE_URL are passed through. The two
smokes that run with --permission-mode bypassPermissions (auto-memory-dir, writer-memory-commit)
do so with their working directory, HOME and config in a scratch temp dir and the guardrails
plugin loaded. That is NOT a sandbox: the permission mode lets Bash write anywhere the user can, so
run them only in CI or another throwaway environment. Every subprocess has a timeout. Auth values
are never printed.

Standard library only; runs on Python 3.9+.
"""
from __future__ import annotations

import argparse
import contextlib
import inspect
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable, Dict, Iterator, List, Optional, Sequence, Tuple

ROOT = Path(__file__).resolve().parents[2]
PLUGINS = ("anthology", "anthology-guardrails", "anthology-superpowers")
AUTH_VARS = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL")
CLAUDE_TIMEOUT = 300
PASS, FAIL, SKIP = "PASS", "FAIL", "SKIP"
NO_AUTH = "no API credentials (set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN)"
# U1: agents read their knowledge. `//` marks an absolute path in permission rules (one `/` is project-relative).
READ_ALLOW = {"permissions": {"allow": [f"Read(/{ROOT}/plugins/**)"]}}
EXPECTED_AGENTS = 11
MARKER = "smoke-unpushed-marker"
PROMPT_WORKTREE = ("Use the Agent tool to run the anthology:implementer agent with the prompt "
                   "'Run git log --oneline -3, change nothing, and report the output verbatim.' "
                   "Then quote its report.")

Result = Tuple[str, str]  # (status, one-line reason)


class BudgetExhausted(Exception):
    pass


class Proc:
    def __init__(self, code: int, out: str, err: str):
        self.code, self.out, self.err = code, out, err

    @property
    def text(self) -> str:
        return self.out + "\n" + self.err


# ---- environment, processes, scratch dirs -----------------------------------------------------

def build_env(tmp: Path, extra: Optional[Dict[str, str]] = None) -> Dict[str, str]:
    """The environment for a `claude` run: built from scratch, never a copy of os.environ."""
    home, cfg = tmp / "home", tmp / "cfg"
    home.mkdir(parents=True, exist_ok=True)
    cfg.mkdir(parents=True, exist_ok=True)
    env = {"HOME": str(home), "CLAUDE_CONFIG_DIR": str(cfg), "PATH": os.environ.get("PATH", ""),
           "TMPDIR": os.environ.get("TMPDIR") or tempfile.gettempdir(), "LANG": "C.UTF-8",
           "TERM": "dumb", "DISABLE_AUTOUPDATER": "1",
           "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB": "1",  # tools and hooks never see the API key
           "NPM_CONFIG_OFFLINE": "true"}  # a broken guard cannot fetch a package
    env.update({k: os.environ[k] for k in AUTH_VARS if os.environ.get(k)})
    env.update(extra or {})
    return env


def has_auth() -> bool:
    return bool(os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_AUTH_TOKEN"))


def redact(text: str) -> str:
    for k in AUTH_VARS:
        v = os.environ.get(k)
        if v and len(v) > 5:
            text = text.replace(v, "***")
    return text


def snippet(text: str, n: int = 160) -> str:
    return " ".join(redact(text).split())[:n]


def run(argv: Sequence[str], cwd: Path, env: Dict[str, str], timeout: int = CLAUDE_TIMEOUT,
        stdin: Optional[str] = None) -> Proc:
    feed = {"input": stdin} if stdin is not None else {"stdin": subprocess.DEVNULL}
    try:
        p = subprocess.run(list(argv), cwd=str(cwd), env=env, timeout=timeout,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, **feed)
    except subprocess.TimeoutExpired:
        return Proc(-1, "", f"timed out after {timeout}s")
    except OSError as exc:
        return Proc(-1, "", f"could not run {argv[0]}: {exc.__class__.__name__}")
    return Proc(p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace"))


class Ctx:
    def __init__(self, claude: str, budget: str, total: float = 10.0):
        self.claude, self.budget, self.total, self.spent = claude, budget, total, 0.0

    def claude_run(self, tmp: Path, args: Sequence[str], cwd: Optional[Path] = None,
                   extra_env: Optional[Dict[str, str]] = None, timeout: int = CLAUDE_TIMEOUT) -> Proc:
        return run([self.claude, *args], cwd or tmp, build_env(tmp, extra_env), timeout)

    def prompt_run(self, tmp: Path, prompt: str, flags: Sequence[str], cwd: Optional[Path] = None,
                   extra_env: Optional[Dict[str, str]] = None, variadic: Sequence[str] = ()) -> Proc:
        """`claude -p PROMPT FLAGS --max-budget-usd B [--allowedTools ...]`; the variadic flag goes last.

        Unless FLAGS pick an output format, the run asks for `json` and `Proc.out` becomes the reply text,
        so the reported cost can be added to the running total (an unreported cost counts as the budget).
        """
        if self.spent >= self.total:
            raise BudgetExhausted()
        own_format = "--output-format" in flags
        args = ["-p", prompt, "--max-budget-usd", self.budget, *flags]
        args += [] if own_format else ["--output-format", "json"]
        if variadic:
            args += ["--allowedTools", *variadic]
        p = self.claude_run(tmp, args, cwd, extra_env)
        cost, text = result_cost_and_text(p.out)
        self.spent += cost if cost is not None else float(self.budget)
        if not own_format and text is not None:
            p.out = text
        return p


@contextlib.contextmanager
def scratch() -> Iterator[Path]:
    with tempfile.TemporaryDirectory(prefix="anthology-smoke-") as d:
        yield Path(d).resolve()


def plugin_flags() -> List[str]:
    return [a for p in PLUGINS for a in ("--plugin-dir", str(ROOT / "plugins" / p))]


def git_env(tmp: Path) -> Dict[str, str]:
    env = build_env(tmp)
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_AUTHOR_NAME": "Smoke", "GIT_COMMITTER_NAME": "Smoke",
                "GIT_AUTHOR_EMAIL": "smoke@example.invalid", "GIT_COMMITTER_EMAIL": "smoke@example.invalid"})
    return env


def git(tmp: Path, cwd: Path, *args: str) -> str:
    p = run(["git", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main", *args], cwd,
            git_env(tmp), timeout=60)
    if p.code != 0:
        raise RuntimeError(f"git {args[0]} failed: {snippet(p.err)}")
    return p.out.strip()


def set_identity(tmp: Path, repo: Path) -> None:
    """Claude's own `git commit` in a scratch repo needs an identity (the HOME has no .gitconfig)."""
    git(tmp, repo, "config", "user.name", "smoke")
    git(tmp, repo, "config", "user.email", "smoke@example.invalid")


def make_repo(tmp: Path, name: str = "repo") -> Path:
    repo = tmp / name
    repo.mkdir()
    git(tmp, repo, "init", "-q", "-b", "main")
    set_identity(tmp, repo)
    (repo / "README.md").write_text("smoke\n", encoding="utf-8")
    git(tmp, repo, "add", "-A")
    git(tmp, repo, "commit", "-q", "-m", "init")
    return repo


def write_plugin(root: Path, name: str, hooks: Optional[dict] = None, files: Optional[Dict[str, str]] = None) -> Path:
    (root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
    (root / ".claude-plugin" / "plugin.json").write_text(
        json.dumps({"name": name, "version": "0.0.1", "description": "anthology smoke probe"}), encoding="utf-8")
    if hooks is not None:
        (root / "hooks").mkdir(exist_ok=True)
        (root / "hooks" / "hooks.json").write_text(json.dumps({"hooks": hooks}), encoding="utf-8")
    for rel, text in (files or {}).items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(text, encoding="utf-8")
    return root


# ---- pure helpers (unit-tested in tests/test_smoke.py) -----------------------------------------

def has_flag(help_text: str, flag: str) -> bool:
    return re.search(r"(?<![\w-])" + re.escape(flag) + r"(?![\w-])", help_text) is not None


def first_problem(text: str) -> str:
    """The first `❯ problem` line of `claude plugin validate` output (else its start)."""
    line = next((l for l in text.splitlines() if "\u276f" in l), text)
    return snippet(line.replace("\u276f", "").strip(), 110)


def cap_padding(n: int) -> str:
    tail = "END-%d" % n
    return (("lorem ipsum " * (n // 12 + 1))[: n - len(tail) - 1]) + " " + tail


def cap_hook_script() -> str:
    """The SessionStart hook for context-cap: prints exactly CAP_N characters ending in END-<N>."""
    return ("import json, os\n" + inspect.getsource(cap_padding) +
            "n = int(os.environ['CAP_N'])\n"
            "print(json.dumps({'hookSpecificOutput': {'hookEventName': 'SessionStart', "
            "'additionalContext': cap_padding(n)}}))\n")


def sh_session_start_commands(hooks_json: Path) -> List[List[str]]:
    """argv templates of the SessionStart hook commands that run a `.sh` file."""
    try:
        data = json.loads(hooks_json.read_text(encoding="utf-8"))
        groups = data.get("hooks", {}).get("SessionStart") or []
    except (OSError, ValueError, AttributeError):
        return []
    found: List[List[str]] = []
    for group in groups:
        for hook in (group.get("hooks") or []) if isinstance(group, dict) else []:
            cmd = hook.get("command") if isinstance(hook, dict) else None
            if not isinstance(cmd, str):
                continue
            args = hook.get("args")
            argv = [cmd] + [str(a) for a in args] if isinstance(args, list) else shlex.split(cmd)
            if any(a.endswith(".sh") for a in argv):
                found.append(argv)
    return found


def has_session_start(hooks_json: Path) -> bool:
    try:
        return bool(json.loads(hooks_json.read_text(encoding="utf-8")).get("hooks", {}).get("SessionStart"))
    except (OSError, ValueError, AttributeError):
        return False


def stream_events(text: str) -> List[dict]:
    events = []
    for line in text.splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if isinstance(ev, dict):
            events.append(ev)
    return events


def tool_result_texts(events: Sequence[dict]) -> List[str]:
    """The text of every tool_result in a stream-json run (what the tool returned, not what was asked)."""
    out = []
    for ev in events:
        content = (ev.get("message") or {}).get("content") if ev.get("type") == "user" else None
        for block in content if isinstance(content, list) else []:
            if isinstance(block, dict) and block.get("type") == "tool_result":
                c = block.get("content")
                out.append(c if isinstance(c, str) else " ".join(
                    b.get("text", "") for b in c if isinstance(b, dict)) if isinstance(c, list) else "")
    return out


def result_cost_and_text(out: str) -> Tuple[Optional[float], Optional[str]]:
    """(total_cost_usd, result text) from the `result` event of a `claude -p` json or stream-json run."""
    try:
        data = json.loads(out.strip())
        events = [e for e in (data if isinstance(data, list) else [data]) if isinstance(e, dict)]
    except ValueError:
        events = stream_events(out)
    result = next((e for e in reversed(events) if e.get("type") == "result"), None)
    if result is None:
        return None, None
    cost, text = result.get("total_cost_usd"), result.get("result")
    return (float(cost) if isinstance(cost, (int, float)) else None), (text if isinstance(text, str) else None)


def tool_uses(events: Sequence[dict]) -> List[str]:
    """Names of the tools the model asked to use in a stream-json run."""
    out: List[str] = []
    for ev in events:
        content = (ev.get("message") or {}).get("content") if ev.get("type") == "assistant" else None
        for block in content if isinstance(content, list) else []:
            if isinstance(block, dict) and block.get("type") == "tool_use" and isinstance(block.get("name"), str):
                out.append(block["name"])
    return out


def hook_failure_text(events: Sequence[dict]) -> str:
    """Output, stderr and exit-code fields of the hook events only (not ids or other metadata)."""
    parts = []
    for ev in events:
        if ev.get("type") == "system" and str(ev.get("subtype", "")).startswith("hook"):
            parts += [ev[k] for k in ("output", "stdout", "stderr", "message", "error") if isinstance(ev.get(k), str)]
            parts += [f"exit code {ev[k]}" for k in ("exit_code", "exitCode") if isinstance(ev.get(k), int)]
    return "\n".join(parts)


NOT_FOUND_RE = re.compile(r"executable not found|command not found|\bexit(ed)?( code| status)? 127\b", re.I)
BLOCKED_RE = re.compile(r"blocking because onFailure is .?block", re.I)


def subagent_files(cfg: Path) -> List[Path]:
    return sorted((cfg / "projects").glob("**/subagents/*.jsonl")) if (cfg / "projects").is_dir() else []


def transcript_cwds(text: str) -> List[str]:
    """The `cwd` of every entry of one transcript JSONL."""
    out = []
    for line in text.splitlines():
        try:
            cwd = json.loads(line).get("cwd")
        except (ValueError, AttributeError):
            continue
        if isinstance(cwd, str):
            out.append(cwd)
    return out


def parse_agent(text: str) -> Dict[str, object]:
    """One agent file -> {model, worktree, preloads, startup}, by plain line parsing (no PyYAML).

    model / isolation come from the frontmatter, preloads from its `skills:` list (the `plugin:` prefix
    dropped), startup from the `## Startup` section: refs `x` or `ds/x` of its `knowledge/.../SKILL.md` paths.
    """
    head, _, body = text[4:].partition("\n---") if text.startswith("---\n") else ("", "", text)
    meta: Dict[str, str] = {}
    preloads: List[str] = []
    listing = False
    for line in head.splitlines():
        m = re.match(r"^(\w+):\s*(.*)$", line)
        if m:
            listing = m.group(1) == "skills"
            meta[m.group(1)] = m.group(2).strip().strip("\"'")
        elif listing and line.strip().startswith("- "):
            preloads.append(line.strip()[2:].strip().strip("\"'").split(":")[-1])
    section = re.search(r"^## Startup\n(.*?)(?=^## |\Z)", body, re.M | re.S)
    startup = []
    for m in re.finditer(r"^- `\$\{CLAUDE_PLUGIN_ROOT\}/knowledge/(.+?)/SKILL\.md`", section.group(1) if section else "", re.M):
        ref = m.group(1)
        startup.append("ds/" + ref[len("design-skills/skills/"):] if ref.startswith("design-skills/skills/") else ref)
    return {"model": meta.get("model", "?"), "worktree": meta.get("isolation") == "worktree",
            "preloads": preloads, "startup": startup}


def load_agents(agents_dir: Path) -> Dict[str, Dict[str, object]]:
    """agent name -> parse_agent() of plugins/anthology/agents/<name>.md."""
    return {f.stem: parse_agent(f.read_text(encoding="utf-8")) for f in sorted(agents_dir.glob("*.md"))}


def transcript_models(cfg: Path) -> Tuple[List[str], List[Path]]:
    """Every message.model in the subagent transcripts under <cfg>/projects, and the files read."""
    files = subagent_files(cfg)
    models = []
    for f in files:
        for line in f.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                m = (json.loads(line).get("message") or {}).get("model")
            except (ValueError, AttributeError):
                continue
            if isinstance(m, str):
                models.append(m)
    return models, files


def startup_regex(ref: str) -> str:
    """Path regex of a Startup file; a ref is `x` or `ds/x` (folder names hold only letters, digits, hyphens)."""
    if ref.startswith("ds/"):
        return r"knowledge/design-skills/skills/%s/SKILL\.md" % ref[3:]
    return r"knowledge/%s/SKILL\.md" % ref


def skill_heading(skill: str) -> str:
    """The first `# ` heading of a superpowers skill, without the marker ('' when there is none)."""
    text = (ROOT / "plugins" / "anthology-superpowers" / "skills" / skill / "SKILL.md").read_text(encoding="utf-8")
    body = text.split("\n---\n", 1)[1] if text.startswith("---\n") else text
    m = re.search(r"^# (.+)$", body, re.M)
    return m.group(1).strip() if m else ""


def transcript_calls(text: str) -> List[Tuple[str, str]]:
    """(tool name, input as JSON) of every tool_use in one transcript JSONL, in order."""
    calls = []
    for line in text.splitlines():
        try:
            content = (json.loads(line).get("message") or {}).get("content")
        except (ValueError, AttributeError):
            continue
        for block in content if isinstance(content, list) else []:
            if isinstance(block, dict) and block.get("type") == "tool_use" and isinstance(block.get("name"), str):
                calls.append((block["name"], json.dumps(block.get("input", {}))))
    return calls


def startup_problems(calls: Sequence[Tuple[str, str]], refs: Sequence[str]) -> List[str]:
    """What is wrong with an agent's Startup reading: every ref read, in order, before its `*.md` Glob."""
    glob = next((i for i, (n, t) in enumerate(calls) if n == "Glob" and "*.md" in t), None)
    if glob is None:
        return ["no Glob of *.md"]
    problems, last = [], -1
    for ref in refs:
        pattern = startup_regex(ref)
        at = next((i for i, (n, t) in enumerate(calls) if n == "Read" and re.search(pattern, t)), None)
        if at is None:
            problems.append(f"{ref} never read")
        elif at > glob:
            problems.append(f"{ref} read after the Glob")
        elif at < last:
            problems.append(f"{ref} read out of order")
        else:
            last = at
    return problems


def find_text(directory: Path, needle: str) -> bool:
    for f in directory.rglob("*") if directory.is_dir() else []:
        if f.is_file() and needle in f.read_text(encoding="utf-8", errors="replace").lower():
            return True
    return False


# ---- the smokes --------------------------------------------------------------------------------

def smoke_cli_syntax(ctx: Ctx) -> Result:
    """U4: the plugin subcommands and flags the plugins and evals rely on exist (no model)."""
    flags = {("marketplace", "add"): ["--scope"], ("install",): ["-s"], ("uninstall",): ["--keep-data"],
             ("validate",): ["--strict"], ("tag",): ["--dry-run", "--push"],
             ("eval",): ["--max-cost-usd", "--trust-plugin", "--ablation", "--no-publish", "--json",
                         "--allow-tools", "--tag", "--threshold", "--output-dir", "--runs"],
             ("list",): [], ("marketplace", "list"): [], ("marketplace", "remove"): [], ("update",): []}
    problems: List[str] = []
    with scratch() as tmp:
        for sub, wanted in flags.items():
            name = "plugin " + " ".join(sub)
            p = ctx.claude_run(tmp, ["plugin", *sub, "--help"], cwd=ROOT, timeout=60)
            if p.code != 0:
                problems.append(f"{name} --help exited {p.code}")
                continue
            problems += [f"{name} lacks {f}" for f in wanted if not has_flag(p.out + p.err, f)]
        for plugin in PLUGINS:
            p = ctx.claude_run(tmp, ["plugin", "validate", "--strict", str(ROOT / "plugins" / plugin)],
                               cwd=ROOT, timeout=120)
            if p.code != 0:
                problems.append(f"validate --strict {plugin} exited {p.code}: {first_problem(p.text)}")
        version = json.loads((ROOT / "plugins/anthology/.claude-plugin/plugin.json").read_text("utf-8"))["version"]
        p = ctx.claude_run(tmp, ["plugin", "tag", "--dry-run", str(ROOT / "plugins/anthology")], cwd=ROOT, timeout=60)
        if f"anthology--v{version}" not in p.text:
            problems.append(f"tag --dry-run does not name anthology--v{version}: {snippet(p.text, 100)}")
    return (FAIL, "; ".join(problems)) if problems else (PASS, "all plugin subcommands, flags, validate and tag behave")


def smoke_eval_ceiling(ctx: Ctx) -> Result:
    """U9: --max-cost-usd stops the run after the first of two runs, as a partial result (exit 2)."""
    with scratch() as tmp:
        d = tmp / "ws"
        write_plugin(d / "plugins" / "smoke-probe", "smoke-probe")
        (d / ".claude-plugin").mkdir()
        (d / ".claude-plugin" / "marketplace.json").write_text(json.dumps({
            "name": "smoke", "owner": {"name": "smoke"},
            "plugins": [{"name": "smoke-probe", "source": "./plugins/smoke-probe"}]}), encoding="utf-8")
        (d / "evals" / "ceiling").mkdir(parents=True)
        (d / "evals" / "ceiling" / "case.yaml").write_text(
            'schema_version: "1.0"\nname: ceiling\ndescription: cost ceiling probe\n'
            'plugins: ["../../plugins/smoke-probe"]\n'
            'execution:\n  prompt: "Reply with the single word OK."\n  model: haiku\n  max_turns: 1\n'
            "runs: 2\ngraders:\n  - name: said-ok\n    type: regex\n    target: last_message\n    pattern: 'OK'\n",
            encoding="utf-8")
        p = ctx.claude_run(tmp, ["plugin", "eval", ".", "--trust-plugin", "--ablation", "none", "--no-publish",
                                 "--max-cost-usd", "0.000001", "--runs", "2", "--json", "out.json"],
                           cwd=d, timeout=600)
        try:
            res = json.loads((d / "out.json").read_text(encoding="utf-8"))
            runs = len(res["cases"][0]["arms"]["with"])
        except (OSError, ValueError, KeyError, IndexError, TypeError):
            return FAIL, f"no usable result JSON (exit {p.code}): {snippet(p.text)}"
        ctx.spent += float(res["costUsd"]) if isinstance(res.get("costUsd"), (int, float)) else 0.0
        bad = []
        if p.code != 2:
            bad.append(f"exit {p.code}, want 2")
        if res.get("partial") is not True or res.get("partialReason") != "cost_ceiling":
            bad.append(f"partial={res.get('partial')!r} reason={res.get('partialReason')!r}")
        if runs != 1:
            bad.append(f"{runs} runs recorded, want 1")
        return (FAIL, "; ".join(bad)) if bad else (PASS, "stopped at the cost ceiling after 1 of 2 runs (partial, exit 2)")


def smoke_context_cap(ctx: Ctx) -> Result:
    """U8: a SessionStart hook's additionalContext is cut at 10,000 characters (the budget is 9,000)."""
    prompt = "Without using tools, quote any token that starts with END-, else reply NONE."
    seen = {}
    with scratch() as tmp:
        probe = write_plugin(tmp / "cap-probe", "cap-probe", {"SessionStart": [{"hooks": [
            {"type": "command", "command": "python3", "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/ss.py"]}]}]},
            {"hooks/ss.py": cap_hook_script()})
        for n in (10000, 10001):
            p = ctx.prompt_run(tmp, prompt, ["--max-turns", "1", "--model", "haiku", "--plugin-dir", str(probe)],
                               extra_env={"CAP_N": str(n)})
            if p.code != 0:
                return FAIL, f"claude exited {p.code} at N={n}: {snippet(p.text)}"
            seen[n] = p.out
    if "END-10000" in seen[10000] and "END-10001" not in seen[10001]:
        return PASS, "10,000 characters arrive intact and 10,001 are cut"
    return FAIL, ("the SessionStart cap moved; re-check the 9,000-char budget "
                  f"(N=10000 quoted its end: {'END-10000' in seen[10000]}; N=10001 quoted its end: {'END-10001' in seen[10001]})")


def smoke_missing_interpreter(ctx: Ctx) -> Result:
    """U8: a hook whose interpreter is missing is non-blocking by default, and blocks with onFailure."""
    prompt = "Run this Bash command once: echo RAN-OK"
    flags = ["--model", "haiku", "--output-format", "stream-json", "--verbose", "--include-hook-events", "--max-turns", "3"]
    seen = {}
    with scratch() as tmp:
        for label, extra in (("default", {}), ("block", {"onFailure": "block"})):
            hook = {"type": "command", "command": "python9", "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/none.py"], **extra}
            probe = write_plugin(tmp / f"miss-{label}", f"miss-{label}",
                                 {"PreToolUse": [{"matcher": "Bash", "hooks": [hook]}]})
            p = ctx.prompt_run(tmp, prompt, [*flags, "--plugin-dir", str(probe)], variadic=["Bash(echo *)"])
            events = stream_events(p.out)
            results = tool_result_texts(events)
            hooks = hook_failure_text(events)
            seen[label] = {"ran": any("RAN-OK" in t for t in results), "bash": "Bash" in tool_uses(events),
                           "not_found": bool(NOT_FOUND_RE.search(hooks)),
                           "blocked": bool(BLOCKED_RE.search(hooks + "\n" + "\n".join(results))), "text": p.text}
    d, b = seen["default"], seen["block"]
    if not (d["ran"] and d["not_found"]):
        return FAIL, f"default: command ran={d['ran']}, hook reports a missing executable={d['not_found']}: {snippet(d['text'])}"
    if not (b["bash"] and b["blocked"] and not b["ran"]):
        return FAIL, (f"onFailure=block: Bash called={b['bash']}, command ran={b['ran']}, "
                      f"'blocking because onFailure is block' seen={b['blocked']}")
    return PASS, "a missing interpreter reports 'not found' and lets the command run; onFailure=block stops it"


def smoke_python_missing(ctx: Ctx) -> Result:
    """U8 (no model): the sh SessionStart wrapper prints a systemMessage naming python3 when python3 is missing.
    The guardrails sentinel (PLAN U8) must also print nothing when python3 is present."""
    cmds = []
    for plugin in ("anthology", "anthology-guardrails"):
        root = ROOT / "plugins" / plugin
        cmds += [(root, argv) for argv in sh_session_start_commands(root / "hooks" / "hooks.json")]
    if not cmds:
        return SKIP, "no sh SessionStart wrapper yet (stream F)"
    problems = []
    with scratch() as tmp:
        tools = ("sh", "cat", "printf", "env", "dirname", "basename", "head", "tr", "sed", "mkdir", "uname")
        bins = {}
        for label, extra in (("missing", ()), ("present", ("python3",))):
            bins[label] = tmp / f"bin-{label}"
            bins[label].mkdir()
            for tool in tools + extra:
                found = shutil.which(tool)  # resolve before PATH is narrowed
                if found:
                    os.symlink(found, bins[label] / tool)

        def wrapper(root: Path, argv: List[str], label: str) -> Proc:
            argv = [re.sub(r"\$\{?CLAUDE_PLUGIN_ROOT\}?", str(root), a) for a in argv]
            env = {"PATH": str(bins[label]), "HOME": str(tmp), "LANG": "C.UTF-8", "CLAUDE_PLUGIN_ROOT": str(root),
                   "CLAUDE_PROJECT_DIR": str(tmp)}
            return run(argv, tmp, env, timeout=60, stdin="{}")

        for root, argv in cmds:
            p = wrapper(root, argv, "missing")
            try:
                msg = json.loads(p.out.strip())["systemMessage"]
                ok = isinstance(msg, str) and msg.strip() != "" and "python3" in msg
            except (ValueError, KeyError, TypeError):
                ok = False
            if not ok:
                problems.append(f"{root.name}: {Path(argv[-1]).name} did not print one JSON object whose systemMessage "
                                f"mentions python3 (exit {p.code}): {snippet(p.text, 100)}")
            if root.name == "anthology-guardrails" and (bins["present"] / "python3").exists():
                present = wrapper(root, argv, "present")
                if present.out.strip():
                    problems.append(f"{root.name}: prints output although python3 is present: {snippet(present.out, 80)}")
    return (FAIL, "; ".join(problems)) if problems else (PASS, f"{len(cmds)} wrapper(s) warn about the missing python3")


def _unpushed_repo(tmp: Path) -> Path:
    git(tmp, tmp, "init", "-q", "--bare", "origin.git")
    git(tmp, tmp, "clone", "-q", "origin.git", "repo")
    repo = tmp / "repo"
    set_identity(tmp, repo)
    git(tmp, repo, "checkout", "-q", "-b", "main")
    (repo / "README.md").write_text("smoke\n", encoding="utf-8")
    git(tmp, repo, "add", "-A")
    git(tmp, repo, "commit", "-q", "-m", "init")
    git(tmp, repo, "push", "-q", "-u", "origin", "main")
    git(tmp, repo, "checkout", "-q", "-b", "feat")
    (repo / "feat.txt").write_text("x\n", encoding="utf-8")
    git(tmp, repo, "add", "-A")
    git(tmp, repo, "commit", "-q", "-m", "feat: smoke-unpushed-marker")
    return repo


def _worktree_run(ctx: Ctx, base_ref_head: bool) -> Tuple[Proc, bool, bool]:
    """One run of the implementer; returns (process, an agent worktree was used, the marker is in a subagent tool result)."""
    settings = json.loads(json.dumps(READ_ALLOW))
    if base_ref_head:
        settings["worktree"] = {"baseRef": "head"}
    with scratch() as tmp:
        repo = _unpushed_repo(tmp)
        p = ctx.prompt_run(tmp, PROMPT_WORKTREE, ["--model", "sonnet", *plugin_flags(), "--settings", json.dumps(settings)],
                           cwd=repo, variadic=["Agent", "Read", "Bash(git log*)"])
        texts = [f.read_text(encoding="utf-8", errors="replace") for f in subagent_files(tmp / "cfg")]
        listed = ".claude/worktrees/agent-" in git(tmp, repo, "worktree", "list")
        # An agent that changes nothing has its worktree removed again, so the `cwd` of the subagent
        # transcript entries is accepted as evidence too.
        in_cwd = any(".claude/worktrees/agent-" in c for t in texts for c in transcript_cwds(t))
        # The marker must be what the SUBAGENT's Bash returned: the main session runs on `feat` itself.
        marker = any(MARKER in r for t in texts for r in tool_result_texts(stream_events(t)))
        return p, listed or in_cwd, marker


def smoke_worktree_base(ctx: Ctx) -> Result:
    """U2: with worktree.baseRef=head an isolated agent sees the unpushed commit; the platform default does not."""
    p, worktree, marker = _worktree_run(ctx, True)
    _, _, control_marker = _worktree_run(ctx, False)
    note = f"control without baseRef: marker {'present' if control_marker else 'absent'} in the agent"
    if p.code != 0:
        return FAIL, f"claude exited {p.code}: {snippet(p.text)}; {note}"
    if marker and worktree:
        return PASS, f"the unpushed commit is visible in an agent worktree; {note}"
    return FAIL, f"marker in the agent's tool results: {marker}; agent worktree seen: {worktree}; {note}"


def smoke_model_check(ctx: Ctx) -> Result:
    """U2: each agent's subagent transcript carries the model family its frontmatter names."""
    roster = {a: v["model"] for a, v in load_agents(ROOT / "plugins" / "anthology" / "agents").items()}
    if len(roster) != EXPECTED_AGENTS:
        return FAIL, f"found {len(roster)} agents, want {EXPECTED_AGENTS}: {', '.join(sorted(roster))}"
    bad = []
    for agent, family in roster.items():
        with scratch() as tmp:
            repo = make_repo(tmp)
            p = ctx.prompt_run(tmp, f"Use the Agent tool to run the anthology:{agent} agent with the prompt "
                               "'Reply OK.' and quote its reply.",
                               ["--model", "sonnet" if family == "haiku" else "haiku",  # never the agent's own model
                                *plugin_flags(), "--settings", json.dumps(READ_ALLOW),
                                "--max-turns", "4"], cwd=repo, variadic=["Agent", "Read"])
            models, files = transcript_models(tmp / "cfg")
            if not files:
                bad.append(f"{agent}: no transcript under {tmp.name}/cfg/projects/*/*/subagents/ "
                           f"(exit {p.code}: {snippet(p.text, 80)})")
            elif not any(family in m for m in models):
                bad.append(f"{agent}: wanted {family}, saw {sorted(set(models))}")
    return (FAIL, "; ".join(bad)) if bad else (PASS, f"all {len(roster)} agents ran on their frontmatter model")


def smoke_knowledge_read_writers(ctx: Ctx) -> Result:
    """U1: each isolation:worktree agent reads its Startup files in order before its first Glob (plugin eval
    cannot run these: it refuses worktree isolation). Preload agents must not call Skill and quote the heading."""
    agents = load_agents(ROOT / "plugins" / "anthology" / "agents")
    writers = {a: v for a, v in agents.items() if v["worktree"]}
    if not writers:
        return FAIL, f"no isolation: worktree agent found among {len(agents)} agent file(s)"
    empty = [a for a, v in writers.items() if not v["startup"]]
    if empty:
        return FAIL, f"no Startup paths parsed for: {', '.join(empty)}"
    bad = []
    for agent, spec in writers.items():
        headings = [skill_heading(p) for p in spec["preloads"]]
        ending = "reply DONE followed by the first heading of each skill preloaded into your context." if headings else "reply DONE."
        prompt = (f"Use the Agent tool to run the anthology:{agent} agent with exactly this prompt: 'Startup check only: "
                  f"do your Startup reading, then use Glob once to list *.md files in the current folder, then {ending}' "
                  "Then quote its reply.")
        with scratch() as tmp:
            repo = make_repo(tmp)
            # the agent may run in a worktree of this repo; that is expected, not a failure
            p = ctx.prompt_run(tmp, prompt, ["--model", "sonnet", *plugin_flags(), "--settings", json.dumps(READ_ALLOW)],
                               cwd=repo, variadic=["Agent", "Read", "Glob"])
            files = subagent_files(tmp / "cfg")
            if not files:
                bad.append(f"{agent}: no subagent transcript (exit {p.code}: {snippet(p.text, 80)})")
                continue
            calls = [c for f in files for c in transcript_calls(f.read_text(encoding="utf-8", errors="replace"))]
            problems = startup_problems(calls, spec["startup"])
            if headings:
                if any(n == "Skill" for n, _ in calls):
                    problems.append("called Skill for a preload")
                problems += [f"reply lacks the heading {h!r}" for h in headings if h not in p.out]
            if problems:
                bad.append(f"{agent}: " + ", ".join(problems))
    return (FAIL, "; ".join(bad)) if bad else (PASS, f"all {len(writers)} worktree agents read their Startup files in order")


def _approved_repo(ctx: Ctx, tmp: Path) -> Path:
    repo = make_repo(tmp)
    git(tmp, repo, "remote", "add", "origin", "https://example.invalid/smoke/repo.git")
    code = ("import sys; sys.path.insert(0, sys.argv[1]); import memory_sync; "
            "from pathlib import Path; memory_sync.approve(Path(sys.argv[2]))")
    p = run([sys.executable, "-c", code, str(ROOT / "plugins/anthology/scripts"), str(repo)], repo, build_env(tmp), 60)
    if p.code != 0:
        raise RuntimeError(f"memory_sync.approve failed: {snippet(p.err)}")
    return repo


def smoke_auto_memory_dir(ctx: Ctx) -> Result:
    """U7: the memory sync points autoMemoryDirectory at the repo, migrates old notes, and saves go there."""
    if not has_session_start(ROOT / "plugins/anthology/hooks/hooks.json"):
        return SKIP, "no SessionStart memory sync yet (stream F)"
    flags = ["--permission-mode", "bypassPermissions", "--model", "sonnet", *plugin_flags(), "--max-turns", "6"]
    with scratch() as tmp:  # bypassPermissions: cwd/HOME/config in this temp dir (not a sandbox), guardrails loaded
        repo = _approved_repo(ctx, tmp)
        mem = repo / ".claude" / "memory"
        ctx.prompt_run(tmp, "Remember for future sessions that the smoke probe color is amber; save it to auto memory now.",
                       flags, cwd=repo)
        # Session 1 starts before the setting exists, so the note goes to the platform's default memory dir;
        # a note already in the repo would make the migration check below pass without migrating anything.
        default_dirs = list((tmp / "cfg" / "projects").glob("*/memory"))
        if find_text(mem, "amber") or not any(find_text(d, "amber") for d in default_dirs):
            return FAIL, ("session 1: want the amber note in the default memory dir (cfg/projects/*/memory) and not yet in "
                          f"the repo; in the default dir: {any(find_text(d, 'amber') for d in default_dirs)}, "
                          f"in the repo: {find_text(mem, 'amber')}")
        try:
            setting = json.loads((repo / ".claude/settings.local.json").read_text("utf-8")).get("autoMemoryDirectory")
        except (OSError, ValueError):
            setting = None
        if not setting or os.path.realpath(setting) != os.path.realpath(str(mem)):
            return FAIL, f"settings.local.json autoMemoryDirectory is {setting!r}, want {mem}"
        ctx.prompt_run(tmp, "Reply OK.", flags, cwd=repo)
        if not find_text(mem, "amber"):
            return FAIL, "the amber note is not under .claude/memory/ after session 2 (migration)"
        ctx.prompt_run(tmp, "Remember that the smoke probe shape is cyan-circle; save it to auto memory now.", flags, cwd=repo)
        if not find_text(mem, "cyan-circle"):
            return FAIL, "the cyan-circle note is not under .claude/memory/ after session 3"
    return PASS, "autoMemoryDirectory set, old note migrated, new note saved in the repo"


def smoke_writer_memory_commit(ctx: Ctx) -> Result:
    """U7: a writer agent commits its memory note in its worktree under chore(memory), at the right path."""
    prompt = ("Use the Agent tool to run the anthology:implementer agent with the prompt 'Save a project memory note named "
              "project-smoke-probe.md saying the smoke probe is teal because this smoke checks writer memory commits; follow "
              "your Memory rules, then finish.' Then report its reply.")
    want = ".claude/agent-memory/anthology-implementer/project-smoke-probe.md"
    with scratch() as tmp:  # bypassPermissions: cwd/HOME/config in this temp dir (not a sandbox), guardrails loaded
        repo = make_repo(tmp)
        p = ctx.prompt_run(tmp, prompt, ["--permission-mode", "bypassPermissions", "--model", "sonnet", *plugin_flags(),
                                         "--settings", json.dumps({"worktree": {"baseRef": "head"}})], cwd=repo)
        found = False
        for branch in git(tmp, repo, "branch", "--list", "worktree-agent-*", "--format=%(refname:short)").split():
            for sha in git(tmp, repo, "rev-list", branch, "--not", "main").split():
                subject = git(tmp, repo, "log", "-1", "--format=%s", sha)
                added = git(tmp, repo, "diff-tree", "--no-commit-id", "--name-status", "-r", sha).splitlines()
                found = found or (subject.startswith("chore(memory)") and f"A\t{want}" in added)
        stray = [os.path.join(d, n) for d, dirs, files in os.walk(tmp) for n in dirs + files
                 if "agent-memory/anthology:" in os.path.join(d, n)]
        if stray:
            return FAIL, f"a note landed in a path with the agent type's colon: {stray[0]}"
        if not found:
            return FAIL, f"no worktree-agent-* branch has a chore(memory) commit adding {want} (exit {p.code}: {snippet(p.text, 80)})"
    return PASS, "chore(memory) commit adds the note in the agent's worktree branch"


SMOKES: Dict[str, Tuple[Callable[[Ctx], Result], bool]] = {  # name -> (function, needs a model)
    "cli-syntax": (smoke_cli_syntax, False),
    "eval-ceiling": (smoke_eval_ceiling, True),
    "context-cap": (smoke_context_cap, True),
    "missing-interpreter": (smoke_missing_interpreter, True),
    "python-missing": (smoke_python_missing, False),
    "worktree-base": (smoke_worktree_base, True),
    "model-check": (smoke_model_check, True),
    "knowledge-read-writers": (smoke_knowledge_read_writers, True),
    "auto-memory-dir": (smoke_auto_memory_dir, True),
    "writer-memory-commit": (smoke_writer_memory_commit, True),
}


def default_claude() -> str:
    local = ROOT / "ci" / "claude-code" / "node_modules" / ".bin" / "claude"
    return os.environ.get("CLAUDE_BIN") or (str(local) if os.access(local, os.X_OK) else "claude")


def binary_exists(claude: str) -> bool:
    return bool(shutil.which(claude)) or (Path(claude).is_file() and os.access(claude, os.X_OK))


def run_smoke(name: str, ctx: Ctx, explicit: bool = True) -> Result:
    func, needs_model = SMOKES[name]
    if ctx.spent >= ctx.total:
        return FAIL, f"smoke budget exhausted (${ctx.spent:.2f} of ${ctx.total:.2f}); raise --total-budget-usd"
    if name != "python-missing" and not binary_exists(ctx.claude):
        # a binary the caller named (flag or $CLAUDE_BIN) must exist: a silent SKIP would hide a broken install
        return (FAIL if explicit else SKIP), f"claude binary not found: {ctx.claude}"
    if needs_model and not has_auth():
        return SKIP, NO_AUTH
    try:
        status, reason = func(ctx)
    except BudgetExhausted:
        status, reason = FAIL, f"smoke budget exhausted (${ctx.spent:.2f} of ${ctx.total:.2f}); raise --total-budget-usd"
    except Exception as exc:  # a smoke must never crash the run
        status, reason = FAIL, f"{exc.__class__.__name__}: {snippet(str(exc))}"
    return status, " ".join(redact(reason).split())[:400]


def positive(text: str) -> float:
    try:
        value = float(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a number: {text!r}")
    if not 0 < value < float("inf"):
        raise argparse.ArgumentTypeError(f"must be greater than 0: {text!r}")
    return value


def main(argv: Optional[Sequence[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Anthology smoke cases U1-U9")
    ap.add_argument("--claude", help="claude binary (default $CLAUDE_BIN, the pinned CI one, else claude)")
    ap.add_argument("--budget-usd", default="1.00", help="--max-budget-usd for every claude -p (default 1.00)")
    ap.add_argument("--total-budget-usd", type=positive, default=10.0,
                    help="stop launching claude runs once this much has been spent (default 10)")
    ap.add_argument("--out", help="write a JSON summary here")
    ap.add_argument("--list", action="store_true", help="print the smoke names and exit")
    ap.add_argument("names", nargs="*", metavar="NAME")
    args = ap.parse_args(argv)
    if args.list:
        print("\n".join(SMOKES))
        return 0
    try:
        positive(args.budget_usd)
    except argparse.ArgumentTypeError as exc:
        ap.error(f"--budget-usd {exc}")
    explicit = bool(args.claude or os.environ.get("CLAUDE_BIN"))
    claude = args.claude or default_claude()
    if os.sep in claude:  # runs happen in other directories, so a path must be absolute
        claude = os.path.abspath(claude)
    unknown = [n for n in args.names if n not in SMOKES]
    if unknown:
        ap.error(f"unknown smoke(s): {', '.join(unknown)}; known: {', '.join(SMOKES)}")
    ctx = Ctx(claude, args.budget_usd, args.total_budget_usd)
    results: Dict[str, Dict[str, str]] = {}
    for name in args.names or list(SMOKES):
        status, reason = run_smoke(name, ctx, explicit)
        results[name] = {"status": status, "reason": reason}
        print(f"{status} {name}: {reason}", flush=True)
    print(f"smoke spend: ${ctx.spent:.2f} of ${ctx.total:.2f}")
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps({"smokes": results}, indent=2) + "\n", encoding="utf-8")
    return 1 if any(r["status"] == FAIL for r in results.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
