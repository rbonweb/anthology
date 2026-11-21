"""Reversible, byte-stable edits of the user's settings.json (`/anthology:setup`).

`settings` is the file's TEXT (`""` = a missing file). Edits are textual splices located by a
small position-tracking JSON scanner, so every byte outside an edited span is unchanged,
inserted lines copy the neighbouring indentation and the file's newline style, and a compact
one-line file stays one line. Text that is not a JSON object is refused (`ValueError`): a
file that cannot be parsed is never rewritten.

Ops are JSON-serialisable dicts:

    {"op": "add", "path": ["permissions", "allow"], "value": "Bash(rtk *)"}   # list entry
    {"op": "set", "path": ["worktree", "baseRef"], "value": "head"}           # object key
    {"op": "unset", "path": ["env", "CLAUDE_CODE_SUBAGENT_MODEL"]}            # drop a key

`plan(text, ops)` drops ops that are already satisfied (the user's own pre-existing rule is
never ours) and records what is needed to undo each one exactly: the containers it created,
the raw text of an empty container it filled, and the previous value of a replaced or dropped
key. `revert(text, planned_ops)` removes only those entries, in reverse order, and puts a dropped
key back unless the user has set it again. Every diff shown to
the user (and so to Claude) shows values only under `permissions.allow`, `permissions.deny` and
`worktree.baseRef`, all of them, including the user's own rule strings (a token inside one of those
rules is shown). Every other value (`env`, tokens, hook commands, URLs, ...) reads `"<redacted>"`,
in any layout. `ops_version(ops)` fingerprints the op set, so session start
can tell when a release changed it. Python stdlib only (3.9+).
"""
from __future__ import annotations

import difflib
import hashlib
import json
import os
import re
import stat
import tempfile
from pathlib import Path

BYPASS_NOTE = ("Note: `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`, which runs any command. rtk's hook "
               "still applies the deny and ask rules in your settings files (user, project, local and managed) "
               "to the command it wraps, but not rules given with `--settings` or `--disallowedTools`, "
               "server-managed settings or an MDM profile.")
ALLOW_RTK = "Bash(rtk *)"
# Lets read-only agents save memory notes (heredocs/redirects never match permission grants).
ALLOW_MEM_SAVE = "Bash(anthology-mem save *)"
TAG_PLUGIN_READ = "plugin-read"
TAG_MEM_SAVE = "mem-save"
# Ops whose value may change between runs (a moved plugin, a revised rule string): re-running
# setup replaces the earlier value it added for the same tag.
_REPLACEABLE_TAGS = (TAG_PLUGIN_READ, TAG_MEM_SAVE)
# Part of `ops_version`; any change to the op list (a new rule, a new guard_rules entry) changes
# the version on its own. Session start only notes a changed version while an op is still missing
# from settings.json, so a bump alone, or a release that only drops an op, shows nothing.
OPS_VERSION = 1
# The opt-in removal setup offers (never part of `setup_ops`, so never part of `ops_version`).
SUBAGENT_MODEL_PATH = ("env", "CLAUDE_CODE_SUBAGENT_MODEL")
REDACTED = "<redacted>"
# The only places whose values a diff shows: where setup makes its changes.
SHOWN_PATHS = (("permissions", "allow"), ("permissions", "deny"), ("worktree", "baseRef"))

_WS = " \t\r\n"
_STRING = re.compile(r'"(?:[^"\\\x00-\x1f]|\\.)*"', re.S)
_SCALAR = re.compile(r"-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null")
_MAX_DEPTH = 100


# ---------------------------------------------------------------------------------------------
# Position-tracking JSON scanner
# ---------------------------------------------------------------------------------------------


class _Member:
    __slots__ = ("key", "kstart", "kend", "value")

    def __init__(self, key, kstart, kend, value):
        self.key, self.kstart, self.kend, self.value = key, kstart, kend, value


class _Node:
    """kind: object (items = members), array (items = nodes), string or scalar."""
    __slots__ = ("kind", "start", "end", "items")

    def __init__(self, kind, start, end, items=None):
        self.kind, self.start, self.end, self.items = kind, start, end, items or []


class _Scanner:
    def __init__(self, text):
        self.t = text
        self.i = 1 if text.startswith("\ufeff") else 0

    def ws(self):
        t, i, n = self.t, self.i, len(self.t)
        while i < n and t[i] in _WS:
            i += 1
        self.i = i

    def peek(self):
        return self.t[self.i] if self.i < len(self.t) else ""

    def value(self, depth):
        if depth > _MAX_DEPTH:
            raise ValueError("settings.json nests too deeply")
        self.ws()
        c = self.peek()
        if c == "{":
            return self.obj(depth)
        if c == "[":
            return self.arr(depth)
        if c == '"':
            m = _STRING.match(self.t, self.i)
            if not m:
                raise ValueError("settings.json is not valid JSON")
            node = _Node("string", self.i, m.end())
            self.i = m.end()
            return node
        m = _SCALAR.match(self.t, self.i)
        if not m:
            raise ValueError("settings.json is not valid JSON")
        node = _Node("scalar", self.i, m.end())
        self.i = m.end()
        return node

    def obj(self, depth):
        start = self.i
        self.i += 1
        members = []
        self.ws()
        if self.peek() == "}":
            self.i += 1
            return _Node("object", start, self.i, members)
        while True:
            self.ws()
            m = _STRING.match(self.t, self.i) if self.peek() == '"' else None
            if not m:
                raise ValueError("settings.json is not valid JSON")
            kstart, kend = self.i, m.end()
            key = json.loads(self.t[kstart:kend])
            self.i = kend
            self.ws()
            if self.peek() != ":":
                raise ValueError("settings.json is not valid JSON")
            self.i += 1
            members.append(_Member(key, kstart, kend, self.value(depth + 1)))
            self.ws()
            c = self.peek()
            self.i += 1
            if c == "}":
                return _Node("object", start, self.i, members)
            if c != ",":
                raise ValueError("settings.json is not valid JSON")

    def arr(self, depth):
        start = self.i
        self.i += 1
        items = []
        self.ws()
        if self.peek() == "]":
            self.i += 1
            return _Node("array", start, self.i, items)
        while True:
            items.append(self.value(depth + 1))
            self.ws()
            c = self.peek()
            self.i += 1
            if c == "]":
                return _Node("array", start, self.i, items)
            if c != ",":
                raise ValueError("settings.json is not valid JSON")


def _is_blank(text):
    return text.strip("\ufeff" + _WS) == ""


def _parse(text):
    """The root object node of `text` (ValueError unless it is one JSON object)."""
    try:
        json.loads(text[1:] if text.startswith("\ufeff") else text)
    except ValueError as exc:
        raise ValueError("settings.json is not valid JSON: %s" % exc)
    scanner = _Scanner(text)
    root = scanner.value(0)
    scanner.ws()
    if scanner.i != len(text):
        raise ValueError("settings.json is not valid JSON")
    if root.kind != "object":
        raise ValueError("settings.json is not a JSON object")
    return root


def _member(node, key):
    for m in reversed(node.items):
        if m.key == key:
            return m
    return None


def _dotted(path):
    return ".".join(path) or "settings.json"


def _descend(root, path):
    """Chain of nodes `[root, ...]` for the leading part of `path` that exists. A non-object
    where a key must be looked up is a ValueError."""
    chain = [root]
    for i, key in enumerate(path):
        node = chain[-1]
        if node.kind != "object":
            raise ValueError("%s is not an object" % _dotted(path[:i]))
        m = _member(node, key)
        if m is None:
            break
        chain.append(m.value)
    return chain


def _strings(text, arr):
    out = []
    for item in arr.items:
        out.append(json.loads(text[item.start:item.end]) if item.kind == "string" else None)
    return out


# ---------------------------------------------------------------------------------------------
# Formatting
# ---------------------------------------------------------------------------------------------


class _Style:
    def __init__(self, nl, unit, colon, comma):
        self.nl, self.unit, self.colon, self.comma = nl, unit, colon, comma


def _style(text, root):
    nl = "\r\n" if "\r\n" in text else "\n"
    m = re.search(r"(?m)^([ \t]+)\S", text)
    unit = ("\t" if m.group(1).startswith("\t") else m.group(1)) if m else "  "
    colon = ": "
    if root is not None:
        stack = [root]
        while stack:
            node = stack.pop(0)
            if node.kind == "object":
                if node.items:
                    first = node.items[0]
                    colon = text[first.kend:first.value.start]
                    break
            elif node.kind == "array":
                stack.extend(node.items)
    return _Style(nl, unit, colon, "," if colon == ":" else ", ")


def _render(value, indent, st, multi):
    """JSON text of `value` whose own line starts with `indent` (dicts keep insertion order)."""
    if isinstance(value, dict):
        if not value:
            return "{}"
        inner = indent + st.unit
        parts = [json.dumps(k, ensure_ascii=False) + st.colon + _render(v, inner, st, multi)
                 for k, v in value.items()]
        if multi:
            return "{" + st.nl + inner + ("," + st.nl + inner).join(parts) + st.nl + indent + "}"
        return "{" + st.comma.join(parts) + "}"
    if isinstance(value, list):
        if not value:
            return "[]"
        inner = indent + st.unit
        parts = [_render(v, inner, st, multi) for v in value]
        if multi:
            return "[" + st.nl + inner + ("," + st.nl + inner).join(parts) + st.nl + indent + "]"
        return "[" + st.comma.join(parts) + "]"
    return json.dumps(value, ensure_ascii=False)


def _line_indent(text, pos):
    start = text.rfind("\n", 0, pos) + 1
    return re.match(r"[ \t]*", text[start:pos]).group(0)


def _item_start(node, idx):
    item = node.items[idx]
    return item.kstart if node.kind == "object" else item.start


def _item_end(node, idx):
    item = node.items[idx]
    return item.value.end if node.kind == "object" else item.end


def _container_multiline(text, chain):
    """Is the (empty) container at the end of `chain` laid out over several lines? Follows
    its parent, which holds it as an item."""
    if len(chain) < 2:
        return True
    parent = chain[-2]
    return "\n" in text[parent.start + 1:_item_start(parent, 0)]


def _sep(text, node):
    """The whitespace that precedes the last item of `node` (after its comma or bracket)."""
    n = len(node.items)
    prev_end = _item_end(node, n - 2) if n > 1 else node.start + 1
    gap = text[prev_end:_item_start(node, n - 1)]
    return gap.rsplit(",", 1)[-1] if n > 1 else gap


def _append(text, root, chain, node, key, value):
    """Add `value` to the array `node` (key None) or `key: value` to the object `node`."""
    st = _style(text, root)
    if node.items:
        sep = _sep(text, node)
        multi = "\n" in sep
        if not multi and sep == "" and len(node.items) == 1:
            sep = st.comma[1:]  # inline: keep the file's `, ` style (the gap is empty when one item)
        indent = sep.rsplit("\n", 1)[-1] if multi else ""
        if key is None:
            item = _render(value, indent, st, multi)
        else:
            last = node.items[-1]
            item = (json.dumps(key, ensure_ascii=False) + text[last.kend:last.value.start]
                    + _render(value, indent, st, multi))
        at = _item_end(node, len(node.items) - 1)
        return text[:at] + "," + sep + item + text[at:]
    multi = _container_multiline(text, chain)
    base = _line_indent(text, node.start)
    new = _render([value] if key is None else {key: value}, base, st, multi)
    return text[:node.start] + new + text[node.end:]


def _remove_at(text, node, idx):
    """Drop item `idx` of `node`. The inverse of `_append` for the last item."""
    n = len(node.items)
    if n == 1:
        return text[:node.start] + ("{}" if node.kind == "object" else "[]") + text[node.end:]
    if idx == n - 1:
        return text[:_item_end(node, n - 2)] + text[_item_end(node, n - 1):]
    return text[:_item_start(node, idx)] + text[_item_start(node, idx + 1):]


def _nest(keys, leaf):
    for key in reversed(keys):
        leaf = {key: leaf}
    return leaf


# ---------------------------------------------------------------------------------------------
# Ops
# ---------------------------------------------------------------------------------------------


def _create(text, op, path, leaf, created_upto, existing=None):
    """Insert `leaf` at `path`, creating the missing containers. Returns (text, info)."""
    blank = _is_blank(text)
    if blank:
        st = _style(text, None)
        doc = _render(_nest(path, leaf), "", st, True) + st.nl
        if text.startswith("\ufeff"):
            doc = "\ufeff" + doc
        created = [path[:k] for k in range(1, created_upto + 1)]
        return doc, {"created": created, "empty": {"path": [], "raw": text, "whole": True}}
    root = existing or _parse(text)
    chain = _descend(root, path)
    n = len(chain) - 1
    node = chain[-1]
    info = {"created": [path[:k] for k in range(n + 1, created_upto + 1)]}
    if not node.items:
        info["empty"] = {"path": path[:n], "raw": text[node.start:node.end]}
    return _append(text, root, chain, node, path[n], _nest(path[n + 1:], leaf)), info


def _apply_op(text, op):
    """Apply one op. Returns `(text, info)`; info is None when it was already satisfied."""
    path = list(op["path"])
    if op["op"] == "unset":
        if _is_blank(text):
            return text, None
        chain = _descend(_parse(text), path)
        if len(chain) - 1 != len(path):
            return text, None
        node, parent = chain[-1], chain[-2]
        idx = [m.value for m in parent.items].index(node)
        return _remove_at(text, parent, idx), {"created": [], "previous": {"present": True,
                                                                          "raw": text[node.start:node.end]}}
    value = op["value"]
    if op["op"] == "add":
        if not _is_blank(text):
            root = _parse(text)
            chain = _descend(root, path)
            if len(chain) - 1 == len(path):
                arr = chain[-1]
                if arr.kind != "array":
                    raise ValueError("%s is not a list" % _dotted(path))
                if value in _strings(text, arr):
                    return text, None
                info = {"created": []}
                if not arr.items:
                    info["empty"] = {"path": path, "raw": text[arr.start:arr.end]}
                return _append(text, root, chain, arr, None, value), info
            return _create(text, op, path, [value], len(path), root)
        return _create(text, op, path, [value], len(path))
    if op["op"] != "set":
        raise ValueError("unknown settings op %r" % (op["op"],))
    if not _is_blank(text):
        root = _parse(text)
        chain = _descend(root, path)
        if len(chain) - 1 == len(path):
            node = chain[-1]
            raw = text[node.start:node.end]
            if json.loads(raw) == value:
                return text, None
            new = json.dumps(value, ensure_ascii=False)
            return (text[:node.start] + new + text[node.end:],
                    {"created": [], "previous": {"present": True, "raw": raw}})
        out, info = _create(text, op, path, value, len(path) - 1, root)
    else:
        out, info = _create(text, op, path, value, len(path) - 1)
    info["previous"] = {"present": False}
    return out, info


_INFO_KEYS = ("created", "empty", "previous")


def _run(text, ops):
    planned = []
    for op in ops:
        text, info = _apply_op(text, op)
        if info is not None:
            base = {k: v for k, v in op.items() if k not in _INFO_KEYS}
            base["path"] = list(base["path"])
            base.update(info)
            planned.append(base)
    return text, planned


def _hidden_leaves(node, path, out):
    """The string and scalar nodes outside `SHOWN_PATHS` (an array item adds `[]` to the path)."""
    if path in SHOWN_PATHS:
        return
    if node.kind == "object":
        for m in node.items:
            _hidden_leaves(m.value, path + (m.key,), out)
    elif node.kind == "array":
        for item in node.items:
            _hidden_leaves(item, path + ("[]",), out)
    else:
        out.append(node)


def redact(text):
    """`text` with every value outside `SHOWN_PATHS` replaced by `"<redacted>"`. Keys, layout and
    line count are kept, so a diff of two redacted texts stays exact for what setup changes."""
    if _is_blank(text):
        return text
    leaves = []
    _hidden_leaves(_parse(text), (), leaves)
    new = json.dumps(REDACTED)
    for node in sorted(leaves, key=lambda n: n.start, reverse=True):
        text = text[:node.start] + new + text[node.end:]
    return text


def _diff(old, new):
    """The unified diff shown to the user (and Claude), redacted on both sides."""
    if old == new:
        return ""
    return "\n".join(difflib.unified_diff(redact(old).splitlines(), redact(new).splitlines(),
                                          "settings.json", "settings.json", lineterm="", n=0))


def plan(settings, ops):
    """The ops that still have to be applied to `settings`, with what is needed to undo them."""
    if not ops and not _is_blank(settings):
        _parse(settings)
    return _run(settings, ops)[1]


def apply(settings, ops):
    """`(new_text, unified_diff)`. Applying an op twice is a no-op."""
    new, _ = _run(settings, ops)
    return new, _diff(settings, new)


def _revert_op(text, op):
    path = list(op["path"])
    root = _parse(text)
    chain = _descend(root, path)
    found = len(chain) - 1 == len(path)
    if op["op"] == "unset":
        prev = op.get("previous") or {}
        if found or not prev.get("present"):
            return text  # set again since: the user's value stays
        value = json.loads(prev["raw"])
        if len(chain) == len(path):
            return _append(text, root, chain, chain[-1], path[-1], value)
        return _create(text, op, path, value, len(path) - 1, root)[0]
    value = op["value"]
    if op["op"] == "add":
        if found and chain[-1].kind == "array":
            names = _strings(text, chain[-1])
            if value in names:
                idx = len(names) - 1 - names[::-1].index(value)
                text = _remove_at(text, chain[-1], idx)
    elif found and len(chain) > 1:
        node = chain[-1]
        if json.loads(text[node.start:node.end]) == value:
            prev = op.get("previous") or {}
            if prev.get("present"):
                text = text[:node.start] + prev["raw"] + text[node.end:]
            else:
                parent = chain[-2]
                idx = [m.value for m in parent.items].index(node)
                text = _remove_at(text, parent, idx)
    for created in reversed(op.get("created") or []):
        chain = _descend(_parse(text), list(created))
        if len(chain) - 1 != len(created) or len(chain) < 2:
            continue
        node, parent = chain[-1], chain[-2]
        if node.kind not in ("object", "array") or node.items:
            break
        idx = [m.value for m in parent.items].index(node)
        text = _remove_at(text, parent, idx)
    empty = op.get("empty")
    if empty:
        chain = _descend(_parse(text), list(empty["path"]))
        node = chain[-1]
        if len(chain) - 1 == len(empty["path"]) and node.kind in ("object", "array") and not node.items:
            if empty.get("whole"):
                return empty["raw"]
            text = text[:node.start] + empty["raw"] + text[node.end:]
    return text


def revert(settings, ops):
    """`(new_text, unified_diff)` with the planned `ops` removed, last first."""
    text = settings
    if _is_blank(text):
        return settings, ""
    _parse(text)
    for op in reversed(list(ops)):
        if _is_blank(text):
            break
        text = _revert_op(text, op)
    return text, _diff(settings, text)


# ---------------------------------------------------------------------------------------------
# The setup op set
# ---------------------------------------------------------------------------------------------


def plugin_read_rule(plugin_root):
    """U1: `Read(//<parent of the plugin root>/**)` (`//` = an absolute path in rule syntax)."""
    parent = os.path.dirname(os.path.realpath(str(plugin_root)))
    return "Read(//%s/**)" % parent.lstrip("/")


_RULE_META = frozenset("*?[]{}()\\")


def validate_plugin_root(plugin_root, env=None):
    """F21: refuse a plugin root the U1 allow rule cannot safely be built from (ValueError).

    It must be an absolute, existing folder holding `.claude-plugin/plugin.json` named
    `anthology`; its parent (the folder the rule opens up) must not be `/`, the home folder or an
    ancestor of it; and its path must hold no glob or rule metacharacter."""
    raw = str(plugin_root)
    if not raw or not os.path.isabs(raw):
        raise ValueError("the plugin root must be an absolute path (got %r)" % raw)
    real = os.path.realpath(raw)
    if not os.path.isdir(real):
        raise ValueError("the plugin root %s is not a folder" % real)
    try:
        with open(os.path.join(real, ".claude-plugin", "plugin.json"), encoding="utf-8") as fh:
            name = json.load(fh).get("name")
    except (OSError, ValueError, AttributeError):
        name = None
    if name != "anthology":
        raise ValueError("%s is not the anthology plugin (no .claude-plugin/plugin.json named anthology)" % real)
    parent = os.path.dirname(real)
    env = os.environ if env is None else env
    home = os.path.realpath(env.get("HOME") or os.path.expanduser("~"))
    if parent == os.sep or home == parent or home.startswith(parent.rstrip(os.sep) + os.sep):
        raise ValueError("refusing to allow reads of %s: it is the home folder, one of its parents or "
                         "the root, so the plugin must live in its own folder" % parent)
    if any(c in _RULE_META for c in real):
        raise ValueError("the plugin path %s has a character (* ? [ ] { } ( ) or a backslash) that "
                         "permission rules would read as a pattern" % real)


def plugin_rule_status(plugin_root, env=None):
    """`"ok"`, `"missing"` (not in `permissions.allow`) or `"unreadable"` for the U1 rule in the
    user's settings.json. Shared by session start and doctor."""
    try:
        text = read_text(settings_path(env))
        if _is_blank(text):
            return "missing"
        data = json.loads(text[1:] if text.startswith("\ufeff") else text)
        permissions = data.get("permissions") if isinstance(data, dict) else None
        allow = permissions.get("allow") if isinstance(permissions, dict) else None
    except Exception:
        return "unreadable"
    if isinstance(allow, list) and plugin_read_rule(plugin_root) in allow:
        return "ok"
    return "missing"


def setup_ops(plugin_root, profile_roots=None, env=None):
    """Everything `/anthology:setup` changes in settings.json (ValueError for an unsafe plugin root).

    The profile roots come from the guardrails plugin's `guard_rules.PROFILE_ROOTS` (a single
    source) unless given; for the `.config/` roots the deny rules also cover the guard's Linux
    channel variants (`guard_rules._PREFIX_VARIANTS`: `-beta`, `-dev`, ...)."""
    validate_plugin_root(plugin_root, env)
    variants = ("",)
    if profile_roots is None:
        import guardrails_loc
        rules = guardrails_loc.load_rules(plugin_root, env)
        profile_roots = rules.PROFILE_ROOTS
        variants = getattr(rules, "_PREFIX_VARIANTS", ("",))
    ops = [{"op": "add", "path": ["permissions", "allow"], "value": ALLOW_RTK},
           {"op": "add", "path": ["permissions", "allow"], "value": ALLOW_MEM_SAVE,
            "tag": TAG_MEM_SAVE}]
    for root in profile_roots:
        for v in (variants if root.startswith(".config/") else ("",)):
            for rule in ("Read(~/%s%s)" % (root, v), "Read(~/%s%s/**)" % (root, v)):
                ops.append({"op": "add", "path": ["permissions", "deny"], "value": rule})
    ops.append({"op": "add", "path": ["permissions", "allow"], "value": plugin_read_rule(plugin_root),
                "tag": TAG_PLUGIN_READ})
    ops.append({"op": "set", "path": ["worktree", "baseRef"], "value": "head"})
    return ops


def subagent_model_op():
    """The opt-in op that drops `env.CLAUDE_CODE_SUBAGENT_MODEL` (setup asks first)."""
    return {"op": "unset", "path": list(SUBAGENT_MODEL_PATH)}


def ops_version(ops):
    """A fingerprint of the setup op set: `"<OPS_VERSION>-<hash>"`. It changes when a release
    adds, drops or edits an op. The plugin-read rule's value is left out: it follows where the
    plugin lives, which session start already checks on its own."""
    canon = []
    for op in ops:
        op = {k: v for k, v in op.items() if k not in _INFO_KEYS}
        if op.get("tag") == TAG_PLUGIN_READ:
            op["value"] = "<plugin root>"
        canon.append(op)
    payload = json.dumps([OPS_VERSION, canon], sort_keys=True, separators=(",", ":"))
    return "%d-%s" % (OPS_VERSION, hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16])


def _key(op):
    return (op["op"], tuple(op["path"]), json.dumps(op.get("value"), sort_keys=True))


def reconcile(settings, desired, previous):
    """Re-run setup over text that may already carry an earlier run's entries.

    Returns `(new_text, diff, ops_to_store)`. A plugin-read rule we added earlier for a root
    that is no longer current, or a memory-save rule whose string changed, is replaced; every other earlier op is kept in the record, so
    `remove` still undoes everything setup ever added."""
    previous = list(previous or [])
    want = {(o.get("tag"), o["value"]) for o in desired if o.get("tag") in _REPLACEABLE_TAGS}
    stale = [o for o in previous if o.get("tag") in _REPLACEABLE_TAGS
             and (o["tag"], o["value"]) not in want]
    text = revert(settings, stale)[0] if stale else settings
    kept = [o for o in previous if o not in stale]
    new = plan(text, desired)
    text, _ = apply(text, new)
    new_keys = {_key(o) for o in new}
    stored = [o for o in kept if _key(o) not in new_keys] + new
    return text, _diff(settings, text), stored


# ---------------------------------------------------------------------------------------------
# The file
# ---------------------------------------------------------------------------------------------


def settings_path(env=None):
    """`${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json`."""
    env = os.environ if env is None else env
    base = env.get("CLAUDE_CONFIG_DIR")
    if base:
        return Path(base) / "settings.json"
    return Path(env.get("HOME") or os.path.expanduser("~")) / ".claude" / "settings.json"


def read_text(path):
    """The file's text with newlines untouched; `""` when it does not exist."""
    try:
        with open(str(path), "rb") as fh:
            return fh.read().decode("utf-8")
    except FileNotFoundError:
        return ""


def write_text(path, text):
    """Replace the file atomically (temp file in the same folder, then `os.replace`), keeping
    its mode. A symlinked settings.json is written through to its target."""
    path = Path(path)
    target = Path(os.path.realpath(str(path))) if path.is_symlink() else path
    target.parent.mkdir(parents=True, exist_ok=True)
    try:
        mode = stat.S_IMODE(os.stat(str(target)).st_mode)
    except FileNotFoundError:
        mode = 0o644
    fd, tmp = tempfile.mkstemp(dir=str(target.parent), prefix=".settings.", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(text.encode("utf-8"))
        os.chmod(tmp, mode)
        os.replace(tmp, str(target))
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
