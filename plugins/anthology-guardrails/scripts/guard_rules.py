"""Rule engine for the anthology-guardrails PreToolUse hook (rules G1-G6).

`check(tool_name, tool_input, home)` returns `(rule_id, message)` for a call that must be
blocked, else None. It never raises: any internal error means "allow" (a guard bug must not
brick a session).

The guard stops a cooperative-but-mistaken agent, including every wrapper form such an agent
would naturally write (chains, prefixes, `rtk`, `sh -c`, `$(...)`, `cd` then a relative path,
globs, literal text piped into a shell). Deliberate obfuscation (base64 | sh, scripts that
build the command, variables assembled piecewise, symlinks) is out of scope: a hook is not
a sandbox, and the native deny rules are the backstop.

Layout: a quote-aware lexer turns a command string into segments; `_strip_prefixes` peels
wrappers off each segment; `_collect` flattens everything (nested shells, substitutions,
heredocs) into entries; the per-rule predicates then run over the entries.
"""
from __future__ import annotations

import fnmatch
import posixpath
import re

# Home-relative profile roots (also used by the native deny rules in the anthology plugin).
PROFILE_ROOTS = (
    "Library/Application Support/Google",
    "Library/Application Support/Chromium",
    ".config/google-chrome",
    ".config/chromium",
)

MESSAGES = {
    "G1": "run it inside Docker (OrbStack); for browser work use BrowserOS Neo",
    "G2": "never launch Chrome on the host; use BrowserOS Neo",
    "G3": "Chrome profiles are off-limits",
    "G4": "agent-browser is not used here; use the BrowserOS Neo MCP tools",
    "G5": "use the BrowserOS Neo MCP tools",
    "G6": "memory is committed to the repo; never store secrets there",
}

# A full hook-input payload that must block (json.dumps it into guard.py's stdin).
SELF_TEST_SAMPLE = {
    "hook_event_name": "PreToolUse",
    "tool_name": "Bash",
    "tool_input": {"command": "npx playwright install chromium"},
}

_MAX_DEPTH = 12         # nested shells / substitutions
_MAX_CWD_DEPTH = 64     # stop tracking `cd` chains deeper than this many components
_MAX_WALK_DEPTH = 12    # nested dict/list values in non-shell tools
_MAX_PATH_STRING = 4096

_SHELLS = frozenset(("bash", "sh", "zsh", "dash", "ksh"))
_DOCKER = frozenset(("docker", "docker-compose"))
_JS_PACKAGE_MANAGERS = frozenset(("npm", "pnpm", "yarn", "bun"))


# ------------------------------------------------------------------ lexer

class _Unbalanced(Exception):
    """Raised by the lexer on an unterminated quote or substitution."""


class _Group:
    """Identity shared by the segments of one pipeline (joined by `|`), plus the literal stdin
    text (heredoc bodies, here-strings) written for it."""
    __slots__ = ("texts",)

    def __init__(self):
        self.texts = []


class _Seg:
    """One simple command: its words and its redirections [(operator, target)]."""
    __slots__ = ("tokens", "redirs", "group")

    def __init__(self, group):
        self.tokens = []
        self.redirs = []
        self.group = group


_HEREDOC_START = re.compile(r"<<(?!<)-?[ \t]*(['\"]?)([A-Za-z_][A-Za-z0-9_.-]*)\1")


def _find_heredoc_end(text, i, delimiter):
    """For a heredoc body starting at text[i]: (start of the terminator line, index just past
    it), or (-1, -1) when the terminator never appears."""
    n = len(text)
    while i <= n:
        end = text.find("\n", i)
        stop = n if end < 0 else end
        if text[i:stop].strip() == delimiter:
            return i, min(stop + 1, n)
        if end < 0:
            break
        i = end + 1
    return -1, -1


def _scan_paren(text, i):
    """Return (inner, index after `)`) for the parenthesis opened just before text[i].
    Quotes, escapes and heredoc bodies (`$(cat <<'EOF' ... EOF\\n)`) are skipped over."""
    depth, j, n = 1, i, len(text)
    delimiters = []
    unterminated = set()                                  # delimiters already searched in vain
    while j < n:
        c = text[j]
        if c == "\\":
            j += 2
            continue
        if c == "<":
            match = _HEREDOC_START.match(text, j)
            if match:
                if match.group(2) not in unterminated:
                    delimiters.append(match.group(2))
                j = match.end()
                continue
        if c == "\n" and delimiters:
            pos = j + 1
            for delimiter in delimiters:
                after = _find_heredoc_end(text, pos, delimiter)[1]
                if after >= 0:
                    pos = after
                else:
                    unterminated.add(delimiter)
            j = pos - 1                                   # the loop's `j += 1` lands on `pos`
            delimiters = []
        if c in "'\"":
            k = text.find(c, j + 1)
            j = n if k < 0 else k + 1
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return text[i:j], j + 1
        j += 1
    return text[i:], n


def _find_backtick(text, i):
    j, n = i, len(text)
    while j < n:
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == "`":
            return j
        j += 1
    raise _Unbalanced()


_HEREDOC_DELIMITER = re.compile(r"[A-Za-z_][A-Za-z0-9_.-]*\Z")
_ARRAY_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\+?=\Z")


def _body_substitutions(body):
    """The `$(...)` and backtick spans inside an unquoted heredoc body (bash runs them)."""
    found, j, n = [], 0, len(body)
    while j < n:
        c = body[j]
        if c == "\\":
            j += 2
        elif c == "$" and body[j + 1:j + 2] == "(":
            inner, j = _scan_paren(body, j + 2)
            found.append(inner)
        elif c == "`":
            k = body.find("`", j + 1)
            if k < 0:
                break
            found.append(body[j + 1:k])
            j = k + 1
        else:
            j += 1
    return found


class _Lexer:
    """Quote-aware splitter. Nested `$(...)`, backticks and `( ... )` are lexed recursively and
    their segments spliced in ahead of the segment that contains them (execution order)."""

    def __init__(self, text, depth):
        self.text = text
        self.depth = depth
        self.out = []
        self.group = _Group()
        self.seg = _Seg(self.group)
        self.cur = []
        self.has_word = False
        self.pending = None      # redirection operator waiting for its target word
        self.heredocs = []       # (delimiter, group) registered on the current line
        self.unterminated = set()  # delimiters whose terminator was not found
        self.quoted = False      # the word being built contains a quote or escape

    # -- building words and segments
    def add(self, chars):
        self.cur.append(chars)
        self.has_word = True

    def end_word(self):
        if not self.has_word:
            return
        word = "".join(self.cur)
        self.cur = []
        self.has_word = False
        quoted, self.quoted = self.quoted, False
        op, self.pending = self.pending, None
        if op is None:
            self.seg.tokens.append(word)
        elif op in ("<<", "<<-"):
            self.heredocs.append((word, self.group, quoted))
        elif op == "<<<":
            self.group.texts.append(word)
        else:
            self.seg.redirs.append((op, word))

    def end_seg(self, new_group):
        self.end_word()
        self.pending = None
        if self.seg.tokens or self.seg.redirs:
            self.out.append(self.seg)
        if new_group:
            self.group = _Group()
        self.seg = _Seg(self.group)

    def nested(self, inner):
        if self.depth < _MAX_DEPTH:
            self.out.extend(_segments(inner, self.depth + 1))
        self.has_word = True

    # -- the scan
    def run(self):
        text, n, i = self.text, len(self.text), 0
        while i < n:
            c = text[i]
            if c == "\\":
                self.quoted = True
                if i + 1 < n and text[i + 1] != "\n":
                    self.add(text[i + 1])
                i += 2
            elif c == "'":
                self.quoted = True
                j = text.find("'", i + 1)
                if j < 0:
                    raise _Unbalanced()
                self.add(text[i + 1:j])
                i = j + 1
            elif c == '"':
                self.quoted = True
                i = self.double_quoted(i)
            elif c == "`":
                j = _find_backtick(text, i + 1)
                self.nested(text[i + 1:j])
                i = j + 1
            elif c == "$":
                i = self.dollar(i)
            elif c in " \t\r":
                self.end_word()
                i += 1
            elif c == "\n":
                self.end_seg(new_group=True)
                i = self.take_heredocs(i + 1)
            elif c == "#" and not self.has_word:
                j = text.find("\n", i)
                i = n if j < 0 else j
            elif c == "(" and self.has_word and _ARRAY_ASSIGNMENT.match("".join(self.cur)):
                inner, i = _scan_paren(text, i + 1)       # `names=(a b)` is an array, not a subshell
                self.add("(" + inner + ")")
                # The elements are analyzed as the arguments of an `echo` (so their substitutions
                # run and their paths are checked), without reading element 0 as a program.
                self.nested("echo " + inner.replace("\n", " "))
            elif c in ";()":
                self.end_seg(new_group=True)
                i += 1
            elif c == "&" and text[i + 1:i + 2] != ">":
                self.end_seg(new_group=True)
                i += 1
            elif c == "|":
                double = text[i + 1:i + 2] == "|"
                self.end_seg(new_group=double)
                i += 2 if double or text[i + 1:i + 2] == "&" else 1
            elif c in "<>&":
                i = self.redirect(i)
            else:
                self.add(c)
                i += 1
        self.end_seg(new_group=True)
        return self.out

    def double_quoted(self, i):
        text, n = self.text, len(self.text)
        self.has_word = True
        j = i + 1
        while j < n:
            c = text[j]
            if c == '"':
                return j + 1
            if c == "\\":
                nxt = text[j + 1:j + 2]
                if nxt == "\n":
                    j += 2
                elif nxt and nxt in '\\$"`':
                    self.cur.append(nxt)
                    j += 2
                else:
                    self.cur.append(c)
                    j += 1
            elif c == "`":
                k = _find_backtick(text, j + 1)
                self.nested(text[j + 1:k])
                j = k + 1
            elif c == "$" and text[j + 1:j + 2] == "(":
                inner, j = _scan_paren(text, j + 2)
                self.nested(inner)
            elif c == "$" and text[j + 1:j + 2] == "{":
                k = text.find("}", j + 2)
                k = n - 1 if k < 0 else k
                self.cur.append(text[j:k + 1])
                j = k + 1
            else:
                self.cur.append(c)
                j += 1
        raise _Unbalanced()

    def dollar(self, i):
        text = self.text
        nxt = text[i + 1:i + 2]
        if nxt == "(":
            inner, j = _scan_paren(text, i + 2)
            self.nested(inner)
            return j
        if nxt == "{":
            k = text.find("}", i + 2)
            k = len(text) - 1 if k < 0 else k
            self.add(text[i:k + 1])
            return k + 1
        if nxt == "'":
            self.quoted = True
            j, n = i + 2, len(text)
            buf = []
            while j < n and text[j] != "'":
                if text[j] == "\\" and j + 1 < n:
                    j += 1
                buf.append(text[j])
                j += 1
            if j >= n:
                raise _Unbalanced()
            self.add("".join(buf))
            return j + 1
        if nxt == '"':
            return i + 1
        self.add("$")
        return i + 1

    def redirect(self, i):
        text = self.text
        if self.has_word and "".join(self.cur).isdigit():
            self.cur, self.has_word = [], False          # `2>file`: the fd number is not a word
        else:
            self.end_word()
        j = i + 1 if text[i] == "&" else i
        if text[j] == ">":
            j += 1
            if text[j:j + 1] and text[j] in ">|&":
                j += 1
        else:
            j += 1
            nxt = text[j:j + 1]
            if nxt == "<":
                j += 1
                if text[j:j + 1] and text[j] in "<-":
                    j += 1
            elif nxt and nxt in ">&":
                j += 1
        if text[j:j + 1] != "(":                          # `<(cmd)` is a substitution, not a file
            self.pending = text[i:j]
        return j

    def take_heredocs(self, i):
        """At the start of the line after a heredoc operator: capture the body (if closed)."""
        text = self.text
        for delimiter, group, quoted in self.heredocs:
            if delimiter in self.unterminated or not _HEREDOC_DELIMITER.match(delimiter):
                continue
            body_end, after = _find_heredoc_end(text, i, delimiter)
            if after < 0:
                self.unterminated.add(delimiter)          # later lines cannot close it either
                continue
            body = text[i:body_end]
            group.texts.append(body)
            if not quoted and self.depth < _MAX_DEPTH:    # an unquoted body runs its `$(...)`
                for inner in _body_substitutions(body):
                    self.out.extend(_segments(inner, self.depth + 1))
            i = after
        self.heredocs = []
        return i


_SHELL_C_FLAG = re.compile(r"-[A-Za-z]*c[A-Za-z]*\Z")


def _lenient_segments(text):
    """Fallback when quotes are unbalanced: whitespace split, quote characters dropped."""
    out = []
    for piece in re.split(r"[;&|\n()`]+|\$\(", text):
        tokens = [t for t in piece.replace("'", " ").replace('"', " ").split() if t]
        if not tokens:
            continue
        seg = _Seg(_Group())
        seg.tokens = tokens
        out.append(seg)
        for k, token in enumerate(tokens[:-1]):          # `bash -c <rest of the line>`
            if _SHELL_C_FLAG.match(token):
                extra = _Seg(_Group())
                extra.tokens = tokens[k + 1:]
                out.append(extra)
    return out


def _segments(text, depth=0):
    try:
        return _Lexer(text, depth).run()
    except _Unbalanced:
        return _lenient_segments(text)


# ------------------------------------------------------------- prefixes

_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
_KEYWORDS = frozenset(("if", "then", "else", "elif", "do", "while", "until", "{", "coproc"))

# Wrapper program -> flags that consume the following word.
_WRAPPERS = {
    "sudo": frozenset(("-u", "-g", "-C", "-h", "-p", "-r", "-t", "-D", "-R", "-T", "-U",
                       "--user", "--group", "--host", "--prompt", "--chdir")),
    "doas": frozenset(("-u", "-C")),
    "env": frozenset(("-u", "-C", "--unset", "--chdir")),
    "exec": frozenset(("-a",)),
    "time": frozenset(("-f", "-o")),
    "nohup": frozenset(),
    "nice": frozenset(("-n", "--adjustment")),
    "timeout": frozenset(("-s", "-k", "--signal", "--kill-after")),
    "xargs": frozenset(("-I", "-n", "-P", "-L", "-s", "-d", "-E", "-a", "--max-args",
                        "--max-lines", "--max-procs", "--replace", "--delimiter")),
    "xvfb-run": frozenset(("-s", "-n", "-e", "-f", "-p", "--server-args", "--server-num",
                           "--error-file", "--auth-file")),
    "caffeinate": frozenset(("-t", "-w")),
    "stdbuf": frozenset(),
    "watch": frozenset(("-n", "--interval")),
    # macOS / coreutils variants an agent writes just as naturally
    "arch": frozenset(),
    "gtimeout": frozenset(("-s", "-k", "--signal", "--kill-after")),
    "setsid": frozenset(),
    "ionice": frozenset(("-c", "-n", "-p", "-P", "-u")),
    "unbuffer": frozenset(),
}
_TIMEOUTS = ("timeout", "gtimeout")
# Flags of npm/pnpm/yarn/bun that take a value and may precede the subcommand.
_PM_VALUE_FLAGS = frozenset(("--filter", "-F", "-C", "--dir", "--prefix", "-w", "--workspace",
                             "--cwd", "--registry"))
_RUNNERS = frozenset(("npx", "bunx", "pnpx", "uvx"))
_RUNNER_VALUE_FLAGS = frozenset(("-p", "--package", "-w", "--workspace", "--from", "--with",
                                 "--python", "-n", "--name", "--prefix", "--project", "--directory",
                                 "--env-file", "--extra", "--group", "-P"))
_RUNNER_SUBCOMMANDS = frozenset(("exec", "dlx", "x"))
# Python project runners: `<name> [<middle>] run [options] <program> ...`
_PYTHON_GLOBAL_VALUE_FLAGS = frozenset((
    "-C", "--directory", "--project", "-e", "--env", "-p", "--python", "--config-file",
    "--cache-dir", "--env-file", "-n", "--name", "--prefix"))
_PYTHON_RUNNERS = {"uv": ("run", "tool run"), "poetry": ("run",), "pipenv": ("run",),
                   "pdm": ("run",), "hatch": ("run",), "rye": ("run",), "conda": ("run",)}
_PM_VERBS = frozenset((
    "add", "install", "i", "remove", "rm", "uninstall", "un", "r", "run", "exec", "dlx", "x",
    "init", "publish", "link", "unlink", "update", "upgrade", "up", "test", "start", "build",
    "ci", "cache", "config", "info", "list", "ls", "outdated", "audit", "why", "pack",
    "version", "workspace", "workspaces", "global", "create", "login", "logout", "whoami",
    "help", "prune", "store", "rebuild", "dedupe", "import", "patch", "set", "get", "plugin",
    "node", "bin", "dev"))
_PYTHON = re.compile(r"python[0-9.]*\Z")


def _prog_name(word):
    """Lowercased program name: basename of a path, but scoped packages keep `@scope/name`."""
    word = word.lower()
    if word.startswith("@"):
        return word
    return posixpath.basename(word.rstrip("/")) or word


def _strip_version(word):
    at = word.rfind("@")
    return word[:at] if at > 0 else word


def _skip_flags(tokens, i, value_flags):
    n = len(tokens)
    while i < n:
        token = tokens[i]
        if token == "--":
            return i + 1
        if len(token) > 1 and token.startswith("-"):
            # `-iu bob`: a combined short-flag group ending in a value flag takes the next word
            takes_value = token in value_flags or (
                token[1] != "-" and len(token) > 2 and token[-1].isalpha()
                and "-" + token[-1] in value_flags)
            i += 2 if takes_value else 1
        else:
            break
    return i


def _runner_program(tokens, i, scripts):
    """tokens[i:] follow a package runner (npx, pnpm dlx, ...): skip options, return the rest
    starting at the package program (version stripped). `-c <script>` is recursed."""
    n = len(tokens)
    while i < n:
        token = tokens[i]
        if token == "--":
            i += 1
            break
        if token in ("-c", "--call"):
            if i + 1 < n:
                scripts.append(tokens[i + 1])
            return []
        if token.startswith("--call="):
            scripts.append(token[len("--call="):])
            return []
        if token in _RUNNER_VALUE_FLAGS:
            i += 2
        elif token.startswith("-") and len(token) > 1:
            i += 1
        else:
            break
    rest = tokens[i:]
    if rest:
        rest[0] = _strip_version(rest[0])
    return rest


def _python_runner_start(name, tokens):
    """Index of the first word after `uv run` / `uv tool run` / `poetry run` ..., or -1.
    Global flags (`poetry -C sub run`, `uv --directory e2e run`, `hatch -e test run`) may precede."""
    start = _skip_flags(tokens, 1, _PYTHON_GLOBAL_VALUE_FLAGS)
    for subcommand in _PYTHON_RUNNERS[name]:
        words = subcommand.split()
        if tokens[start:start + len(words)] == words:
            return start + len(words)
    return -1


def _shell_args(args):
    """Classify a shell's arguments: ('c', script) | ('file', None) | ('stdin', None)."""
    i, n = 0, len(args)
    while i < n:
        token = args[i]
        if token in ("-o", "+o", "-O", "+O"):
            i += 2
        elif _SHELL_C_FLAG.match(token):
            return ("c", args[i + 1]) if i + 1 < n else ("stdin", None)
        elif token.startswith(("-", "+")):
            i += 1
        else:
            return "file", None
    return "stdin", None


def _strip_prefixes(tokens):
    """Peel assignments, keywords, wrappers and package runners off a command.

    Returns (command tokens starting at the program, embedded scripts to analyze)."""
    toks = list(tokens)
    scripts = []
    for _ in range(64):
        if not toks:
            break
        word = toks[0]
        name = _prog_name(word)
        if _ASSIGNMENT.match(word) or word == "!" or word in _KEYWORDS or word == "":
            toks = toks[1:]                               # `""` is a `$(...)` placeholder
        elif name in _WRAPPERS:
            if name == "env":
                split = next((k for k, t in enumerate(toks) if t in ("-S", "--split-string")), -1)
                if split >= 0 and split + 1 < len(toks):
                    scripts.append(toks[split + 1])
                    toks = []
                    break
            i = _skip_flags(toks, 1, _WRAPPERS[name])
            if name in _TIMEOUTS and i < len(toks):
                i += 1                                    # the duration
            if name == "watch" and i < len(toks) and " " in toks[i]:
                scripts.append(toks[i])                   # watch runs its argument via sh -c
            toks = toks[i:]
        elif name == "command":
            if len(toks) > 1 and toks[1] in ("-v", "-V"):
                return ["which"] + toks[2:], scripts      # lookup, not execution
            toks = toks[_skip_flags(toks, 1, frozenset()):]
        elif name == "rtk":
            i = _skip_flags(toks, 1, frozenset())
            if i < len(toks) and toks[i] in ("bypass", "proxy"):
                i = _skip_flags(toks, i + 1, frozenset())
            toks = toks[i:]
        elif name in _RUNNERS:
            toks = _runner_program(toks, 1, scripts)
        elif name in _PYTHON_RUNNERS and _python_runner_start(name, toks) > 0:
            toks = _runner_program(toks, _python_runner_start(name, toks), scripts)
        elif name in _JS_PACKAGE_MANAGERS and len(toks) > 1:
            i = _skip_flags(toks, 1, _PM_VALUE_FLAGS)
            verb = toks[i] if i < len(toks) else ""
            if verb in _RUNNER_SUBCOMMANDS or (verb == "run" and name != "npm"):
                toks = _runner_program(toks, i + 1, scripts)  # `yarn run playwright ...`
            elif verb == "workspace" and name == "yarn" and len(toks) > i + 2:
                toks = ["yarn"] + toks[i + 2:]                # `yarn workspace <w> <rest>`
            elif name in ("yarn", "pnpm") and verb and verb not in _PM_VERBS:
                toks = _runner_program(toks, i, scripts)  # `yarn playwright test` runs the bin
            else:
                break
        elif len(toks) > 2 and name == "pipx" and toks[1] == "run":
            toks = _runner_program(toks, 2, scripts)
        elif _PYTHON.match(name):
            module = next((k for k, t in enumerate(toks) if t == "-m" and k + 1 < len(toks)), -1)
            first_positional = next(
                (k for k, t in enumerate(toks[1:], 1) if not t.startswith("-")), len(toks))
            if module < 0 or module > first_positional:
                break
            toks = toks[module + 1:]                      # `python -m pip ...` runs `pip`
        else:
            break
    if toks:
        name = _prog_name(toks[0])
        if name in _SHELLS:
            kind, script = _shell_args(toks[1:])
            if kind == "c":
                scripts.append(script)
        elif name == "eval":
            scripts.append(" ".join(toks[1:]))
    return toks, scripts


# ------------------------------------------------------------- entries

class _Entry:
    """A flattened command ready for the per-rule predicates."""
    __slots__ = ("raw", "redirs", "prog", "prog_path", "args", "cwd", "exempt", "group")

    def __init__(self, raw, redirs, cmd, cwd, exempt, group):
        self.raw = raw                                    # every word, wrappers included
        self.redirs = redirs
        self.prog_path = cmd[0].lower() if cmd else ""
        self.prog = _prog_name(cmd[0]) if cmd else ""
        self.args = cmd[1:]
        self.cwd = cwd                                    # tracked `cd` directory, normalized
        self.exempt = exempt                              # docker: G1 and G2 do not apply
        self.group = group


class _Context:
    def __init__(self, home):
        self.home = home
        self.cwd = None
        self.entries = []


_CHDIR_PROGRAMS = frozenset(("tar", "git", "make"))


def _first_arg(args):
    return next((a for a in args if not a.startswith("-")), None)


def _reads_stdin_as_shell(entry):
    """True for a shell fed from stdin (`bash`, `sh -s`) or a docker command that runs one."""
    if entry.prog in _SHELLS:
        return _shell_args(entry.args)[0] == "stdin"
    if entry.prog in _DOCKER:
        return any(_prog_name(a) in _SHELLS and _shell_args(entry.args[k + 1:])[0] == "stdin"
                   for k, a in enumerate(entry.args))
    return False


def _add_command(tokens, redirs, group, ctx, exempt, depth):
    cmd, scripts = _strip_prefixes(tokens)
    entry = _Entry(tokens, redirs, cmd, ctx.cwd, exempt, group)
    entry.exempt = exempt or entry.prog in _DOCKER
    if entry.prog in _CHDIR_PROGRAMS:                     # `tar -C dir x`, `git -C dir ...`
        for k, arg in enumerate(entry.args[:-1]):
            if arg in ("-C", "--directory"):
                entry.cwd = _normalize(entry.args[k + 1], ctx.home, ctx.cwd) or entry.cwd
    ctx.entries.append(entry)
    for script in scripts:
        _collect(script, ctx, exempt, depth + 1)
    if entry.prog in _DOCKER:                             # `docker run img sh -c "..."`
        for k, token in enumerate(entry.args[:-1]):
            if _SHELL_C_FLAG.match(token):
                _collect(entry.args[k + 1], ctx, True, depth + 1)
    elif entry.prog == "find":
        _collect_find_exec(entry, ctx, depth)
    elif entry.prog in ("cd", "pushd"):
        target = _first_arg(entry.args)
        if target is None:
            ctx.cwd = _normalize("~", ctx.home, None)
        elif target != "-":
            new_cwd = _normalize(target, ctx.home, ctx.cwd)
            if new_cwd.count("/") <= _MAX_CWD_DEPTH:      # keeps absurd `cd` chains linear
                ctx.cwd = new_cwd
    return entry


def _collect_find_exec(entry, ctx, depth):
    args = entry.args
    k = 0
    while k < len(args):
        if args[k] in ("-exec", "-execdir", "-ok", "-okdir"):
            end = k + 1
            while end < len(args) and args[end] not in (";", "+"):
                end += 1
            if end > k + 1:
                _add_command(args[k + 1:end], [], entry.group, ctx, entry.exempt, depth + 1)
            k = end
        k += 1


def _echo_text(entry):
    args = list(entry.args)
    while args and re.match(r"-[neE]+\Z", args[0]):
        args.pop(0)
    return " ".join(args).replace("\\n", "\n")


def _collect(text, ctx, exempt, depth):
    if depth > _MAX_DEPTH or not text:
        return
    new_entries = []
    for seg in _segments(text):
        new_entries.append(_add_command(seg.tokens, seg.redirs, seg.group, ctx, exempt, depth))
    # Literal text that reaches a shell's stdin is a command: heredocs, here-strings, `echo | sh`.
    by_group = {}
    for entry in new_entries:
        by_group.setdefault(id(entry.group), []).append(entry)
    for entries in by_group.values():
        shell = next((e for e in entries if _reads_stdin_as_shell(e)), None)
        if shell is None:
            continue
        texts = list(entries[0].group.texts)
        texts += [_echo_text(e) for e in entries if e.prog in ("echo", "printf")]
        for body in texts:
            _collect(body, ctx, shell.exempt, depth + 1)


def _analyze(command, home):
    ctx = _Context(home)
    _collect(command, ctx, False, 0)
    return ctx.entries


# ------------------------------------------------------------- paths

_HOME_VAR = re.compile(r"\$\{HOME\}|\$HOME(?![A-Za-z0-9_])")
_TILDE_USER = re.compile(r"~[A-Za-z0-9_.-]+(?=/|\Z)")
_GLOB_CHARS = re.compile(r"[*?\[]")

# (components, last component matches as a prefix: -beta, -unstable, ...)
_PROFILE_COMPONENTS = tuple(
    (tuple(root.lower().split("/")), root.startswith(".config/")) for root in PROFILE_ROOTS)
_MEMORY_COMPONENTS = ((".claude", "memory"), (".claude", "agent-memory"))
_PREFIX_VARIANTS = ("", "-beta", "-unstable", "-dev", "-stable", "-canary")


def _expand(path, home):
    path = path.replace("\\", "/")
    if path == "~" or path.startswith("~/"):
        path = home + path[1:]
    elif _TILDE_USER.match(path):
        path = "/" + path                                 # another user's home: absolute
    return _HOME_VAR.sub(lambda _m: home, path)


def _normalize(path, home, cwd):
    """Expanded, normalized, lowercase path; relative paths resolve against `cwd` if given."""
    path = _expand(path, home)
    if cwd and not path.startswith("/"):
        path = cwd + "/" + path
    return posixpath.normpath(path).lower() if path else ""


def _component_matches(candidate, protected, prefix):
    if _GLOB_CHARS.search(candidate):
        variants = _PREFIX_VARIANTS if prefix else ("",)
        return any(fnmatch.fnmatchcase(protected + v, candidate) for v in variants)
    return candidate.startswith(protected) if prefix else candidate == protected


_WILDCARD_ONLY = re.compile(r"[*?]+\Z")


def _is_home_dir(comps, home_comps):
    """True when the components name a home directory: `home`'s, `/home/<u>`, `<root>/<u>`."""
    return (bool(home_comps) and comps == home_comps) or (
        len(comps) == 2 and comps[0] in ("home", "users")) or comps == ["root"]


def _contains_sequence(path, sequences, prefix_flags=None, home_comps=()):
    """True when `path` holds one of the component sequences: anywhere if absolute, from the
    start (after any `..` or `$var` components) if relative.

    A window of nothing but wildcards (`**/*`, `*/*`) names no particular path and does not
    match, unless it starts directly under a home directory (`~/*/*/*` reaches `~/.config/...`)."""
    comps = [c for c in path.split("/") if c and c != "."]
    absolute = path.startswith("/")
    leading = 0
    if not absolute:
        while leading < len(comps) and (comps[leading] == ".." or "$" in comps[leading]):
            leading += 1
    last_start = len(comps) if absolute else leading
    for index, sequence in enumerate(sequences):
        prefix = prefix_flags[index] if prefix_flags else False
        size = len(sequence)
        for start in range(0, min(last_start, len(comps) - size) + 1):
            window = comps[start:start + size]
            if not all(_component_matches(c, p, prefix and k == size - 1)
                       for k, (c, p) in enumerate(zip(window, sequence))):
                continue
            if not all(_WILDCARD_ONLY.match(c) for c in window):
                return True
            if absolute and _is_home_dir(comps[:start], home_comps):
                return True
    return False


def _path_candidates(token):
    """The token itself, the part after the first `=`, and each `:`-separated piece."""
    parts = [token]
    if "=" in token:
        parts.append(token.split("=", 1)[1])
    pieces = list(parts)
    for part in parts:
        if ":" in part:
            pieces.extend(part.split(":"))
    return [p for p in pieces if p]


_PROFILE_SEQUENCES = tuple(seq for seq, _ in _PROFILE_COMPONENTS)
_PROFILE_PREFIX_FLAGS = tuple(flag for _, flag in _PROFILE_COMPONENTS)


def _is_profile_path(candidate, home, cwd):
    home_comps = [c for c in _normalize(home, home, None).split("/") if c] if home else []
    for base in (None, cwd) if cwd else (None,):
        path = _normalize(candidate, home, base)
        if path and _contains_sequence(
                path, _PROFILE_SEQUENCES, _PROFILE_PREFIX_FLAGS, home_comps):
            return True
    return False


def _is_memory_path(candidate, home, cwd):
    for base in (None, cwd) if cwd else (None,):
        path = _normalize(candidate, home, base)
        if path and _contains_sequence(path, _MEMORY_COMPONENTS):
            return True
    return False


_BRACE_ALTERNATIVES = re.compile(r"\{([^{}]*,[^{}]*)\}")


def _expand_braces(path, limit=32):
    """`a/{b,c}` -> [`a/b`, `a/c`] (bounded; `${HOME}` has no comma and is left alone)."""
    results = [path]
    for _ in range(4):
        expanded, changed = [], False
        for item in results:
            match = _BRACE_ALTERNATIVES.search(item)
            if match:
                changed = True
                expanded.extend(item[:match.start()] + alt + item[match.end():]
                                for alt in match.group(1).split(","))
            else:
                expanded.append(item)
        results = expanded[:limit]
        if not changed:
            break
    return results


def _words_hit(words, predicate, home, cwd):
    for word in words:
        if len(word) > _MAX_PATH_STRING:
            continue
        for piece in _path_candidates(word):
            candidates = _expand_braces(piece) if "{" in piece else (piece,)
            if any(predicate(candidate, home, cwd) for candidate in candidates):
                return True
    return False


# ------------------------------------------------------------- rules

# G1 ------------------------------------------------------------------

_PLAYWRIGHT = frozenset(("playwright", "playwright-core", "@playwright/test", "@playwright/cli"))
_PLAYWRIGHT_SUBCOMMANDS = frozenset((
    "install", "install-deps", "test", "codegen", "open", "cr", "chromium", "ff", "firefox",
    "wk", "webkit", "screenshot", "pdf"))
_PUPPETEER = frozenset(("puppeteer", "@puppeteer/browsers"))
_PUPPETEER_SUBCOMMANDS = frozenset(("browsers", "install"))
_JS_INSTALL_VERBS = frozenset(("i", "install", "add"))
_PIP = re.compile(r"pip[0-9.]*\Z")
_OS_INSTALLERS = frozenset(("brew", "apt", "apt-get", "dnf", "yum", "snap", "pacman"))
_BROWSER_PACKAGES = ("google-chrome*", "chromium*", "ungoogled-chromium", "chromedriver",
                     "playwright*")
_PACMAN_SYNC = re.compile(r"-S[yuw]*\Z")


def _positional(args, value_flags=frozenset()):
    words, i = [], 0
    while i < len(args):
        if args[i] in value_flags:
            i += 1
        elif not args[i].startswith("-"):
            words.append(args[i])
        i += 1
    return words


def _js_installs_puppeteer(entry):
    words = _positional(entry.args, _PM_VALUE_FLAGS)
    if words and words[0] == "global":
        words = words[1:]
    if not words or words[0] not in _JS_INSTALL_VERBS:
        return False
    return any(_strip_version(w).lower() == "puppeteer" for w in words[1:])


def _js_scaffolds_playwright(entry):
    """`npm init playwright`, `yarn create playwright`: downloads browsers unless told not to."""
    words = _positional(entry.args, _PM_VALUE_FLAGS)
    return (len(words) > 1 and words[0] in ("init", "create")
            and _strip_version(words[1]).lower() == "playwright"
            and "--no-browsers" not in entry.args)


def _pip_installs_playwright(entry):
    words = _positional(entry.args)
    if entry.prog == "uv":
        if not words or words[0] != "pip":
            return False
        words = words[1:]
    if not words or words[0] != "install":
        return False
    for spec in words[1:]:
        if re.split(r"[=<>!~\[;@ ]", spec.lower(), maxsplit=1)[0].replace("_", "-") == "playwright":
            return True
    return False


def _os_installs_browser(entry):
    words = _positional(entry.args)
    if entry.prog == "pacman":
        if not any(_PACMAN_SYNC.match(a) for a in entry.args):
            return False
    elif not words or words[0] not in (
            ("install", "reinstall", "upgrade") if entry.prog == "brew" else ("install",)):
        return False
    for package in words[0 if entry.prog == "pacman" else 1:]:
        name = _strip_version(package.lower().rsplit("/", 1)[-1])
        if any(fnmatch.fnmatchcase(name, pattern) for pattern in _BROWSER_PACKAGES):
            return True
    return False


def _g1(entry):
    if entry.exempt:
        return False
    prog = entry.prog
    sub = _first_arg(entry.args)
    if prog in _PLAYWRIGHT:
        return sub in _PLAYWRIGHT_SUBCOMMANDS
    if prog in _PUPPETEER:
        return sub in _PUPPETEER_SUBCOMMANDS
    if prog == "create-playwright":
        return "--no-browsers" not in entry.args
    if prog in _JS_PACKAGE_MANAGERS:
        return _js_installs_puppeteer(entry) or _js_scaffolds_playwright(entry)
    if _PIP.match(prog) or prog in ("pipx", "uv"):
        return _pip_installs_playwright(entry)
    if prog in _OS_INSTALLERS:
        return _os_installs_browser(entry)
    return False


# G2 ------------------------------------------------------------------

# Programs that only look at / print the text `--remote-debugging-port` appears in.
_SEARCH_PROGRAMS = frozenset(("grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf"))


def _chrome_app_name(name):
    name = name.lower()
    return "chrome" in name or "chromium" in name


def _open_launches_chrome(args):
    k = 0
    while k < len(args):
        token = args[k]
        value = args[k + 1] if k + 1 < len(args) else ""
        if token.startswith("-") and not token.startswith("--") and len(token) > 1 \
                and token[-1] in "ab" and token[1:].isalpha():
            if token[-1] == "a" and _chrome_app_name(value):
                return True
            bundle = value.lower()
            if token[-1] == "b" and (bundle.startswith("com.google.chrome")
                                     or bundle.startswith("org.chromium.")):
                return True
            k += 2
            continue
        if not token.startswith("-"):
            app = token.lower().rstrip("/")
            if app.endswith(".app") and _chrome_app_name(posixpath.basename(app)):
                return True
        k += 1
    return False


def _g2(entry):
    if entry.exempt:
        return False
    prog = entry.prog
    if (prog.startswith(("google-chrome", "chromium", "google chrome", "chrome-headless-shell"))
            or prog in ("chrome", "chromedriver", "chrome-launcher")
            or "google chrome.app/" in entry.prog_path or "chromium.app/" in entry.prog_path):
        return True
    if prog == "open" and _open_launches_chrome(entry.args):
        return True
    if prog == "osascript" and any(
            "google chrome" in a.lower() or "chromium" in a.lower() for a in entry.args):
        return True
    if prog not in _SEARCH_PROGRAMS:
        return any(t.startswith("--remote-debugging-port") for t in entry.raw)
    return False


# G3 ------------------------------------------------------------------

def _g3(entry, home):
    words = list(entry.raw) + [target for _, target in entry.redirs]
    return _words_hit(words, _is_profile_path, home, entry.cwd)


# G4 ------------------------------------------------------------------

_G4_EXEMPT_PROGRAMS = frozenset((
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf", "which", "type",
    "ls", "cat", "head", "tail", "less", "wc", "file", "stat", "rm",
    # also only name paths and cannot run them (`find -exec` is analyzed as its own command)
    "cd", "pushd", "mkdir", "touch", "cp", "mv", "find", "du", "diff", "tree", "realpath",
    "basename", "dirname", "readlink", "chmod", "chown", "ln"))
_UNINSTALL_VERBS = frozenset(("uninstall", "remove", "rm", "un", "r"))
_INSPECT_VERBS = frozenset(("ls", "list", "view", "info", "why", "outdated"))


def _names_agent_browser(token):
    if "://" in token:
        return False
    last = token.strip().lower().rsplit("/", 1)[-1]
    return re.split(r"[@:]", last, maxsplit=1)[0] == "agent-browser"


def _g4(entry):
    prog = entry.prog
    if prog in _G4_EXEMPT_PROGRAMS:
        return False
    verb = _first_arg(entry.args)
    if prog in _JS_PACKAGE_MANAGERS and verb in _UNINSTALL_VERBS | _INSPECT_VERBS:
        return False
    if prog == "brew" and verb in ("uninstall", "remove", "rm", "info", "list", "search"):
        return False
    return any(_names_agent_browser(t) for t in entry.raw)


# G5 ------------------------------------------------------------------

_BLOCKED_MCP = ("playwright", "puppeteer", "chrome-devtools", "claude-in-chrome")


def _g5(tool_name):
    if not tool_name.startswith("mcp__"):
        return False
    name = tool_name.lower().replace("_", "-")
    return any(word in name for word in _BLOCKED_MCP)


# G6 ------------------------------------------------------------------

_SECRET_PATTERNS = tuple(re.compile(p) for p in (
    r"(?:AKIA|ASIA)[0-9A-Z]{16}",
    r"\bgh[pousr]_[A-Za-z0-9]+",
    r"github_pat_",
    r"\bsk-[A-Za-z0-9_-]{16,}",
    r"\bxox[baprs]-",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
)) + tuple(re.compile(p, re.IGNORECASE) for p in (
    r"password\s*[:=]",
    r"api[_-]?key\s*[:=]",
))
_CONTENT_KEYS = ("content", "new_string", "new_source")
_PATH_KEYS = ("file_path", "path", "notebook_path")
_FILE_WRITERS = frozenset(("tee", "cp", "mv", "install", "rsync", "ln", "dd"))
_IN_PLACE_EDITORS = frozenset(("sed", "perl"))
_IN_PLACE_FLAG = re.compile(r"-[A-Za-z]*i|--in-place")
_OUTPUT_REDIRECT = re.compile(r"(?:&>|>)")


def _has_secret(text):
    return any(p.search(text) for p in _SECRET_PATTERNS)


def _written_strings(tool_input):
    """Text a file tool writes: content / new_string / new_source / edits[].new_string."""
    for key in _CONTENT_KEYS:
        value = tool_input.get(key)
        if isinstance(value, str):
            yield value
    edits = tool_input.get("edits")
    if isinstance(edits, list):
        for edit in edits:
            if isinstance(edit, dict) and isinstance(edit.get("new_string"), str):
                yield edit["new_string"]


def _g6_file_tool(tool_input, home):
    paths = [tool_input.get(k) for k in _PATH_KEYS if isinstance(tool_input.get(k), str)]
    if not _words_hit(paths, _is_memory_path, home, None):
        return False
    return any(_has_secret(text) for text in _written_strings(tool_input))


def _entry_writes_memory(entry, home):
    for op, target in entry.redirs:
        if _OUTPUT_REDIRECT.match(op) and _words_hit([target], _is_memory_path, home, entry.cwd):
            return True
    prog = entry.prog
    if prog in _FILE_WRITERS or (
            prog in _IN_PLACE_EDITORS and any(_IN_PLACE_FLAG.match(a) for a in entry.args)):
        return _words_hit(entry.args, _is_memory_path, home, entry.cwd)
    return False


def _g6_command(command, entries, home):
    return any(_entry_writes_memory(e, home) for e in entries) and _has_secret(command)


# ------------------------------------------------------------- entry point

_PLAIN_TEXT_KEYS = frozenset(_CONTENT_KEYS + ("old_string", "edits", "command"))


_PATH_KEY_WORDS = ("path", "file", "dir", "folder", "uri", "url", "pattern", "glob", "source",
                   "src", "dest", "destination", "target", "location", "root", "cwd")


def _path_strings(value, skip, in_path_key=False, depth=0):
    """Strings under path-like keys (at any depth, lists included). Free text such as a
    prompt, a query or a todo is not a path. Keys in `skip` (written text, the shell
    command) are never read."""
    if depth > _MAX_WALK_DEPTH:
        return
    if isinstance(value, str):
        if in_path_key:
            yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            if key not in skip:
                path_like = in_path_key or (
                    isinstance(key, str) and any(w in key.lower() for w in _PATH_KEY_WORDS))
                yield from _path_strings(item, skip, path_like, depth + 1)
    elif isinstance(value, (list, tuple)):
        for item in value:
            yield from _path_strings(item, skip, in_path_key, depth + 1)


def _search_roots(tool_input, keys):
    """Glob/Grep: the directory joined with each pattern, so `Google/**` under a parent dir
    is seen as the profile path it will walk."""
    base = tool_input.get("path")
    if not isinstance(base, str):
        return []
    return [posixpath.join(base, tool_input[key])
            for key in keys if isinstance(tool_input.get(key), str)]


def _check(tool_name, tool_input, home):
    name = tool_name if isinstance(tool_name, str) else ""
    inp = tool_input if isinstance(tool_input, dict) else {}
    home = home if isinstance(home, str) else ""
    command = inp.get("command") if isinstance(inp.get("command"), str) else None

    if _g5(name):
        return "G5"
    entries = _analyze(command, home) if command else []
    if _g6_file_tool(inp, home) or (command and _g6_command(command, entries, home)):
        return "G6"
    # Grep's `pattern` is a regex over file contents, not a path; Glob's is a path glob.
    grep = name == "Grep"
    skip = _PLAIN_TEXT_KEYS | {"pattern"} if grep else _PLAIN_TEXT_KEYS
    paths = list(_path_strings(inp, skip)) + _search_roots(inp, ("glob",) if grep else ("pattern", "glob"))
    if _words_hit(paths, _is_profile_path, home, None):
        return "G3"
    for entry in entries:
        if _g3(entry, home):
            return "G3"
        if _g4(entry):
            return "G4"
        if _g1(entry):
            return "G1"
        if _g2(entry):
            return "G2"
    return None


def check(tool_name, tool_input, home):
    """Return (rule_id, message) when the call must be blocked, else None. Never raises."""
    try:
        rule = _check(tool_name, tool_input, home)
    except Exception:  # a guard bug must never brick a session
        return None
    return (rule, MESSAGES[rule]) if rule else None
