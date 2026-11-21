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
import itertools
import posixpath
import re
import shlex
import urllib.parse

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

_SHELLS = frozenset(("bash", "sh", "zsh", "dash", "ksh", "fish", "ash", "mksh", "hush", "yash",
                     "csh", "tcsh"))
# Long shell options that take the next word as their value (bash `--rcfile x`, zsh `--emulate sh`).
_SHELL_VALUE_OPTIONS = frozenset(("--rcfile", "--init-file", "--emulate"))
# Programs whose arguments (and fed stdin text) are inline code: `python3 -c`, `node -e`, ...
_INTERPRETER = re.compile(
    r"(?:python|pypy|ruby|perl|php|lua)[0-9.]*\Z"
    r"|(?:node|nodejs|deno|bun|tsx|ts-node|osascript|rscript)\Z")
# rtk subcommands that run a command (rtk/src/lib.rs `Commands`); `proxy` aliases `bypass`.
_RTK_PASSTHROUGH = frozenset(("bypass", "proxy"))
_RTK_RUNNERS = frozenset(("run", "err", "test", "summary"))
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
        if c == "'":
            k = text.find(c, j + 1)
            j = n if k < 0 else k + 1
            continue
        if c == '"':
            j += 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1          # `\"` does not close the quote
            j += 1
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
        self.array = None        # elements of the `name=( ... )` being lexed, else None
        self.array_word = ""     # its `name=` / `name+=` prefix
        self.array_start = 0     # index just past its `(`

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
        if self.array is not None:
            self.array.append(word)
            return
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

    def open_array(self, i):
        """At the `(` of `name=(`: lex the elements as words until the closing `)`."""
        self.array_word = "".join(self.cur)
        self.cur, self.has_word, self.quoted = [], False, False
        self.array, self.array_start = [], i + 1
        return i + 1

    def array_char(self, i):
        """A metacharacter inside an array literal: words end; `<(..)`/`>(..)` still run."""
        text, c = self.text, self.text[i]
        if c in "<>" and text[i + 1:i + 2] == "(":
            inner, j = _scan_paren(text, i + 2)
            self.nested(inner)
            return j
        self.end_word()
        if c == ")":
            self.close_array(i)
            return i + 1
        if c == "\n":
            return self.take_heredocs(i + 1)
        return i + 1

    def close_array(self, i):
        """The elements become the arguments of an `echo` (their paths are checked and none is
        a program), and `name=( ... )` stays one assignment word of the current command."""
        elements, self.array = self.array, None
        if elements:
            seg = _Seg(_Group())
            seg.tokens = ["echo"] + elements
            self.out.append(seg)
        self.add(self.array_word + "(" + self.text[self.array_start:i] + ")")

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
                if i + 1 < n and text[i + 1] != "\n":     # backslash-newline adds nothing
                    self.quoted = True
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
            elif self.array is not None and c in "\n;&|<>()":
                i = self.array_char(i)
            elif c == "\n":
                self.end_seg(new_group=True)
                i = self.take_heredocs(i + 1)
            elif c == "#" and not self.has_word:
                j = text.find("\n", i)
                i = n if j < 0 else j
            elif c == "(" and self.has_word and _ARRAY_ASSIGNMENT.match("".join(self.cur)):
                i = self.open_array(i)                    # `names=(a b)` is an array, not a subshell
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
        if self.array is not None:                        # unclosed `name=(`: keep its elements
            self.end_word()
            self.close_array(n)
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
        for start, first in _script_starts(tokens):       # `bash -c <rest of the line>`
            extra = _Seg(_Group())
            extra.tokens = [first] + tokens[start + 1:]
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

# Wrapper program -> flags that consume the following word. A wrapper runs the command that
# follows its options (and its operands, `_WRAPPER_OPERANDS`).
_SU_VALUE_FLAGS = frozenset(("-c", "--command", "--session-command", "-s", "--shell", "-g",
                             "--group", "-G", "--supp-group", "-w", "--whitelist-environment",
                             "-u", "--user"))
_WRAPPERS = {
    "sudo": frozenset(("-u", "-g", "-C", "-h", "-p", "-r", "-t", "-D", "-R", "-T", "-U",
                       "--user", "--group", "--host", "--prompt", "--chdir", "--close-from",
                       "--chroot", "--role", "--type", "--command-timeout", "--other-user")),
    "doas": frozenset(("-u", "-C")),
    "env": frozenset(("-u", "-C", "--unset", "--chdir")),   # -S: see _env_split_string
    "exec": frozenset(("-a",)),
    "time": frozenset(("-f", "-o", "--format", "--output")),
    "nohup": frozenset(),
    "nice": frozenset(("-n", "--adjustment")),
    "timeout": frozenset(("-s", "-k", "--signal", "--kill-after")),
    # `--replace` takes its value only attached (`--replace=R`), like `-i[R]`
    "xargs": frozenset(("-I", "-n", "-P", "-L", "-s", "-d", "-E", "-a", "-J", "-R", "-S",
                        "--max-args", "--max-lines", "--max-procs", "--delimiter", "--arg-file",
                        "--max-chars", "--process-slot-var")),
    "xvfb-run": frozenset(("-s", "-n", "-e", "-f", "-p", "--server-args", "--server-num",
                           "--error-file", "--auth-file")),
    "caffeinate": frozenset(("-t", "-w")),
    "stdbuf": frozenset(("-i", "-o", "-e", "--input", "--output", "--error")),
    "watch": frozenset(("-n", "--interval", "-q", "--equexit")),
    # macOS / coreutils variants an agent writes just as naturally
    "arch": frozenset(("-arch", "-e", "-d")),
    "gtimeout": frozenset(("-s", "-k", "--signal", "--kill-after")),
    "setsid": frozenset(),
    "ionice": frozenset(("-c", "-n", "-p", "-P", "-u")),
    "unbuffer": frozenset(),
    # util-linux, busybox and other programs that run the command after their options (FW-I)
    "builtin": frozenset(),
    "busybox": frozenset(),
    "chroot": frozenset(("--userspec", "--groups")),
    "nsenter": frozenset(("-t", "--target", "-S", "--setuid", "-G", "--setgid")),
    "unshare": frozenset(("-S", "--setuid", "-G", "--setgid", "-R", "--root", "-w", "--wd",
                          "--map-user", "--map-group", "--map-users", "--map-groups",
                          "--propagation", "--setgroups")),
    "flock": frozenset(("-w", "--wait", "--timeout", "-E", "--conflict-exit-code", "-c",
                        "--command")),
    "taskset": frozenset(),
    "chrt": frozenset(),
    "setarch": frozenset(),
    "setpriv": frozenset(("--reuid", "--regid", "--ruid", "--rgid", "--euid", "--egid",
                          "--groups", "--inh-caps", "--ambient-caps", "--bounding-set",
                          "--securebits", "--pdeathsig", "--selinux-label", "--apparmor-profile")),
    "prlimit": frozenset(("-p", "--pid")),
    "fakeroot": frozenset(("-l", "--lib", "--faked", "-i", "-s")),
    "strace": frozenset(("-o", "-e", "-p", "-s", "-u", "-E", "-P", "-a", "-b", "-I", "-O", "-S",
                         "-X", "--output")),
    "dbus-run-session": frozenset(("--config-file", "--dbus-daemon")),
    "dumb-init": frozenset(("-r", "--rewrite")),
    "tini": frozenset(("-e", "-p")),
    "pkexec": frozenset(("--user",)),
    "su": _SU_VALUE_FLAGS,
    "runuser": _SU_VALUE_FLAGS,
    "script": frozenset(("-c", "--command", "-E", "--echo", "-m", "--logging-format", "-T",
                         "--log-timing", "-I", "--log-in", "-O", "--log-out", "-B", "--log-io",
                         "-t")),
    "entr": frozenset(),
    "corepack": frozenset(),
    "cross-env": frozenset(),
    "dotenv": frozenset(("-e", "-c", "-v", "-p")),
    "env-cmd": frozenset(("-f", "--file", "-e", "--environments", "-r", "--rc-file")),
    # review round 1 (I3)
    "systemd-run": frozenset(("-u", "--unit", "-p", "--property", "-E", "--setenv", "-M",
                              "--machine", "-H", "--host", "--description", "--slice", "--uid",
                              "--gid", "--nice", "--working-directory", "--on-calendar",
                              "--on-active", "--timer-property", "--path-property",
                              "--socket-property", "--service-type")),
    "dbus-launch": frozenset(("--config-file",)),
    "ssh-agent": frozenset(("-a", "-E", "-t", "-P", "-O")),
    "ltrace": frozenset(("-o", "-e", "-p", "-s", "-u", "-n", "-a", "-A", "-D", "-F", "-l", "-x",
                         "-w", "--output", "--align", "--indent", "--library")),
    "valgrind": frozenset(),
    "chpst": frozenset(("-u", "-U", "-e", "-/", "-n", "-l", "-L", "-m", "-d", "-o", "-p", "-f",
                        "-c")),
    "cpulimit": frozenset(("-l", "--limit", "-p", "--pid", "-e", "--exe", "-c", "--cpu")),
}
# Wrappers whose first word(s) after the options are operands, not the command: `timeout 5
# cmd`, `chroot /new cmd`, `flock file cmd`, `taskset mask cmd`, `chrt prio cmd`, `setarch
# arch cmd`, macOS `script file cmd`. Options may follow the operand (`setarch x86_64 -R cmd`).
_WRAPPER_OPERANDS = {"timeout": 1, "gtimeout": 1, "chroot": 1, "flock": 1, "taskset": 1,
                     "chrt": 1, "setarch": 1, "script": 1}
# Options that give a wrapper a shell command line (`su -c X`, `script -qc X`, `flock f -c X`).
# They are looked for in every later word: util-linux reorders options, so `su root -c X` runs X.
_WRAPPER_SCRIPT_OPTIONS = {
    "su": ("-c", "--command", "--session-command"),
    "runuser": ("-c", "--command", "--session-command"),
    "script": ("-c", "--command"),
    "flock": ("-c", "--command"),
    "nodemon": ("-x", "--exec"),
    "nix-shell": ("--run", "--command"),
}
# `<tool> <subcommand> [options] [--] <command>`: version managers, env loaders, profilers.
_SUBCOMMAND_WRAPPERS = {"mise": ("exec", "x"), "rtx": ("exec", "x"), "asdf": ("exec",),
                        "direnv": ("exec",), "devbox": ("run",), "volta": ("run",),
                        "fnm": ("exec",), "nvm": ("exec",), "pixi": ("run", "exec"),
                        "perf": ("stat", "record", "trace")}
# Programs whose `--` ends their own options or paths and passes the rest to something that is
# not a command line (a test filter, a script's arguments, a pattern): no alternate reading
# starts after their `--` (see `_alternate_trigger`).
_DASHDASH_ARGUMENTS = frozenset((
    "cargo", "go", "make", "gmake", "just", "task", "pytest", "jest", "vitest", "mocha", "ava",
    "mvn", "gradle", "gradlew", "dotnet", "swift", "zig", "rustc", "helm", "terraform", "gh",
    "ansible-playbook"))
# Programs whose every argument is a shell command line (value flags listed are skipped).
_SCRIPT_ARGUMENTS = {
    "concurrently": frozenset(("-n", "--names", "-c", "--prefix-colors", "-p", "--prefix", "-s",
                               "--success", "--restart-tries", "--restart-after", "-m",
                               "--max-processes", "-t", "--timestamp-format", "-l",
                               "--prefix-length", "--name-separator", "--default-input-target")),
    "hyperfine": frozenset(("-w", "--warmup", "-m", "--min-runs", "-M", "--max-runs", "-r",
                            "--runs", "-S", "--shell", "-u", "--time-unit", "--style", "--sort",
                            "--export-asciidoc", "--export-csv", "--export-json",
                            "--export-markdown", "--export-orgmode", "--output", "--input", "-n",
                            "--command-name", "-D", "--parameter-step-size")),
}
_PARALLEL_VALUE_FLAGS = frozenset((
    "-j", "-P", "--jobs", "-a", "--arg-file", "-S", "--sshlogin", "--sshloginfile", "--colsep",
    "-C", "-d", "--delimiter", "-E", "-I", "-n", "-N", "-L", "--max-args", "--max-lines",
    "--halt", "--timeout", "--results", "--joblog", "--tmpdir", "--workdir", "--wd",
    "--tagstring", "--env", "--retries", "--delay", "--memfree", "--load", "--basefile",
    "--return", "--transferfile"))
_PARALLEL_INPUTS = frozenset((":::", "::::", ":::+", "::::+"))
_MAX_FED = 64           # stdin lines (xargs, parallel) and `:::` inputs turned into commands
_MAX_ALTERNATES = 64    # alternate readings of one command
_MAX_ALTERNATE_WORDS = 256  # words of each (an install spells itself in its first words)
# Flags of npm/pnpm/yarn/bun that take a value and may precede the subcommand.
_PM_VALUE_FLAGS = frozenset(("--filter", "-F", "-C", "--dir", "--prefix", "-w", "--workspace",
                             "--cwd", "--registry", "--loglevel", "--userconfig", "--cache",
                             "--reporter", "--tag", "--node-options", "--script-shell"))
_RUNNERS = frozenset(("npx", "bunx", "pnpx", "uvx"))
_RUNNER_VALUE_FLAGS = frozenset(("-p", "--package", "-w", "--workspace", "--from", "--with",
                                 "--python", "-n", "--name", "--prefix", "--project", "--directory",
                                 "--env-file", "--extra", "--group", "-P", "--registry", "--cache",
                                 "--userconfig", "--loglevel", "--node-options", "--script-shell",
                                 "--index", "--index-url", "--extra-index-url", "--default-index",
                                 "--find-links", "--with-requirements", "--with-editable",
                                 "--cache-dir", "--config-file"))
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


def _parse_shell_args(args, i=0):
    """Parse a bash/sh/zsh/dash/ksh argument list (bash(1) "Invocation").

    Options end at the first non-option word, `--` or a lone `-`. A short cluster (`-euo`,
    `+O`) sets every letter, and each `o`/`O` in it takes the next word as its value; `c`
    anywhere means command mode. `--rcfile`, `--init-file` and `--emulate` take a value
    unless written `--x=v`; other long options take none.

    Parsing starts at args[i]. Returns (mode, index of the first word after the options):
    mode 'c' (that word is the script), 'stdin' (`-s`, no words left, or `-c` without a
    script) or 'file'."""
    n = len(args)
    command = stdin = False
    while i < n:
        token = args[i]
        if token in ("--", "-"):
            i += 1
            break
        if token.startswith("--"):
            i += 2 if token in _SHELL_VALUE_OPTIONS else 1
        elif len(token) > 1 and token[0] in "-+":
            letters = token[1:]
            command = command or "c" in letters
            stdin = stdin or "s" in letters
            i += 1 + letters.count("o") + letters.count("O")
        else:
            break
    i = min(i, n)
    if command:
        return ("c", i) if i < n else ("stdin", i)
    return ("stdin" if stdin or i >= n else "file"), i


# fish long options that take a value (`--opt=v` or `--opt v`), and those that do not; getopt
# accepts any unique prefix. From fish's own option table (src/fish.cpp `long_opts`). The scan
# for `-c` in _parse_fish_args does not depend on these lists being complete, and neither does
# the stdin check: a long option in neither list may take a value, so the first word after it
# could be that value, not a script file; fish is then also read as running a script from stdin.
_FISH_LONG_VALUE = ("--command", "--init-command", "--debug", "--debug-output",
                    "--debug-stack-frames", "--features", "--profile", "--profile-startup")
_FISH_LONG_FLAG = ("--interactive", "--login", "--no-config", "--no-execute", "--private",
                   "--print-rusage-self", "--print-debug-categories", "--help", "--version")
_FISH_SHORT_VALUE = "cCdDofp"             # -c -C run code; -d -D -o -f -p take a value
_FISH_SCRIPT_LONG = ("--command", "--init-command")


def _fish_long_option(token):
    """(option, attached value or None) for a `--...` token. A prefix of a value option
    (`--comm`, `--ini`) names it, as getopt would; anything else is returned as written."""
    name, eq, value = token.partition("=")
    fits = [opt for opt in _FISH_LONG_VALUE if len(name) > 2 and opt.startswith(name)]
    return (fits[0] if fits else name), (value if eq else None)


def _fish_option_at(args, i):
    """Read the option word args[i] the way fish would: (scripts, runs_c, takes_next) where
    `scripts` lists the (index, text) of each command it gives fish (the attached value, else
    the next word), `runs_c` is true for `-c`/`--command`, and `takes_next` is true when the
    option consumes the next word. A non-option word gives ([], False, False)."""
    token, n = args[i], len(args)
    scripts, runs_c, takes_next = [], False, False
    if token.startswith("--") and len(token) > 2:
        name, value = _fish_long_option(token)
        takes_next = name in _FISH_LONG_VALUE and value is None
        if name in _FISH_SCRIPT_LONG:
            runs_c = name == "--command"
            if value is not None:
                scripts.append((i, value))
            elif i + 1 < n:
                scripts.append((i + 1, args[i + 1]))
    elif token.startswith("-") and len(token) > 1 and token != "--":
        for k, letter in enumerate(token[1:], 1):
            if letter not in _FISH_SHORT_VALUE:
                continue
            attached = token[k + 1:]
            if letter in "cC":
                runs_c = letter == "c"
                if attached:
                    scripts.append((i, attached))
                elif i + 1 < n:
                    scripts.append((i + 1, args[i + 1]))
            takes_next = not attached
            break
    return scripts, runs_c, takes_next


def _parse_fish_args(args, i=0):
    """Parse a fish argument list (fish(1)). `-c`/`--command` and `-C`/`--init-command` each
    take a command that fish runs (a short one may be attached: `-cX`, `-icX`); `-d -D -o -f
    -p` and the long options above take a value too. Options end at the first non-option word,
    `--` or `-`; later words are a script file's or `$argv`.

    Fail closed: the list of value options may be incomplete, so after that parse every word
    from args[i] on is also read as an option (values not consumed) and any `-c`-style command
    it introduces is added. An unknown option's value, a script file's arguments, or a later
    word that happens to look like `-c` can therefore only add commands to check.

    Parsing starts at args[i]. Returns (mode, index of the first word after the options,
    scripts): `scripts` is a list of (index of the word holding it, the command text) for every
    command found. mode is 'c' when `-c` was given before the options end, else 'file' when a
    word is left, else 'stdin'. A long option the lists above do not know may take the next
    word as its value, so with one before that word the mode is 'stdin' (not 'file'): the word
    might not be a script file, and a piped script must still be checked."""
    n = len(args)
    first, command, unknown = i, False, False
    while i < n:
        token = args[i]
        if token in ("--", "-"):
            i += 1
            break
        if not (token.startswith("-") and len(token) > 1):
            break
        _scripts, runs_c, takes_next = _fish_option_at(args, i)
        command = command or runs_c
        if token.startswith("--") and len(token) > 2:
            name = token.partition("=")[0]
            unknown = unknown or not any(
                len(name) > 2 and opt.startswith(name) for opt in _FISH_LONG_VALUE + _FISH_LONG_FLAG)
        i += 2 if takes_next else 1
    i = min(i, n)
    scripts = []
    for k in range(first, n):
        for found in _fish_option_at(args, k)[0]:
            if found not in scripts:
                scripts.append(found)
    return ("c" if command else "file" if i < n and not unknown else "stdin"), i, scripts


def _shell_parse(prog, args, i=0):
    """(mode, start, scripts) for shell `prog`'s arguments from args[i]: the mode and option
    end of `_parse_shell_args` (or `_parse_fish_args`), and the (index, text) of every command
    its options give it."""
    if prog == "fish":
        return _parse_fish_args(args, i)
    mode, start = _parse_shell_args(args, i)
    return mode, start, ([(start, args[start])] if mode == "c" else [])


def _embedded_shells(words):
    """(mode, start) for each shell program among `words` (`docker run img bash -euo pipefail
    -c X`). A shell's own options are skipped, not rescanned."""
    k, n = 0, len(words)
    while k < n:
        prog = _prog_name(words[k])
        if prog in _SHELLS:
            mode, start, _scripts = _shell_parse(prog, words, k + 1)
            yield mode, start
            k = max(start, k + 1)
        else:
            k += 1


def _script_starts(words):
    """(index of the word an embedded script starts in, the script's first word) for each one in
    `words`: after a shell program's options (`sh -euo pipefail -c X`, `fish --command=X`), or
    after a `-c` flag of another program (`npx -c X`). The first word differs from
    words[index] only for a value attached to its option (`--command=X`)."""
    starts, k, n = [], 0, len(words)
    while k < n - 1:
        word = words[k]
        prog = _prog_name(word)
        if prog in _SHELLS:
            _mode, start, scripts = _shell_parse(prog, words, k + 1)
            starts.extend(scripts)
            k = max(start, k + 1)
            continue
        if _SHELL_C_FLAG.match(word):
            starts.append((k + 1, words[k + 1]))
        k += 1
    return starts


def _rtk_command(toks):
    """`rtk ...` -> the command it runs. `bypass`/`proxy`/`run`/`err`/`test`/`summary` run a
    command; any other subcommand wraps a fixed tool (`rtk npx ...`, `rtk git ...`) and the
    words after the global flags stand.

    `rtk run -c|--command <script> [--shell X]` and `rtk err|test|summary --shell X <script>`
    become `X -c <script>` (X defaults to `sh`), so a shell recurses and an interpreter gets
    the inline-code scan. A lone argument holding whitespace is also run as a script."""
    i = _skip_flags(toks, 1, frozenset())
    sub = toks[i] if i < len(toks) else ""
    if sub in _RTK_PASSTHROUGH:
        return toks[_skip_flags(toks, i + 1, frozenset()):]
    if sub not in _RTK_RUNNERS:
        return toks[i:]
    shell = script = None
    i, n = i + 1, len(toks)
    while i < n:
        token = toks[i]
        value = toks[i + 1] if i + 1 < n else None
        if token == "--":
            i += 1
            break
        if token == "--shell":
            shell, i = value, i + 2
        elif token.startswith("--shell="):
            shell, i = token[len("--shell="):], i + 1
        elif sub == "run" and token == "--command":
            script, i = value, i + 2
        elif sub == "run" and token.startswith("--command="):
            script, i = token[len("--command="):], i + 1
        elif sub == "run" and token[:1] == "-" and token[1:2] not in ("", "-") and "c" in token:
            attached = token[token.index("c") + 1:]       # `-c X`, `-vc X`, `-cX`
            script, i = (attached, i + 1) if attached else (value, i + 2)
        elif token.startswith("-") and len(token) > 1:
            i += 1                                        # `--skip-env`, `-v`, ...
        else:
            break
    rest = toks[i:]
    if sub == "run" and script is not None:
        return [shell or "sh", "-c", script]
    if sub != "run" and shell and rest:
        return [shell, "-c", " ".join(rest)]
    if len(rest) == 1 and len(rest[0].split()) > 1:
        return ["sh", "-c", rest[0]]                      # `rtk err "npx playwright install"`
    return rest


_ENV_LONG_VALUES = ("--split-string", "--unset", "--chdir", "--argv0")


def _env_split_string(toks):
    """For `env ...` in toks: (index of the word holding -S/--split-string, index after its
    value, the value), or None. Only env's own options count: they end at the first word that
    is not an option (an assignment or the program) or at `--`, so a `-S` after the program
    belongs to the program. The value may be attached (`-S'x y'`, `-iSx`, `--split-string=x`),
    and a long option may be shortened as getopt allows (`--split=x`)."""
    i, n = 1, len(toks)
    while i < n:
        token = toks[i]
        if token in ("--", "-") or not token.startswith("-"):
            return None
        if token.startswith("--"):
            name, eq, value = token.partition("=")
            full = next((o for o in _ENV_LONG_VALUES if len(name) > 2 and o.startswith(name)), "")
            if full == "--split-string":
                if eq:
                    return i, i + 1, value
                return (i, i + 2, toks[i + 1]) if i + 1 < n else None
            i += 2 if full and not eq else 1
            continue
        for k, letter in enumerate(token[1:], 1):          # `-i`, `-iu NAME`, `-uNAME`, `-vS x`
            if letter in "uCSa":
                attached = token[k + 1:]
                if letter == "S":
                    if attached:
                        return i, i + 1, attached
                    return (i, i + 2, toks[i + 1]) if i + 1 < n else None
                i += 1 if attached else 2
                break
        else:
            i += 1
    return None


def _split_words(text):
    """Words of an `env -S` string: POSIX shell-like splitting (quotes, backslashes); on an
    unbalanced quote, plain whitespace splitting."""
    try:
        return shlex.split(text)
    except ValueError:
        return text.split()


def _option_scripts(toks, options, value_flags):
    """Command lines given by `options` (`-c X`, `--command X`, `--command=X`, a short cluster
    `-qc X` / `-cX`) in any of toks[1:]. Extra scripts only add checks, so this scans every
    word rather than guessing where the options end."""
    found, k, n = [], 1, len(toks)
    while k < n:
        token = toks[k]
        name, eq, value = token.partition("=")
        if token in options:
            if k + 1 < n:
                found.append(toks[k + 1])
            k += 1
        elif eq and name.startswith("--") and name in options:
            found.append(value)
        elif len(token) > 2 and token[0] == "-" and token[1] != "-":
            for pos, letter in enumerate(token[1:], 1):
                if "-" + letter in options:
                    attached = token[pos + 1:]
                    if attached:
                        found.append(attached)
                    elif k + 1 < n:
                        found.append(toks[k + 1])
                        k += 1
                    break
                if "-" + letter in value_flags:
                    break                                 # the rest is that option's value
        k += 1
    return found


# xargs short options that take a value (GNU and BSD); `e` and `l` take one only attached.
_XARGS_VALUE_LETTERS = "adEeLlnPsRS"


def _xargs_replace(options):
    """The string xargs replaces with each input line (`-I R`, `-IR`, `-i[R]`, `--replace[=R]`,
    BSD `-J R`), or None. `options` are the words between `xargs` and its command."""
    replace, k = None, 0
    while k < len(options):
        token = options[k]
        if token == "--replace":
            replace = "{}"
        elif token.startswith("--replace="):
            replace = token[len("--replace="):] or "{}"
        elif len(token) > 1 and token[0] == "-" and token[1] != "-":
            for pos, letter in enumerate(token[1:], 1):   # `-tI{}`, `-tI {}`, `-0i`, `-iX`
                attached = token[pos + 1:]
                if letter in "IJ":
                    if attached:
                        replace = attached
                    elif k + 1 < len(options):
                        replace, k = options[k + 1], k + 1
                    break
                if letter == "i":
                    replace = attached or "{}"
                    break
                if letter in _XARGS_VALUE_LETTERS:
                    k += 0 if attached or letter in "el" else 1   # `-e`/`-l`: attached only
                    break
        k += 1
    return replace


_SHELL_SYNTAX = re.compile(r"[\s'\"`$\\;&|<>()]|(?:\A|\0)#")
_PARALLEL_POSITION = re.compile(r"\{([0-9]+)\}")


def _needs_shell(words):
    """True when `" ".join(words)` would lex into other words than `words` (a word holds a
    blank, a quote or shell syntax, or starts a comment). Otherwise peeling the words sees all
    a `sh -c` of the joined line would run, and no script is added for it. One scan."""
    return bool(_SHELL_SYNTAX.search("\0".join(words)))


def _parallel_replace(options):
    """GNU parallel's replacement string: `-I R` (or `-IR`), `--replace=R`, else `{}`."""
    replace = "{}"
    for k, token in enumerate(options):
        if token == "-I" and k + 1 < len(options):
            replace = options[k + 1]
        elif token.startswith("-I") and len(token) > 2:
            replace = token[2:]
        elif token.startswith("--replace="):
            replace = token[len("--replace="):] or "{}"
    return replace


def _parallel_inputs(words):
    """The input sources after GNU parallel's command: one list per `:::`-style separator."""
    groups = []
    for word in words:
        if word in _PARALLEL_INPUTS:
            groups.append([])
        elif groups:
            groups[-1].append(word)
    return [group[:_MAX_FED] for group in groups if group]


def _parallel_lines(line, replace, groups):
    """The command lines GNU parallel runs: for each combination of one input per source (at
    most `_MAX_FED`), `{1}`, `{2}`, ... are put in place, else the replacement string, else the
    inputs are appended; with no command line the inputs are the command."""
    out = []
    for combo in itertools.islice(itertools.product(*groups), _MAX_FED) if groups else ():
        joined = " ".join(combo)
        if _PARALLEL_POSITION.search(line):
            out.append(_PARALLEL_POSITION.sub(
                lambda m: combo[int(m.group(1)) - 1] if 0 < int(m.group(1)) <= len(combo)
                else m.group(0), line))
        elif not line:
            out.append(joined)
        elif replace in line:
            out.append(line.replace(replace, joined))
        else:
            out.append(line + " " + joined)
    return out


def _unknown_option_before(toks, i):
    """True when toks[i - 1] is an option whose value-taking the tables do not know."""
    prev = toks[i - 1] if i > 0 else ""
    return (len(prev) > 1 and prev.startswith("-") and prev != "--" and "=" not in prev
            and prev not in _PM_VALUE_FLAGS)


def _pm_verb_index(toks):
    """Index of a package manager's subcommand in toks (`npm --prefix web exec ...` -> 3). An
    option the tables do not know may take a value, so a word after one that is not a known
    verb is read as that value (`npm --loglevel-x silent exec`); the alternate readings of
    `_alternates` cover the other choice (`yarn --frozen-lockfile playwright install`)."""
    i, n = _skip_flags(toks, 1, _PM_VALUE_FLAGS), len(toks)
    while i < n and toks[i] not in _PM_VERBS and _unknown_option_before(toks, i):
        i = _skip_flags(toks, i + 1, _PM_VALUE_FLAGS)
    return i if i < n else _skip_flags(toks, 1, _PM_VALUE_FLAGS)


def _pm_words(args):
    """Positional words of a package manager's arguments (the subcommand first), read like
    `_pm_verb_index`: before the subcommand, a non-verb word after an unknown option is that
    option's value (`npm --loglevel-x silent i puppeteer` -> ['i', 'puppeteer'])."""
    toks = [""] + list(args)
    i = _pm_verb_index(toks)
    head = _positional(toks[1:i], _PM_VALUE_FLAGS)
    head = [w for w in head if w in _PM_VERBS]            # skipped words were option values
    return head + _positional(toks[i:], _PM_VALUE_FLAGS)


# Programs that run their arguments on another machine or in a container: an alternate reading
# never starts after one (docker's arguments are exempt from G1 and G2; the others are not
# this host).
_ELSEWHERE = frozenset(("docker", "docker-compose", "podman", "nerdctl", "orb", "orbctl", "ssh",
                        "kubectl", "limactl", "lima", "multipass", "vagrant"))


def _alternate_span(tokens, main_at):
    """(start, end) of the words that may start an alternate reading, or None. `main_at` is
    the index in tokens of the program the main parse found. The first program word (after
    assignments and keywords) decides:
    - a wrapper, runner or package manager the peeling knows: every later word. For a package
      manager, the words after a bare `--` that follows the main program are a script's
      arguments and are left out (`npm run e2e -- x`, `npm test -- x`, `pnpm run e2e -- x`;
      not `npm exec -- playwright install`, where `--` comes before the program);
    - any other program, unless it only reads or prints text, runs elsewhere, is a shell or an
      interpreter, or passes `--` arguments on (`_DASHDASH_ARGUMENTS`): the words after its
      first bare `--` (`doppler run -- cmd`, `lerna exec -- cmd`)."""
    for k, word in enumerate(tokens):
        if _ASSIGNMENT.match(word) or word == "!" or word in _KEYWORDS or word == "":
            continue
        name = _prog_name(word)
        rest = tokens[k + 1:]
        if name in _ALTERNATE_TRIGGERS:
            after = tokens[main_at + 1:]
            if name in _JS_PACKAGE_MANAGERS and "--" in after:
                return k + 1, main_at + 1 + after.index("--")
            return k + 1, len(tokens)
        if (name in _QUIET_PROGRAMS or name in _ELSEWHERE or name in _SHELLS
                or name in _DASHDASH_ARGUMENTS or _INTERPRETER.match(name) or "--" not in rest):
            return None
        return k + 2 + rest.index("--"), len(tokens)
    return None


def _alternates(tokens, start, end):
    """Alternate readings of a wrapped command: each word in tokens[start:end] that names a
    program the peeling or G1 knows starts one (`npx --registry X playwright install` ->
    `playwright install`), up to the first word naming a program in `_ELSEWHERE`. They catch
    an option whose value the tables do not know, read by the main parse as the program."""
    out = []
    for k in range(start, end):
        word = _strip_version(tokens[k])
        name = _prog_name(word)
        if name in _ELSEWHERE:
            break
        if name in _ALTERNATE_STARTS or _PIP.match(name):
            out.append([word] + tokens[k + 1:k + _MAX_ALTERNATE_WORDS])
            if len(out) >= _MAX_ALTERNATES:
                break
    return out


# git config keys whose value git runs as a shell command (`alias.*` and credential helpers
# only with a leading `!`), and the environment variables that do the same.
_GIT_COMMAND_KEYS = frozenset((
    "core.pager", "core.sshcommand", "core.fsmonitor", "core.editor", "core.askpass",
    "sequence.editor", "diff.external", "interactive.difffilter"))
_GIT_COMMAND_KEY = re.compile(
    r"pager\.[^.]+\Z|(?:diff|merge|filter)\..+\.(?:textconv|command|driver|clean|smudge|process)\Z")
_COMMAND_VARIABLES = frozenset((
    "GIT_SSH_COMMAND", "GIT_SSH", "GIT_PAGER", "GIT_EDITOR", "GIT_SEQUENCE_EDITOR",
    "GIT_EXTERNAL_DIFF", "GIT_ASKPASS", "PAGER", "EDITOR", "VISUAL", "RSYNC_RSH"))
_GIT_CONFIG_KEY_VAR = re.compile(r"GIT_CONFIG_KEY_([0-9]+)\Z")
_GIT_VALUE_FLAGS = frozenset(("-c", "-C", "--git-dir", "--work-tree", "--namespace",
                              "--config-env", "--super-prefix", "--list-cmds"))
_GIT_CONFIG_VALUE_FLAGS = frozenset(("-f", "--file", "--blob", "--type", "--default",
                                     "--comment", "--value"))


def _git_command_value(key, value):
    """The shell command git config `key` = `value` runs, or None."""
    key = key.lower()
    if key.startswith("alias.") or (key.startswith("credential.") and key.endswith(".helper")) \
            or key == "credential.helper":
        return value[1:] if value.startswith("!") else None
    if key in _GIT_COMMAND_KEYS or _GIT_COMMAND_KEY.match(key):
        return value
    return None


def _assignment_scripts(words):
    """Commands that `NAME=value` words before a program hand to it: GIT_SSH_COMMAND, GIT_PAGER,
    EDITOR, RSYNC_RSH, ..., and command-valued keys in GIT_CONFIG_PARAMETERS or
    GIT_CONFIG_KEY_<n>/GIT_CONFIG_VALUE_<n>."""
    values = dict(word.split("=", 1) for word in words)
    out = []
    for name, value in values.items():
        if name in _COMMAND_VARIABLES:
            out.append(value)
        elif name == "GIT_CONFIG_PARAMETERS":
            for match in re.finditer(r"'([^']*)'(?:='([^']*)')?", value):
                key, _eq, val = (match.group(1), "=", match.group(2)) \
                    if match.group(2) is not None else match.group(1).partition("=")
                out.append(_git_command_value(key, val))
        else:
            match = _GIT_CONFIG_KEY_VAR.match(name)
            if match and "GIT_CONFIG_VALUE_" + match.group(1) in values:
                out.append(_git_command_value(value, values["GIT_CONFIG_VALUE_" + match.group(1)]))
    return [script for script in out if script]


def _git_scripts(args):
    """Commands a git invocation runs from its config: `-c key=value` anywhere, and the value
    `git config [options] [set] <key> <value>` stores for later."""
    out = []
    for k, arg in enumerate(args[:-1]):
        if arg == "-c":
            key, eq, value = args[k + 1].partition("=")
            out.append(_git_command_value(key, value) if eq else None)
    i = _skip_flags(args, 0, _GIT_VALUE_FLAGS)
    sub, rest = (args[i], args[i + 1:]) if i < len(args) else ("", [])
    if sub == "config":
        words = _positional(rest, _GIT_CONFIG_VALUE_FLAGS)
        if words[:1] == ["set"]:
            words = words[1:]
        if len(words) >= 2:
            out.append(_git_command_value(words[0], words[1]))
    elif sub == "bisect" and rest[:1] == ["run"]:
        out.append(shlex.join(rest[1:]))                  # `git bisect run <cmd> [args]`
    elif sub == "submodule":
        k = _skip_flags(rest, 0, frozenset())
        if rest[k:k + 1] == ["foreach"]:                  # `foreach [--recursive] <command>`
            out.append(" ".join(rest[_skip_flags(rest, k + 1, frozenset()):]))
    elif sub in ("rebase", "difftool"):                   # `rebase -x X`, `difftool -x X`
        long = "--exec" if sub == "rebase" else "--extcmd"
        for k, arg in enumerate(rest):
            following = rest[k + 1] if k + 1 < len(rest) else None
            if arg in ("-x", long):
                out.append(following)
            elif arg.startswith(long + "="):
                out.append(arg[len(long) + 1:])
            elif arg.startswith("-x") and len(arg) > 2:
                out.append(arg[2:])
    return [script for script in out if script]


_TAR_COMMAND_OPTIONS = ("--to-command", "--use-compress-program", "--info-script",
                        "--new-volume-script", "--rsh-command")


def _tar_scripts(args):
    """Commands GNU tar runs: `--to-command`, `-I`/`--use-compress-program`, `-F`/
    `--info-script`, `--new-volume-script`, `--rsh-command` (separate, attached or `=` value;
    a long name of more than four characters may be shortened, as getopt allows) and
    `--checkpoint-action=exec=`."""
    out = []
    for k, arg in enumerate(args):
        following = args[k + 1] if k + 1 < len(args) else None
        name, eq, value = arg.partition("=")
        if arg in ("-I", "-F"):
            out.append(following)
        elif arg.startswith(("-I", "-F")):                # `-I'zstd -19'`: attached value
            out.append(arg[2:])
        elif arg.startswith("--checkpoint-action=exec="):
            out.append(arg[len("--checkpoint-action=exec="):])
        elif name.startswith("--") and len(name) > 4:
            if any(option.startswith(name) for option in _TAR_COMMAND_OPTIONS):
                out.append(value if eq else following)
    return [script for script in out if script]


def _rsync_scripts(args):
    """The remote shell rsync runs: `-e X`, `-eX`, `-avze X`, `--rsh X`, `--rsh=X`."""
    out = []
    for k, arg in enumerate(args):
        following = args[k + 1] if k + 1 < len(args) else None
        if arg == "--rsh":
            out.append(following)
        elif arg.startswith("--rsh="):
            out.append(arg[len("--rsh="):])
        elif len(arg) > 1 and arg[0] == "-" and arg[1] != "-" and "e" in arg[1:]:
            attached = arg[arg.index("e", 1) + 1:]
            out.append(attached or following)
    return [script for script in out if script]


def _python_module(toks):
    """For `python ... -m mod args`: [mod, args...], or None. Python's options before `-m` may
    be clustered (`-Bm mod`, `-mmod`) and `-X`/`-W` take a value (`-X dev`, `-Wignore`); `-c`,
    a script, `-` or `--` end the search."""
    k, n = 1, len(toks)
    while k < n:
        token = toks[k]
        if token in ("-X", "-W"):
            k += 2
            continue
        if token in ("-", "--") or not token.startswith("-"):
            return None
        if token.startswith("--"):
            k += 1
            continue
        letters = token[1:]
        for pos, letter in enumerate(letters):
            if letter == "m":
                attached = letters[pos + 1:]
                if attached:
                    return [attached] + toks[k + 1:]
                return toks[k + 1:] if k + 1 < n else None
            if letter == "c":
                return None
            if letter in "XW":                            # the rest of the word, or the next
                k += 0 if letters[pos + 1:] else 1
                break
        k += 1
    return None


def _trap_script(args):
    """`trap [--] <command> <signal>...`: the command (a lone word, `-` or `''` resets or
    ignores)."""
    i = 0
    while i < len(args) and args[i].startswith("-") and args[i] != "-":
        i += 1
        if args[i - 1] == "--":
            break
    return args[i] if len(args) - i >= 2 and args[i] != "-" else None


def _strip_prefixes(tokens, alternates=None, stdin_runs=None, layer_scripts=True):
    """Peel assignments, keywords, wrappers and package runners off a command.

    Returns (command tokens starting at the program, embedded scripts to analyze). When given,
    `alternates` receives other token lists that may be the real command (`_alternates`), and
    `stdin_runs` receives (kind, template, replace string) for each xargs or parallel layer,
    whose command takes words or lines from stdin."""
    toks = list(tokens)
    scripts = []
    assigned = []
    # Each command-running construct is taken once per command: the first watch/sg/parallel
    # line, the first xargs/parallel stdin run, one option scan per program name. Later
    # layers are peeled only, and the words they would run are inside the first one's text,
    # so the work stays linear in the command's length (FW-I review C1). With `layer_scripts`
    # false (an alternate reading: a suffix of a command already parsed) no joined line is
    # added, since each suffix's line would be a new text of the same length.
    joined = not layer_scripts
    fed = False
    scanned = set()
    for _ in range(64):
        if not toks:
            break
        word = toks[0]
        name = _prog_name(word)
        if _ASSIGNMENT.match(word) or word == "!" or word in _KEYWORDS or word == "":
            if _ASSIGNMENT.match(word):
                assigned.append(word)
            toks = toks[1:]                               # `""` is a `$(...)` placeholder
        elif name == "env" and _env_split_string(toks) is not None:
            at, after, value = _env_split_string(toks)    # the split words are env's arguments
            toks = toks[:at] + _split_words(value) + toks[after:]
        elif name == "busybox" and (len(toks) < 2 or toks[1].startswith("-")):
            break                                         # `busybox --list`: no applet
        elif name in _WRAPPERS:
            flags = _WRAPPERS[name]
            if name in _WRAPPER_SCRIPT_OPTIONS and name not in scanned:
                scanned.add(name)
                scripts.extend(_option_scripts(toks, _WRAPPER_SCRIPT_OPTIONS[name], flags))
            i = _skip_flags(toks, 1, flags)
            if name in _WRAPPER_OPERANDS and i < len(toks):
                i = _skip_flags(toks, i + _WRAPPER_OPERANDS[name], flags)
            if name == "watch" and not joined:            # watch runs its words via sh -c
                joined = True
                if _needs_shell(toks[i:]):
                    scripts.append(" ".join(toks[i:]))
            if name == "entr" and i < len(toks) and any(
                    t[:1] == "-" and t[1:2] != "-" and "s" in t for t in toks[1:i]):
                scripts.append(toks[i])                   # `entr -s X` runs X via $SHELL
            if name == "xargs" and stdin_runs is not None and not fed:
                fed = True
                stdin_runs.append(("xargs", toks[i:], _xargs_replace(toks[1:i])))
            toks = toks[i:]
        elif name == "sg":                                # `sg [-] group [-c] command`: sh -c
            rest = toks[1:]
            rest = rest[2:] if rest[:1] == ["-"] else rest[1:]
            rest = rest[1:] if rest[:1] == ["-c"] else rest
            if not joined:
                joined = True
                if _needs_shell(rest):
                    scripts.append(" ".join(rest))
            toks = rest
        elif name == "parallel":
            i = _skip_flags(toks, 1, _PARALLEL_VALUE_FLAGS)
            replace = _parallel_replace(toks[1:i])
            rest = toks[i:]
            cut = next((k for k, t in enumerate(rest) if t in _PARALLEL_INPUTS), len(rest))
            template = rest[:cut]
            if not joined:                                # parallel runs its line via a shell
                joined = True
                line = " ".join(template)
                if _needs_shell(template):
                    scripts.append(line)
                scripts.extend(_parallel_lines(line, replace, _parallel_inputs(rest[cut:])))
            if cut == len(rest) and stdin_runs is not None and not fed:
                fed = True
                stdin_runs.append(("parallel", template, replace))
            toks = template
        elif name == "gdb":                               # `gdb [options] --args prog args`
            if "--args" not in toks:
                break
            toks = toks[toks.index("--args") + 1:]
        elif name == "nix" and toks[1:2] in (["develop"], ["shell"]):
            at = next((k for k, t in enumerate(toks) if t in ("-c", "--command")), -1)
            if at < 0:
                break
            toks = toks[at + 1:]                          # `nix develop -c cmd args`
        elif name in _SCRIPT_ARGUMENTS:
            k, flags = 1, _SCRIPT_ARGUMENTS[name]
            while k < len(toks):
                if toks[k] in flags:
                    k += 1
                elif not toks[k].startswith("-"):
                    scripts.append(toks[k])
                k += 1
            break
        elif name in _WRAPPER_SCRIPT_OPTIONS:             # `nodemon --exec X`
            scripts.extend(_option_scripts(toks, _WRAPPER_SCRIPT_OPTIONS[name], frozenset()))
            break
        elif name in _SUBCOMMAND_WRAPPERS and len(toks) > 1 \
                and toks[1] in _SUBCOMMAND_WRAPPERS[name]:
            rest = toks[3:] if name == "direnv" else toks[2:]   # `direnv exec <dir> cmd`
            toks = rest[rest.index("--") + 1:] if "--" in rest else rest[_skip_flags(rest, 0, ()):]
        elif name == "command":
            if len(toks) > 1 and toks[1] in ("-v", "-V"):
                return ["which"] + toks[2:], scripts      # lookup, not execution
            toks = toks[_skip_flags(toks, 1, frozenset()):]
        elif name == "rtk":
            toks = _rtk_command(toks)
        elif name in _RUNNERS:
            toks = _runner_program(toks, 1, scripts)
        elif name in _PYTHON_RUNNERS and _python_runner_start(name, toks) > 0:
            toks = _runner_program(toks, _python_runner_start(name, toks), scripts)
        elif name in _JS_PACKAGE_MANAGERS and len(toks) > 1:
            i = _pm_verb_index(toks)
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
            module = _python_module(toks)
            if module is None:
                break
            toks = module                                 # `python -m pip ...` runs `pip`
        else:
            break
    scripts.extend(_assignment_scripts(assigned))
    if toks:
        name = _prog_name(toks[0])
        if name in _SHELLS:
            scripts.extend(script for _index, script in _shell_parse(name, toks[1:])[2])
        elif name == "eval":
            scripts.append(" ".join(toks[1:]))
        elif name == "trap" and _trap_script(toks[1:]):
            scripts.append(_trap_script(toks[1:]))        # runs at the signal or at exit
        elif name == "git":
            scripts.extend(_git_scripts(toks[1:]))
        elif name in ("tar", "gtar", "bsdtar"):
            scripts.extend(_tar_scripts(toks[1:]))
        elif name == "rsync":
            scripts.extend(_rsync_scripts(toks[1:]))
    main = _prog_name(toks[0]) if toks else ""
    main_at = max(len(tokens) - len(toks), 0)   # exact when the first program is a package manager
    span = _alternate_span(tokens, main_at) if alternates is not None else None
    if span is not None and main not in _QUIET_PROGRAMS:
        alternates.extend(_alternates(list(tokens), *span))
    return toks, scripts


# ------------------------------------------------------------- entries

class _Entry:
    """A flattened command ready for the per-rule predicates."""
    __slots__ = ("raw", "redirs", "prog", "prog_path", "args", "cwd", "exempt", "group", "code",
                 "stdin_runs")

    def __init__(self, raw, redirs, cmd, cwd, exempt, group):
        self.raw = raw                                    # every word, wrappers included
        self.redirs = redirs
        self.prog_path = cmd[0].lower() if cmd else ""
        self.prog = _prog_name(cmd[0]) if cmd else ""
        self.args = cmd[1:]
        self.cwd = cwd                                    # tracked `cd` directory, normalized
        self.exempt = exempt                              # docker: G1 and G2 do not apply
        self.group = group
        # inline code an interpreter runs: its arguments, plus text fed to its stdin
        self.code = list(self.args) if _INTERPRETER.match(self.prog) else []
        self.stdin_runs = []                              # xargs / parallel layers (_strip_prefixes)


class _Context:
    def __init__(self, home):
        self.home = home
        self.cwd = None
        self.entries = []
        # (text, exempt, cwd at the start) -> (depth, cwd at the end) for every text analyzed:
        # the same text from the same state gives the same entries, so it is analyzed once.
        self.seen = {}


_CHDIR_PROGRAMS = frozenset(("tar", "git", "make"))
# A shell's script file that is its stdin, or a numbered file descriptor.
_STDIN_PATH = re.compile(r"/dev/stdin\Z|/dev/fd/0\Z|/proc/self/fd/0\Z")
_FD_PATH = re.compile(r"/dev/fd/[0-9]+\Z|/proc/self/fd/[0-9]+\Z")


def _first_arg(args):
    return next((a for a in args if not a.startswith("-")), None)


def _shell_script_file(entry):
    """The script file a shell entry runs, or None (command mode, stdin)."""
    mode, start, _scripts = _shell_parse(entry.prog, entry.args)
    return entry.args[start] if mode == "file" else None


def _reads_stdin_as_shell(entry):
    """True for a shell fed from stdin (`bash`, `sh -s`, `sh /dev/stdin`, `source /dev/stdin`)
    or a docker command that runs one."""
    if entry.prog in _SHELLS:
        if _shell_parse(entry.prog, entry.args)[0] == "stdin":
            return True
        return bool(_STDIN_PATH.match(_shell_script_file(entry) or ""))
    if entry.prog in ("source", "."):
        return bool(_STDIN_PATH.match(_first_arg(entry.args) or ""))
    if entry.prog in _DOCKER:
        return any(mode == "stdin" for mode, _start in _embedded_shells(entry.args))
    return False


def _reads_fd_as_shell(entry):
    """True for a shell (or `source`) whose script is a file descriptor: `sh <&3`,
    `sh /dev/fd/3`, `source /dev/fd/3`."""
    if entry.prog in ("source", "."):
        return bool(_FD_PATH.match(_first_arg(entry.args) or ""))
    if entry.prog not in _SHELLS:
        return False
    script = _shell_script_file(entry)
    if script is not None:
        return bool(_FD_PATH.match(script))
    return _shell_parse(entry.prog, entry.args)[0] == "stdin" and any(
        op == "<&" for op, _target in entry.redirs)


def _add_command(tokens, redirs, group, ctx, exempt, depth):
    alternates, stdin_runs = [], []
    cmd, scripts = _strip_prefixes(tokens, alternates, stdin_runs)
    entry = _Entry(tokens, redirs, cmd, ctx.cwd, exempt, group)
    entry.exempt = exempt or entry.prog in _DOCKER
    entry.stdin_runs = stdin_runs
    if entry.prog in _CHDIR_PROGRAMS:                     # `tar -C dir x`, `git -C dir ...`
        for k, arg in enumerate(entry.args[:-1]):
            if arg in ("-C", "--directory"):
                entry.cwd = _normalize(entry.args[k + 1], ctx.home, ctx.cwd) or entry.cwd
    ctx.entries.append(entry)
    _add_alternates(alternates, scripts, entry, ctx, exempt)
    for script in scripts:
        _collect(script, ctx, exempt, depth + 1)
    if entry.prog in _DOCKER:                             # `docker run img sh -euc "..."`
        for _start, script in _script_starts(entry.args):
            _collect(script, ctx, True, depth + 1)
    elif entry.prog == "find":
        _collect_find_exec(entry, ctx, depth)
    elif entry.prog in ("fd", "fdfind"):
        _collect_fd_exec(entry, ctx, depth)
    elif entry.prog in ("cd", "pushd"):
        target = _first_arg(entry.args)
        if target is None:
            ctx.cwd = _normalize("~", ctx.home, None)
        elif target != "-":
            new_cwd = _normalize(target, ctx.home, ctx.cwd)
            if new_cwd.count("/") <= _MAX_CWD_DEPTH:      # keeps absurd `cd` chains linear
                ctx.cwd = new_cwd
    return entry


def _add_alternates(alternates, scripts, entry, ctx, exempt):
    """Check each alternate reading of a wrapped command (`_alternates`) against G1 only: the
    first that installs browser tooling becomes an entry. The scripts an alternate gives
    (`sh -c X` after an option the tables do not know) join `scripts`, once each."""
    for alternate in alternates:
        cmd, more = _strip_prefixes(alternate, layer_scripts=False)
        other = _Entry(entry.raw, [], cmd, entry.cwd, exempt, entry.group)
        other.exempt = exempt or other.prog in _DOCKER
        scripts.extend(script for script in more if script not in scripts)
        if _g1(other):
            ctx.entries.append(other)
            return


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


def _collect_fd_exec(entry, ctx, depth):
    """`fd ... -x|--exec|-X|--exec-batch <command> [;]`: the command runs once per match. fd
    takes one such option, so only the first counts (a later `-x` is part of its command)."""
    args = entry.args
    k = next((k for k, a in enumerate(args) if a in ("-x", "--exec", "-X", "--exec-batch")), -1)
    if k >= 0:
        end = next((j for j in range(k + 1, len(args)) if args[j] == ";"), len(args))
        if end > k + 1:
            _add_command(args[k + 1:end], [], entry.group, ctx, entry.exempt, depth + 1)


def _echo_args_text(args):
    args = list(args)
    while args and re.match(r"-[neE]+\Z", args[0]):
        args.pop(0)
    return " ".join(args).replace("\\n", "\n")


def _echo_text(entry):
    return _echo_args_text(entry.args)


def _process_substitutions(text):
    """The inner text of each `<(...)` outside quotes. A cheap scan beside the lexer (which
    lexes the inner commands themselves); it only looks for what may feed a shell."""
    found, j, n = [], 0, len(text)
    while j < n:
        c = text[j]
        if c == "\\":
            j += 2
        elif c == "'":
            k = text.find("'", j + 1)
            j = n if k < 0 else k + 1
        elif c == '"':
            j += 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            j += 1
        elif c == "<" and text[j + 1:j + 2] == "(":
            inner, j = _scan_paren(text, j + 2)
            found.append(inner)
        else:
            j += 1
    return found


def _literal_output(text, depth):
    """Literal text the commands in `text` print: echo/printf arguments, here-strings and
    heredoc bodies."""
    texts, groups = [], set()
    for seg in _segments(text, depth + 1):
        cmd = _strip_prefixes(seg.tokens)[0]
        if cmd and _prog_name(cmd[0]) in ("echo", "printf"):
            texts.append(_echo_args_text(cmd[1:]))
        if id(seg.group) not in groups:
            groups.add(id(seg.group))
            texts.extend(seg.group.texts)
    return texts


def _feed_stdin(entry, texts, ctx, depth):
    """Literal stdin text becomes commands for an xargs or parallel layer of `entry`: xargs
    appends its words to the command (or puts each line in place of its -I string); parallel
    runs each line, or its command with the line put in place of its replacement string or
    appended. A command has at most one such layer fed (`_strip_prefixes`)."""
    lines = [line.strip() for text in texts for line in text.splitlines() if line.strip()]
    lines = lines[:_MAX_FED]
    for kind, template, replace in entry.stdin_runs if lines else ():
        if kind == "parallel":
            for line in _parallel_lines(" ".join(template), replace, [lines]):
                _collect(line, ctx, entry.exempt, depth + 1)
        elif replace is not None:
            for line in lines:
                _add_command([t.replace(replace, line) for t in template], [], entry.group,
                             ctx, entry.exempt, depth + 1)
        elif template:
            words = [word for line in lines for word in line.split()]
            _add_command(template + words, [], entry.group, ctx, entry.exempt, depth + 1)


def _collect(text, ctx, exempt, depth):
    if depth > _MAX_DEPTH or not text:
        return
    key = (text, exempt, ctx.cwd)
    if key in ctx.seen and ctx.seen[key][0] <= depth:
        ctx.cwd = ctx.seen[key][1]                        # analyzed already: same entries, same cwd
        return
    ctx.seen[key] = (depth, ctx.cwd)
    _collect_once(text, ctx, exempt, depth)
    ctx.seen[key] = (depth, ctx.cwd)


def _collect_once(text, ctx, exempt, depth):
    new_entries = []
    for seg in _segments(text):
        new_entries.append(_add_command(seg.tokens, seg.redirs, seg.group, ctx, exempt, depth))
    # Literal text that reaches a shell's stdin is a command: heredocs, here-strings, `echo | sh`.
    by_group = {}
    for entry in new_entries:
        by_group.setdefault(id(entry.group), []).append(entry)
    # Text that reaches an interpreter's stdin is code: `python3 - <<EOF`, `echo ... | node`.
    # Text that reaches xargs or parallel becomes the words or lines of their command.
    for entries in by_group.values():
        shell = next((e for e in entries if _reads_stdin_as_shell(e)), None)
        interpreters = [e for e in entries if _INTERPRETER.match(e.prog)]
        feeders = [e for e in entries if e.stdin_runs]
        if shell is None and not interpreters and not feeders:
            continue
        texts = list(entries[0].group.texts)
        texts += [_echo_text(e) for e in entries if e.prog in ("echo", "printf")]
        for entry in interpreters:
            entry.code.extend(texts)
        for body in texts if shell is not None else ():
            _collect(body, ctx, shell.exempt, depth + 1)
        for entry in feeders:
            _feed_stdin(entry, texts, ctx, depth)
    # A shell or `source` given a process substitution runs what it prints: `sh <(echo X)`,
    # `cat <(echo X) | sh`. The lexer keeps no link between a `<(...)` and its command, so any
    # such consumer in the text makes the literal output of every `<(...)` in it a command.
    if "<(" in text and any(e.prog in _SHELLS or e.prog in ("source", ".") for e in new_entries):
        for inner in _process_substitutions(text):
            for body in _literal_output(inner, depth):
                _collect(body, ctx, exempt, depth + 1)
    # A shell reading a file descriptor (`sh <&3`, `sh /dev/fd/3`) runs the literal text that a
    # bare `exec` in the same text opened (`exec 3<<<'X'`, `exec 3<<EOF`), or that a
    # redirection on the shell itself opened (`bash /dev/fd/3 3<<<'X'`).
    fd_shells = {id(e) for e in new_entries if _reads_fd_as_shell(e)}
    for entry in new_entries if fd_shells else ():
        if id(entry) in fd_shells or (
                not entry.prog and entry.raw and _prog_name(entry.raw[0]) == "exec"):
            for body in entry.group.texts:
                _collect(body, ctx, exempt, depth + 1)


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
        try:
            return any(fnmatch.fnmatchcase(protected + v, candidate) for v in variants)
        except re.error:                                  # `[z-a]`: matches nothing
            return False
    return candidate.startswith(protected) if prefix else candidate == protected


_WILDCARD_ONLY = re.compile(r"[*?]+\Z")


def _is_home_dir(comps, home_comps):
    """True when the components name a home directory: `home`'s, `/home/<u>`, `<root>/<u>`."""
    return (bool(home_comps) and comps == home_comps) or (
        len(comps) == 2 and comps[0] in ("home", "users")) or comps == ["root"]


def _contains_sequence(path, sequences, prefix_flags=None, home_comps=(), anywhere=False):
    """True when `path` holds one of the component sequences: anywhere if absolute (or if
    `anywhere`), from the start (after any `..` or `$var` components) if relative.

    A window of nothing but wildcards (`**/*`, `*/*`) names no particular path and does not
    match, unless it starts directly under a home directory (`~/*/*/*` reaches `~/.config/...`)."""
    comps = [c for c in path.split("/") if c and c != "."]
    absolute = path.startswith("/")
    leading = 0
    if not absolute:
        while leading < len(comps) and (comps[leading] == ".." or "$" in comps[leading]):
            leading += 1
    last_start = len(comps) if absolute or anywhere else leading
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


_ENCODED_FILE_URL = re.compile(r"file:.*%[0-9a-f]{2}", re.IGNORECASE | re.DOTALL)


def _path_candidates(token):
    """The token itself, the part after the first `=`, and each `:`-separated piece. A part
    that is a percent-encoded `file:` URL also yields its decoded form (other URLs are not
    decoded: a search for `Library%2FApplication%20Support%2FGoogle` is not a path)."""
    parts = [token]
    if "=" in token:
        parts.append(token.split("=", 1)[1])
    parts += [urllib.parse.unquote(p) for p in parts if _ENCODED_FILE_URL.match(p)]
    pieces = list(parts)
    for part in parts:
        if ":" in part:
            pieces.extend(part.split(":"))
    return [p for p in pieces if p]


_PROFILE_SEQUENCES = tuple(seq for seq, _ in _PROFILE_COMPONENTS)
_PROFILE_PREFIX_FLAGS = tuple(flag for _, flag in _PROFILE_COMPONENTS)


def _is_profile_path(candidate, home, cwd, anywhere=False):
    home_comps = [c for c in _normalize(home, home, None).split("/") if c] if home else []
    for base in (None, cwd) if cwd else (None,):
        path = _normalize(candidate, home, base)
        if path and _contains_sequence(
                path, _PROFILE_SEQUENCES, _PROFILE_PREFIX_FLAGS, home_comps, anywhere):
            return True
    return False


def _is_profile_path_in_code(candidate, home, cwd):
    """A code literal carries prefixes (`{home}/`, `${process.env.HOME}/`), so a relative
    candidate matches the root sequence anywhere, not only from its first component."""
    return _is_profile_path(candidate, home, cwd, anywhere=True)


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


_ESCAPED_SPACE = re.compile(r"(?<!\\)\\+ ")
_CODE_LITERAL = re.compile("'([^']*)'|\"([^\"]*)\"|`([^`]*)`")
_CODE_PUNCTUATION = "()[]{}<>,;'\"`"
# Pieces between quote characters and newlines: a stray `'` in a comment (`# Chrome's ...`)
# can pair with a literal's opening quote, so the literals alone are not enough.
_CODE_QUOTE_SPLIT = re.compile("['\"`\n]")
# A code candidate can name a profile root only if it has a separator and a root-like word
# or a glob; the rest is dropped before path matching (keeps huge programs cheap).
_CODE_ROOT_HINT = re.compile(r"librar|config|chrom|google|[*?\[]", re.IGNORECASE)


def _code_candidates(text):
    """Path candidates in a piece of code: the contents of every '...', "..." and `...`
    literal (no nesting; triple quotes fall out naturally) and every whitespace-separated
    word with surrounding punctuation stripped. An escaped space is a space however many
    backslashes escape it: one in the shell, two inside a Python/JS/AppleScript string.
    Names assembled piecewise (`Path.home() / "Library" / ...`) are not seen (R1)."""
    text = _ESCAPED_SPACE.sub(" ", text)
    found = {literal.strip() for match in _CODE_LITERAL.finditer(text)
             for literal in match.groups() if literal}
    found.update(piece.strip() for piece in _CODE_QUOTE_SPLIT.split(text))
    found.update(word.strip(_CODE_PUNCTUATION) for word in text.split())
    found.discard("")
    return found


def _may_name_profile(candidate):
    """Cheap prefilter: a `/` (or an encoded `%2F`, or a backslash) and a root-like word."""
    lowered = candidate.lower()
    return ("/" in candidate or "\\" in candidate or "%2f" in lowered) and bool(
        _CODE_ROOT_HINT.search(candidate))


def _code_hits_profile(texts, home, cwd):
    """True when a profile path is spelled inside any of the code texts (G3)."""
    candidates = set()
    for text in texts:
        candidates |= _code_candidates(text)
    return _words_hit([c for c in candidates if _may_name_profile(c)],
                      _is_profile_path_in_code, home, cwd)


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
    words = _pm_words(entry.args)
    if words and words[0] == "global":
        words = words[1:]
    if not words or words[0] not in _JS_INSTALL_VERBS:
        return False
    return any(_strip_version(w).lower() == "puppeteer" for w in words[1:])


def _js_scaffolds_playwright(entry):
    """`npm init playwright`, `yarn create playwright`: downloads browsers unless told not to."""
    words = _pm_words(entry.args)
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


_APT_VALUE_FLAGS = frozenset(("-o", "--option", "-c", "--config-file", "-t", "--target-release",
                              "--default-release"))
_DNF_VALUE_FLAGS = frozenset(("-c", "--config", "--setopt", "--releasever", "--installroot",
                              "-x", "--exclude", "--repo", "--repoid", "--enablerepo",
                              "--disablerepo", "-d", "--debuglevel", "-e", "--errorlevel"))
_OS_INSTALLER_VALUE_FLAGS = {"apt": _APT_VALUE_FLAGS, "apt-get": _APT_VALUE_FLAGS,
                             "dnf": _DNF_VALUE_FLAGS, "yum": _DNF_VALUE_FLAGS,
                             "snap": frozenset(("--channel", "--revision"))}


def _os_installs_browser(entry):
    words = _positional(entry.args, _OS_INSTALLER_VALUE_FLAGS.get(entry.prog, frozenset()))
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
    return (_words_hit(words, _is_profile_path, home, entry.cwd)
            or _code_hits_profile(entry.code, home, entry.cwd))


# G4 ------------------------------------------------------------------

_G4_EXEMPT_PROGRAMS = frozenset((
    "grep", "egrep", "fgrep", "rg", "ag", "ack", "git", "echo", "printf", "which", "type",
    "ls", "cat", "head", "tail", "less", "wc", "file", "stat", "rm",
    # also only name paths and cannot run them (`find -exec` is analyzed as its own command)
    "cd", "pushd", "mkdir", "touch", "cp", "mv", "find", "du", "diff", "tree", "realpath",
    "basename", "dirname", "readlink", "chmod", "chown", "ln"))
_UNINSTALL_VERBS = frozenset(("uninstall", "remove", "rm", "un", "r"))
_INSPECT_VERBS = frozenset(("ls", "list", "view", "info", "why", "outdated"))

# Alternate readings (`_alternates`, used by `_strip_prefixes`; defined here, after the tables
# they combine). A command gets them when its first program is one the peeling knows; a word
# starts one when it names such a program or one G1 knows. None are made when the program the
# main parse found only reads or prints text (`sudo echo playwright install`).
_ALTERNATE_TRIGGERS = frozenset(
    set(_WRAPPERS) | set(_SUBCOMMAND_WRAPPERS) | set(_SCRIPT_ARGUMENTS) | set(_PYTHON_RUNNERS)
    | _RUNNERS | _JS_PACKAGE_MANAGERS | {"command", "rtk", "parallel", "sg", "nodemon", "pipx"})
_ALTERNATE_STARTS = frozenset(
    _ALTERNATE_TRIGGERS | _PLAYWRIGHT | _PUPPETEER | _OS_INSTALLERS | _SHELLS
    | {"create-playwright", "uv", "eval"})
_QUIET_PROGRAMS = _SEARCH_PROGRAMS | _G4_EXEMPT_PROGRAMS


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

# What is not a secret value after `key:`, `key=` or `"key":` (shared with memory_privacy; a test keeps
# the two copies identical): a value that is wholly null/none/true/false or a template placeholder
# (`<...>`, `${...}`, a run of `*`, `.` or `x`), optionally quoted (a quoted one needs its closing
# quote right after it), then optional blanks and a `,`, `}`, `]`, a line break or the end. Quotes
# may carry a backslash (JSON inside a double-quoted shell string).
_SKIP_WORD = r"(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)"
_NOT_A_VALUE = r"(?:\\?[\"']" + _SKIP_WORD + r"\\?[\"']|" + _SKIP_WORD + r")[ \t]*(?:[,}\]\r\n]|$)"
# After the separator of an unquoted key: not a placeholder.
_UNQUOTED_VALUE = r"(?![ \t]*" + _NOT_A_VALUE + ")"
# After a key's closing quote (one optional quote, `\"` included): a separator, then a real scalar,
# not empty, an object or array (a schema) or a placeholder.
_QUOTED_VALUE = r"\\?[\"']\s*[:=]\s*(?!" + _NOT_A_VALUE + r")(?:\\?[\"'])?(?![\\\"'{\[\]},\s])"

_SECRET_PATTERNS = tuple(re.compile(p) for p in (
    r"(?:AKIA|ASIA)[0-9A-Z]{16}",
    r"\bgh[pousr]_[A-Za-z0-9]+",
    r"github_pat_",
    r"\bsk-[A-Za-z0-9_-]{16,}",
    r"\bxox[baprs]-",
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
)) + tuple(re.compile(p, re.IGNORECASE) for p in (
    r"password(?:\s*[:=]" + _UNQUOTED_VALUE + "|" + _QUOTED_VALUE + r"\S)",
    r"api[_-]?key(?:\s*[:=]" + _UNQUOTED_VALUE + "|" + _QUOTED_VALUE + r"\S)",
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


_CODE_KEYS = frozenset(("script", "code", "expression", "func", "fn"))


def _code_strings(value, in_code_key=False, depth=0):
    """Strings under keys named `script`, `code`, `expression`, `func` or `fn` (at any depth):
    code a tool runs, such as BrowserOS Neo `run {code}`, `evaluate_in_page {code|func}` or
    `save_skill {flow[].fn, patch[].fn}`."""
    if depth > _MAX_WALK_DEPTH:
        return
    if isinstance(value, str):
        if in_code_key:
            yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            code_key = in_code_key or (isinstance(key, str) and key.lower() in _CODE_KEYS)
            yield from _code_strings(item, code_key, depth + 1)
    elif isinstance(value, (list, tuple)):
        for item in value:
            yield from _code_strings(item, in_code_key, depth + 1)


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
    if _words_hit(paths, _is_profile_path, home, None) or _code_hits_profile(
            _code_strings(inp), home, None):
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
