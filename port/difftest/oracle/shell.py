"""SH oracle for the `shell.segments` and `shell.analyze` difftest surfaces (plan SH1, SH2; design §7.1, §13.2).

    python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl > results.jsonl

Run as `python3 -I` (no script folder on `sys.path`): `json.py` beside this file shadows the standard
library's `json`. F4's driver loads this module by file path.

Library surface: `run(case: dict) -> dict` (the case's `data`, or a dict holding it under "data").

Case data. The reference is `guard_rules_fw.py` beside this file: guard_rules.py with fix-waves H and I
merged, vendored read-only (its header names both commits; controller ruling Q1=A). The owner moved those
fix-waves to Rust, so the plugin's own guard_rules.py is no longer the shell oracle.
- `{"op": "segments", "text": T, "depth": D}`: `_segments(T, D)`, reported with `lenient` (the top-level
  `_Lexer` raised `_Unbalanced`, so `_lenient_segments` produced the segments) and `incomplete` (false).
- `{"op": "shell_parse", "prog": P, "args": [..], "i": I}`: `_shell_parse(P, args, I)`.
- `{"op": "script_starts", "words": [..]}`: `_script_starts(words)`.
- `{"op": "analyze", "command": C, "home": H}`: `_analyze(C, H)` as `check` runs it: the bash 5 reading,
  then the bash 3.2 and wide readings when a heredoc line inside `$(...)` made them matter (`_check`), every
  pass's entries in order. Every `_Entry` field (`Entry` minus `opaque`), and `alternates`: each alternate
  reading `_add_alternates` looks at whose command differs from its entry's, as an entry (Rust keeps them all
  for G1, lead ruling Q3=A; the reference checks G1 as it goes, which this oracle replaces by recording).

Rust is stricter than the reference where a reviewer's bash-verified payload needs it (owner direction, D-8).
Where such a rule reaches across the grammar, so that no case list could name the cases, this oracle applies
it to the reference too, so the difftest still checks everything else; each such rule is listed in
`port/difftest/divergences/shell.*.toml` and pinned by Rust tests:
- FW-H review R1-M1: the lenient split also reads each closed `$'...'` decoded (`_lenient_segments` below).
- FW-I review R1-I1: a package manager's `--` ends the alternate span unless its verb is `exec`/`dlx`/`x`
  (`_alternate_span` below).
- SH-FW review C-3: a command's brace expressions give one more alternate reading with them expanded, at
  most 4096 words and 64 levels, only for a segment with an unquoted `{` (re-review F-1: `_add`, `_end_seg`
  and `_Seg` below), and too complex when it changes the program word (re-review C-3a,
  `_add_alternates_braces`); C-4: echo/printf text fed to a shell or to xargs/parallel is also fed decoded,
  not to an interpreter's code (`_collect_once`, `_literal_output` below), and (re-review C-4a) printf's
  words without a leading `--` and its format filled (`_echo_decoded`, `_printf_fill`).
- SH-FW review I-2: a newline (heredoc bodies, comments, blank lines) right after a `|`/`|&` keeps the
  pipeline (`_end_seg` below, part C).
- SH-FW re-review I-2a: a pipeline headed by a compound command or a function call (`( echo X ) | sh`,
  `fi | sh`, `f | sh`) is fed the literal output of the commands before it (`_collect_once`).
The narrow ones are declared per case (D-8, `fw:` cases): every `fd -x` group (R1-M2), and `doppler`/`op`/
`dotenvx`/`infisical run` (R1-M3).

Both ops run under `check`'s work budget (`_WORK_PER_CHAR` per character plus `_WORK_BASE`); over it the
reference raises `_TooComplex` and `check` blocks. The expected result is then Rust's incomplete one (`incomplete`
true, nothing else), marked D-7: Rust spends the same budget where `_charge` does, so it must stop too.

Groups are identified by object identity in Python; a result numbers them by first appearance (segments in
order, entries in order) and lists each group's texts in that order.

Result: `{"exit": 0, "stdout": <canonical JSON of the result>, "stderr": ""}`, canonical meaning
`json.dumps(obj, sort_keys=True, separators=(",", ":"))` (Rust: `pycompat::json::dumps(.., &COMPACT_SORTED)`).
When Python raises (RecursionError on hostile nesting), the expected result is the incomplete one, marked
D-1: guard.py turns any exception into "allow", and Rust blocks instead (GC-16 D-1).

`--batch`: JSON lines `{"id": ID, "data": DATA}` on stdin, JSON lines `{"id": ID, "out": run(DATA)}` on stdout
(canonical form), in input order.

Python 3.9+ and the standard library only.
"""
from __future__ import annotations

import importlib.util
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
GUARD_RULES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "guard_rules_fw.py")


def _load():
    spec = importlib.util.spec_from_file_location("anthology_oracle_guard_rules", GUARD_RULES)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


G = _load()


def canonical(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


class _Groups:
    """Ordinal of each group object, by first appearance; keeps the objects alive while numbering."""

    def __init__(self):
        self.ids = {}
        self.objects = []

    def ordinal(self, group):
        key = id(group)
        if key not in self.ids:
            self.ids[key] = len(self.objects)
            self.objects.append(group)
        return self.ids[key]

    def texts(self):
        return [list(group.texts) for group in self.objects]


def _budget(text):
    """`check`'s work budget for a command of this length."""
    G._STATE.reset(G._WORK_PER_CHAR * len(text) + G._WORK_BASE)


def _segments(text, depth):
    _budget(text)
    G._charge(len(text) + 1)                              # as `_segments` does
    try:
        segs, lenient = G._Lexer(text, depth).run(), False
    except G._Unbalanced:
        segs, lenient = G._lenient_segments(text), True
    groups = _Groups()
    out = [{"tokens": list(seg.tokens), "redirs": [list(r) for r in seg.redirs],
            "group": groups.ordinal(seg.group)} for seg in segs]
    return {"lenient": lenient, "incomplete": False, "segments": out, "groups": groups.texts()}


def _shell_parse(prog, args, i):
    mode, start, scripts = G._shell_parse(prog, list(args), i)
    return {"mode": mode, "start": start, "scripts": [list(s) for s in scripts]}


def _script_starts(words):
    return {"starts": [list(s) for s in G._script_starts(list(words))]}


_ALTERNATES = []
_BRACE_READING = [None]                                   # the brace reading `_add_alternates` is given


def _add_alternates(alternates, scripts, entry, ctx, exempt):
    """`_add_alternates`, recording every alternate whose command differs from the entry's instead of
    checking G1. The scripts of every alternate are joined, as Rust does; the reference stops at the first
    alternate G1 blocks, so it joins fewer (the oracle and Rust are the stricter ones here)."""
    main = G._strip_prefixes(entry.raw)[0]
    for alternate in alternates:
        cmd, more = G._strip_prefixes(alternate, layer_scripts=False)
        if alternate is _BRACE_READING[0] and cmd[:1] != main[:1]:
            raise G._TooComplex()                         # re-review C-3a
        scripts.extend(script for script in more if script not in scripts)
        if cmd != main:
            other = G._Entry(entry.raw, [], cmd, entry.cwd, exempt, entry.group)
            other.exempt = exempt or other.prog in G._DOCKER
            _ALTERNATES.append(other)


G._add_alternates = _add_alternates


def _decode_ansi_c(piece):
    """`piece` with each closed `$'...'` decoded (bash 5) and the other quotes dropped, or None."""
    out, last, start, found = [], 0, 0, False
    while True:
        at = piece.find("$'", start)
        if at < 0:
            break
        got = G._ansi_c(piece, at + 2)
        if got is None:
            break
        decoded, end = got
        out += [G._LENIENT_QUOTES.sub("", piece[last:at]), decoded]
        last = start = end
        found = True
    if not found:
        return None
    out.append(G._LENIENT_QUOTES.sub("", piece[last:]))
    return "".join(out)


def _lenient_segments(text):
    """`_lenient_segments` with Rust's third reading (FW-H review R1-M1, D-8)."""
    out = []
    G._charge(2 * len(text))
    for piece in G.re.split(r"[;&|\n()`]+|\$\(", text):
        spaced = piece.replace("'", " ").replace('"', " ").split()
        joined = G._LENIENT_QUOTES.sub("", piece).split()
        readings = [spaced, joined] if joined != spaced else [spaced]
        decoded = _decode_ansi_c(piece)
        if decoded is not None and decoded.split() not in readings:
            readings.append(decoded.split())
        for tokens in readings:
            if not tokens:
                continue
            seg = G._Seg(G._Group())
            seg.tokens = tokens
            seg.braces = True                             # re-review F-1: quoting is unknown here
            out.append(seg)
            for start, first in G._script_starts(tokens):   # `bash -c <rest of the line>`
                G._charge(len(tokens) - start)
                extra = G._Seg(G._Group())
                extra.tokens = [first] + tokens[start + 1:]
                extra.braces = True
                out.append(extra)
    return out


G._lenient_segments = _lenient_segments


def _alternate_span(tokens, main_at):
    """`_alternate_span` with Rust's package-manager rule (FW-I review R1-I1, D-8)."""
    for k, word in enumerate(tokens):
        if G._ASSIGNMENT.match(word) or word == "!" or word in G._KEYWORDS or word == "":
            continue
        name = G._prog_name(word)
        rest = tokens[k + 1:]
        if name in G._ALTERNATE_TRIGGERS:
            after = tokens[main_at + 1:]
            if name in G._JS_PACKAGE_MANAGERS and "--" in after:
                verb = k + G._pm_verb_index(tokens[k:])
                if not (verb < len(tokens) and tokens[verb] in G._RUNNER_SUBCOMMANDS):
                    return k + 1, main_at + 1 + after.index("--")
            return k + 1, len(tokens)
        if (name in G._QUIET_PROGRAMS or name in G._ELSEWHERE or name in G._SHELLS
                or name in G._DASHDASH_ARGUMENTS or G._INTERPRETER.match(name) or "--" not in rest):
            return None
        return k + 2 + rest.index("--"), len(tokens)
    return None


G._alternate_span = _alternate_span


# --- part B
# SH-FW review, part B (D-8, Rust stricter; crates/anthology-shell/src/collect.rs).

_MAX_BRACE_WORDS = 4096
_MAX_BRACE_DEPTH = 64


def _brace_at(word):
    """The first brace expression of `word`, as (open, close, commas), or None: a `{` (not `${`) with its
    matching `}` and a comma at its own level; one whose text holds a quote character is skipped."""
    stack, first = [], None
    for j, c in enumerate(word):
        if c == "{":
            stack.append([j, j > 0 and word[j - 1] == "$", False, []])
        elif c == "," and stack:
            stack[-1][3].append(j)
        elif c in "\"'" and stack:
            stack[-1][2] = True
        elif c == "}" and stack:
            at, param, quote, commas = stack.pop()
            if stack:
                stack[-1][2] = stack[-1][2] or quote
            if not param and not quote and commas and (first is None or at < first[0]):
                first = (at, j, commas)
    return first


def _expand_braces(word, level, out):
    """Bash's brace expansion of `word`, appended to `out`; too complex past `_MAX_BRACE_WORDS`."""
    found = _brace_at(word)
    if found is None:
        if len(out) >= _MAX_BRACE_WORDS:
            raise G._TooComplex()
        out.append(word)
        return
    if level >= _MAX_BRACE_DEPTH:
        raise G._TooComplex()
    open_, close, commas = found
    start = open_ + 1
    for end in commas + [close]:
        _expand_braces(word[:open_] + word[start:end] + word[close + 1:], level + 1, out)
        start = end + 1


def _brace_reading(tokens):
    """C-3: `tokens` with every brace expression expanded, empty words dropped; None without one."""
    words, reading, found = [], [], False
    for token in tokens:
        if "{" not in token or _brace_at(token) is None:
            reading.append(token)
            continue
        found = True
        before = len(words)
        _expand_braces(token, 0, words)
        reading += [w for w in words[before:] if w]
    return reading if found else None


_add_alternates_before_b = G._add_alternates


def _add_alternates_braces(alternates, scripts, entry, ctx, exempt):
    """C-3: the command with its brace expressions expanded is one more alternate reading, for a segment
    with an unquoted `{` (re-review F-1); one that changes the program word is too complex (C-3a)."""
    reading = _brace_reading(entry.raw) if getattr(ctx, "braces", False) else None
    if reading is not None:
        alternates = alternates + [reading]
    _BRACE_READING[0] = reading
    try:
        _add_alternates_before_b(alternates, scripts, entry, ctx, exempt)
    finally:
        _BRACE_READING[0] = None


G._add_alternates = _add_alternates_braces


def _echo_octal(text):
    """`\\0nnn` (echo -e, printf's `%b`) as `$'...'`'s `\\nnn`."""
    return G.re.sub(r"\\(\\|0(?=[0-7]))", lambda m: "\\\\" if m.group(1) == "\\" else "\\", text)


def _echo_decoded(prog, args):
    """C-4: what an echo/printf writes with its escapes decoded (`$'...'`, and `\\0nnn` as echo -e reads it);
    for printf also its arguments after the format alone. Each reading once, in that order."""
    words = list(args)
    while words and G.re.match(r"-[neE]+\Z", words[0]):
        words.pop(0)
    sources = [" ".join(words)]
    if prog == "printf":
        rest = words[1:] if words[:1] == ["--"] else words
        if len(rest) > 1:
            sources.append(" ".join(rest[1:]))
        if len(rest) < len(words):                        # re-review C-4a
            sources.append(" ".join(rest))
        if rest:
            sources.append(_printf_fill(rest[0], rest[1:]))
    out = []
    for source in sources:
        for decoded in (G._ansi_c_decode(source), G._ansi_c_decode(_echo_octal(source))):
            if decoded not in out:
                out.append(decoded)
    return out


_PRINTF_FLAGS = "#'-+ 0"
_PRINTF_LENMODS = "hjlLtz"
_PRINTF_NUMERIC = "diuoxXeEfFgGaA"


def _printf_int(arg):
    """A `*` width or precision argument: bash reads an integer; anything else (or none) is 0."""
    arg = (arg or "").lstrip()
    negative = arg[:1] == "-"
    if arg[:1] in "-+" and arg:
        arg = arg[1:]
    m = G.re.match(r"[0-9]*", arg)
    value = int(m.group(0)) if m.group(0) else 0
    return -value if negative else value


def _printf_fill(fmt, args):
    """crates/anthology-shell/src/collect/printf.rs `printf_fill`: the raw text `printf fmt args...` writes
    (re-review C-4a; a superset reading, escapes left as written)."""
    out, state = [], {"next": 0}

    def take():
        k = state["next"]
        if k < len(args):
            state["next"] = k + 1
            return args[k]
        return None

    def pad(text, width, left):
        fill = width - min(len(text), width)
        if fill > 1_000_000:
            raise G._TooComplex()                         # Rust: over its budget
        out.append(text + " " * fill if left else " " * fill + text)

    def once():
        i, n = 0, len(fmt)
        while i < n:
            c = fmt[i]
            if c == "\\":
                out.append(fmt[i:i + 2])
                i += 2
                continue
            if c != "%":
                j = i
                while j < n and fmt[j] not in "\\%":
                    j += 1
                out.append(fmt[i:j])
                i = j
                continue
            start, j = i, i + 1
            if fmt[j:j + 1] == "%":
                out.append("%")
                i = j + 1
                continue
            left = False
            while j < n and fmt[j] in _PRINTF_FLAGS:
                left = left or fmt[j] == "-"
                j += 1
            if fmt[j:j + 1] == "*":
                j += 1
                w = _printf_int(take())
                left = left or w < 0
                width = abs(w)
            else:
                m = G.re.match(r"[0-9]*", fmt[j:])
                width = int(m.group(0)) if m.group(0) else 0
                j += len(m.group(0))
            precision = None
            if fmt[j:j + 1] == ".":
                j += 1
                if fmt[j:j + 1] == "*":
                    j += 1
                    v = _printf_int(take())
                    precision = v if v >= 0 else None
                else:
                    m = G.re.match(r"[0-9]*", fmt[j:])
                    precision = int(m.group(0)) if m.group(0) else 0
                    j += len(m.group(0))
            while j < n and fmt[j] in _PRINTF_LENMODS:
                j += 1
            if j >= n:
                out.append(fmt[start:])
                i = n
                continue
            conv, after = fmt[j], j + 1
            if conv in "sq":
                text = take() or ""
                text = text if precision is None else text[:precision]
            elif conv == "b":
                text = take() or ""
            elif conv == "c":
                text = (take() or "")[:1]
            elif conv in _PRINTF_NUMERIC:
                got = take()
                text = "0" if got is None else got
            elif conv == "(":
                close = fmt.find(")", after)
                if close >= 0 and fmt[close + 1:close + 2] == "T":
                    take()
                    pad(fmt[after:close], width, left)
                    i = close + 2
                else:
                    out.append(fmt[start:after])
                    i = after
                continue
            else:
                out.append(fmt[start:after])
                i = after
                continue
            pad(text, width, left)
            i = after

    while True:
        begin = state["next"]
        once()
        if state["next"] == begin or state["next"] >= len(args):
            return "".join(out)


def _literal_output(text, depth):
    """`_literal_output`, with each echo/printf text also decoded (C-4)."""
    texts, groups = [], set()
    for seg in G._segments(text, depth + 1):
        cmd = G._strip_prefixes(seg.tokens)[0]
        if cmd and G._prog_name(cmd[0]) in ("echo", "printf"):
            decoded = _echo_decoded(G._prog_name(cmd[0]), cmd[1:])
            texts.append(G._echo_args_text(cmd[1:]))
            texts += [t for t in decoded if t not in texts]
        if id(seg.group) not in groups:
            groups.add(id(seg.group))
            texts.extend(seg.group.texts)
    return texts


def _dedupe(items):
    return [item for k, item in enumerate(items) if item not in items[:k]]


G._literal_output = _literal_output


def _collect_once(text, ctx, exempt, depth):
    """`_collect_once`, with each echo/printf text also fed decoded to a shell and to xargs/parallel, not
    to an interpreter's code (C-4); brace readings scoped to the segment (re-review F-1); a pipeline headed
    by a compound command or a function call fed what the commands before it print (re-review I-2a)."""
    outer = getattr(ctx, "braces", False)
    try:
        _collect_once_body(text, ctx, exempt, depth)
    finally:
        ctx.braces = outer


_CLOSERS = ("}", "fi", "done", "esac")


def _collect_once_body(text, ctx, exempt, depth):
    new_entries, braced, after_compound = [], set(), set()
    for seg in G._segments(text):
        ctx.braces = seg.braces
        entry = G._add_command(seg.tokens, seg.redirs, seg.group, ctx, exempt, depth)
        new_entries.append(entry)
        if seg.braces:
            braced.add(id(entry))
        if seg.after_compound:
            after_compound.add(id(entry))
    position = {id(e): k for k, e in enumerate(new_entries)}
    defined = {}                                          # I-2a: `f()` lexes as `f`, `function f` as is
    for k, e in enumerate(new_entries):
        if len(e.raw) == 1:
            defined.setdefault(e.raw[0], k)
        elif len(e.raw) >= 2 and e.raw[0] == "function":
            defined.setdefault(e.raw[1], k)
    fed_from = 0
    by_group = {}
    for entry in new_entries:
        by_group.setdefault(id(entry.group), []).append(entry)
    for entries in by_group.values():
        shell = next((e for e in entries if G._reads_stdin_as_shell(e)), None)
        interpreters = [e for e in entries if G._INTERPRETER.match(e.prog)]
        feeders = [e for e in entries if e.stdin_runs]
        if shell is None and not interpreters and not feeders:
            continue
        texts, decoded = list(entries[0].group.texts), []
        for e in entries:
            if e.prog in ("echo", "printf"):
                texts.append(G._echo_text(e))
                decoded += _echo_decoded(e.prog, e.args)
        head = entries[0]
        at = position[id(head)]
        if not texts and (id(head) in after_compound
                          or (len(head.raw) == 1 and head.raw[0] in _CLOSERS)
                          or defined.get(head.prog, at) < at):
            for e in new_entries[min(fed_from, at):at]:
                if e.prog in ("echo", "printf"):
                    texts.append(G._echo_text(e))
                    decoded += _echo_decoded(e.prog, e.args)
            fed_from = max(fed_from, at)
        fed = list(texts)
        fed += [t for t in _dedupe(decoded) if t not in fed]
        for entry in interpreters:
            G._charge(sum(map(len, texts)))
            entry.code.extend(texts)
        for body in fed if shell is not None else ():
            G._collect(body, ctx, shell.exempt, depth + 1)
        for entry in feeders:
            ctx.braces = id(entry) in braced
            G._feed_stdin(entry, fed, ctx, depth)
    if "<(" in text and any(e.prog in G._SHELLS or e.prog in ("source", ".") for e in new_entries):
        for inner in G._process_substitutions(text):
            for body in G._literal_output(inner, depth):
                G._collect(body, ctx, exempt, depth + 1)
    fd_shells = {id(e) for e in new_entries if G._reads_fd_as_shell(e)}
    for entry in new_entries if fd_shells else ():
        if id(entry) in fd_shells or (
                not entry.prog and entry.raw and G._prog_name(entry.raw[0]) == "exec"):
            for body in entry.group.texts:
                G._collect(body, ctx, exempt, depth + 1)


G._collect_once = _collect_once


# --- part C

_END_SEG = G._Lexer.end_seg


def _end_seg(self, new_group):
    """`_Lexer.end_seg` with Rust's pipeline rule (SH-FW review I-2): a break before any command follows a
    `|`/`|&` keeps the group, so `echo X |\nsh` stays one pipeline. Re-review: the segment records whether
    an unquoted `{` was among its words (F-1) and whether it follows a `|` with no command before it in its
    pipeline, after a `)` (I-2a)."""
    empty = not (self.has_word or self.seg.tokens or self.seg.redirs)
    piped = getattr(self, "piped", False)
    if new_group and piped and empty:
        new_group = False
    else:
        if not new_group and empty and not piped:
            self.compound = True
        self.piped = not new_group
    braces, self.braces = getattr(self, "braces", False), False
    count = len(self.out)
    _END_SEG(self, new_group)
    if len(self.out) > count:
        seg = self.out[-1]
        seg.braces = braces
        seg.after_compound, self.compound = getattr(self, "compound", False), False
    if new_group:
        self.compound = False


G._Lexer.end_seg = _end_seg

_ADD = G._Lexer.add
_RUN = G._Lexer.run.__code__


def _add(self, chars):
    """`_Lexer.add`, noting an unquoted `{` (`run`'s default branch; re-review F-1)."""
    if chars == "{":
        caller = sys._getframe(1)
        if caller.f_code is _RUN and caller.f_locals.get("c") == "{":
            self.braces = True
    _ADD(self, chars)


G._Lexer.add = _add


class _Seg(G._Seg):
    """`_Seg` with Rust's segment flags (re-review F-1, I-2a)."""
    __slots__ = ("braces", "after_compound")

    def __init__(self, group):
        super().__init__(group)
        self.braces = False
        self.after_compound = False


G._Seg = _Seg


def _entry(e, groups):
    return {
        "raw": list(e.raw), "redirs": [list(r) for r in e.redirs], "prog_path": e.prog_path,
        "prog": e.prog, "args": list(e.args), "cwd": e.cwd, "exempt": e.exempt,
        "group": groups.ordinal(e.group), "code": list(e.code),
    }


def _analyze(command, home):
    _budget(command)
    del _ALTERNATES[:]
    entries = G._analyze(command, home) if command else []
    for reading in ("exact", "wide"):                     # `_check`, without its early exit on a rule
        if not (command and G._STATE.paren_lines):
            break
        G._STATE.reading = reading
        entries += G._analyze(command, home)
    groups = _Groups()
    out = [_entry(e, groups) for e in entries]
    alternates = [_entry(e, groups) for e in _ALTERNATES]
    return {"incomplete": False, "entries": out, "alternates": alternates, "groups": groups.texts()}


def _call(data):
    """The function and its arguments for a case (a malformed case is a harness bug: it raises here)."""
    op = data["op"]
    if op == "segments":
        return _segments, (data["text"], data.get("depth", 0))
    if op == "shell_parse":
        return _shell_parse, (data["prog"], data["args"], data.get("i", 0))
    if op == "script_starts":
        return _script_starts, (data["words"],)
    if op == "analyze":
        return _analyze, (data["command"], data["home"])
    raise ValueError("shell oracle: unknown op %r" % (op,))


def run(case):
    data = case["data"] if "data" in case and "op" not in case else case
    function, args = _call(data)
    try:
        result = function(*args)
    except G._TooComplex:  # over the work budget: `check` blocks ("too complex")
        return dict(_incomplete(data["op"]), divergence="D-7")
    except Exception:  # RecursionError and anything else guard.py would turn into "allow"
        return dict(_incomplete(data["op"]), divergence="D-1")
    finally:
        G._STATE.reset(None)
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def _incomplete(op):
    """Rust's result for a command it stopped on (the guard blocks it)."""
    if op == "segments":
        result = {"lenient": False, "incomplete": True, "segments": [], "groups": []}
    else:
        result = {"incomplete": True, "entries": [], "alternates": [], "groups": []}
    return {"exit": 0, "stdout": canonical(result), "stderr": ""}


def main(argv):
    if argv != ["--batch"]:
        print("usage: python3 -I port/difftest/oracle/shell.py --batch < cases.jsonl", file=sys.stderr)
        return 2
    out = sys.stdout
    for line in sys.stdin:
        if not line.strip():
            continue
        case = json.loads(line)
        out.write(canonical({"id": case["id"], "out": run(case["data"])}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
