"""Privacy scan for notes that would be written into repo memory (which may be public).

`privacy_findings(text)` returns `[{"kind": str, "line": int}, ...]`: labels and 1-based line
numbers only, never the matched text. Every kind blocks a write into repo memory:

- `home-path`         `/Users/<name>` or `/home/<name>` (case-sensitive, as macOS and Linux spell
                      them) anywhere in an absolute path, not in a `scheme://host/...` URL or a
                      relative path, and the literal value of `$HOME`. An API route such as
                      `/api/home/feed` is flagged too: an accepted false positive on the safe side
- `email`             an email address (not noreply, GitHub noreply, `git@` a public forge, or a
                      reserved domain)
- `private-ip`        RFC 1918, CGNAT, link-local IPv4; IPv6 ULA and link-local
- `ip-address`        any other literal IPv4 (not loopback, documentation ranges, or versions)
- `private-hostname`  `*.local`, `*.lan`, `*.internal`, ... and this machine's hostname
- `ssh-target`        a `user@host` on a line that runs ssh, scp, sftp, rsync or mosh
- `ssh-key-path`      a path through a `.ssh/` folder, or a bare `id_rsa`-style key file name
- `secret:<label>`    a token from `SECRET_PATTERNS`

It is an accident guard, not a guarantee: it knows common formats only. `SECRET_PATTERNS` is the
one secret-pattern list on the anthology side (export/import reporting uses it too); a test keeps
every anthology-guardrails G6 pattern in it. Pure and stdlib only (3.9+); imports no anthology
module. The input may be hostile (an imported archive): the scan stays linear in the line length,
so no regex here is searched with an end bound from many start positions.
"""
from __future__ import annotations

import ipaddress
import os
import re
import socket

# What is not a secret value (the same fragments as guard_rules.py; a test checks the patterns match).
# A value that is wholly null/none/true/false or a template placeholder (`<...>`, `${...}`, a run of
# `*`, `.` or `x`), optionally quoted, ending at a blank, `,`, `}`, `]` or the end of the line. Quotes
# may carry a backslash (JSON inside a double-quoted shell string).
_VALUE_END = r"(?:\\?[\"'])?[ \t]*(?:[,}\]\r\n]|$)"
_NOT_A_VALUE = r"(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)" + _VALUE_END
# After the separator of an unquoted key: not a placeholder.
_UNQUOTED_VALUE = r"(?![ \t]*(?:\\?[\"'])?" + _NOT_A_VALUE + ")"
# After a key's closing quote (one optional quote, `\"` included): a separator, then a real scalar,
# not empty, an object or array (a schema) or a placeholder.
_QUOTED_VALUE = r"\\?[\"']\s*[:=]\s*(?:\\?[\"'])?(?![\\\"'{\[\]},\s]|" + _NOT_A_VALUE + ")"

# The anthology-guardrails G6 patterns, copied exactly (same source and flags; a test checks it),
# then the export/import patterns, then a few more common token formats.
SECRET_PATTERNS = (
    ("aws-access-key", re.compile(r"(?:AKIA|ASIA)[0-9A-Z]{16}")),
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9]+")),
    ("github-token", re.compile(r"github_pat_")),
    ("api-key", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}")),
    ("slack-token", re.compile(r"\bxox[baprs]-")),
    ("private-key", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    # A key may be followed by one closing quote (`"password": "x"`, a JSON or dict key).
    ("credential-assignment", re.compile(
        r"password(?:\s*[:=]" + _UNQUOTED_VALUE + "|" + _QUOTED_VALUE + r"\S)", re.IGNORECASE)),
    ("credential-assignment", re.compile(
        r"api[_-]?key(?:\s*[:=]" + _UNQUOTED_VALUE + "|" + _QUOTED_VALUE + r"\S)", re.IGNORECASE)),
    ("aws-access-key", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("github-token", re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}")),
    ("github-token", re.compile(r"github_pat_[A-Za-z0-9_]{20,}")),
    ("slack-token", re.compile(r"xox[abprs]-[A-Za-z0-9-]{10,}")),
    ("api-key", re.compile(r"sk-(?:ant-)?[A-Za-z0-9_-]{20,}")),
    ("credential-assignment",
     re.compile(r"(password|passwd|secret|api[_-]?key|token)(?:\s*[:=]" + _UNQUOTED_VALUE
                + r"\s*\S{8,}|" + _QUOTED_VALUE + r"\S{8,})", re.I)),
    # A look-behind, not \b: `-` is in the class, so \b would start a scan at every `-eyJ` (quadratic).
    ("jwt", re.compile(r"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")),
    # An AWS-style `aws_secret_access_key = …`, also JSON-quoted (`"SecretAccessKey": "…"`); the token
    # pattern above needs `secret` to be followed by `=`. One optional quote keeps the scan linear.
    ("cloud-secret-key", re.compile(r"(?:secret|access)[_-]?(?:access[_-]?)?key[\x22']?\s*[:=]\s*\S{8,}", re.I)),
    # `scheme://user:password@host`: the userinfo of a connection URL. Each run stops at `/`, so the scan is linear.
    ("url-credentials", re.compile(r"://[^\s/:@]*:[^\s/@]+@")),
    ("anthropic-key", re.compile(r"sk-ant-[A-Za-z0-9_-]{8,}")),
    ("google-api-key", re.compile(r"AIza[0-9A-Za-z_-]{35}")),
    ("stripe-live-key", re.compile(r"\b[sr]k_live_[0-9A-Za-z]{8,}")),
    ("bearer-token", re.compile(r"\bBearer\s+[A-Za-z0-9._~+/=-]{20,}")),
    ("basic-auth", re.compile(r"Authorization:\s*Basic\s+[A-Za-z0-9+/=]{8,}", re.IGNORECASE)),
    ("gitlab-token", re.compile(r"\bglpat-[A-Za-z0-9_-]{20,}")),
    ("huggingface-token", re.compile(r"\bhf_[A-Za-z0-9]{30,}")),
    ("npm-token", re.compile(r"\bnpm_[A-Za-z0-9]{36,}")),
)

_LABELS = {
    "home-path": "home path",
    "email": "email address",
    "private-ip": "private IP address",
    "ip-address": "IP address",
    "private-hostname": "private hostname",
    "ssh-target": "ssh user@host",
    "ssh-key-path": "ssh key path",
}

_HOME_PATH = re.compile(r"/(?:Users|home)/([A-Za-z0-9._-]+)")
_ALNUM = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
_PATH_CHARS = frozenset(_ALNUM + "._~$@%+/-")  # characters of a path run before a home segment
_SCHEME_CHARS = frozenset(_ALNUM + "+.-")
_TOKEN_END = re.compile(r"[ \t]")
_EMAIL = re.compile(r"(?<![\w.%+-])([A-Za-z0-9._%+-]+)@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})"
                    r"(?![\w-])")
_IPV4 = re.compile(r"(?<![\w.])(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?!\d|\.\d)")
_IPV6 = re.compile(r"(?<![\w:.])[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?:%[\w.-]+)?(?![\w:])")
_PRIVATE_SUFFIXES = r"(?:local|lan|internal|intranet|corp|home|localdomain|home\.arpa|ts\.net)"
_PRIVATE_HOST = re.compile(r"(?<![\w.-])(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+"
                           + _PRIVATE_SUFFIXES + r"(?![\w(-]|\.[\w-])", re.IGNORECASE)
_SSH_COMMAND = re.compile(r"(?<![\w-])(?:ssh|scp|sftp|rsync|mosh)(?![\w-])")
_USER_AT_HOST = re.compile(r"(?<![\w.%+-])([A-Za-z0-9._-]+)@([A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?)")
_SSH_PATH = re.compile(r"/\.ssh/[A-Za-z0-9._@+-]")
_KEY_NAME = re.compile(r"(?<![\w-])id_(?:rsa|ed25519|ecdsa|dsa)(?![\w-])")

_PRIVATE_V4 = tuple(ipaddress.ip_network(n) for n in (
    "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "169.254.0.0/16"))
_EXEMPT_V4 = tuple(ipaddress.ip_network(n) for n in (
    "127.0.0.0/8", "0.0.0.0/32", "255.255.255.255/32",
    "192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24"))
_PRIVATE_V6 = tuple(ipaddress.ip_network(n) for n in ("fc00::/7", "fe80::/10"))
_RESERVED_DOMAINS = ("example.com", "example.org", "example.net")
_RESERVED_TLDS = ("example", "test", "invalid", "localhost")
# `git@<forge>` is the shared ssh user of a public forge, not a person or a private host.
_PUBLIC_FORGES = ("github.com", "gitlab.com", "bitbucket.org", "codeberg.org", "ssh.dev.azure.com",
                  "git.sr.ht")
# Default or generic machine names: never a finding on their own.
_GENERIC_HOSTNAMES = frozenset((
    "ubuntu", "debian", "fedora", "arch", "alpine", "linux", "macbook", "macbook-pro",
    "macbook-air", "imac", "mac-mini", "mac", "localhost", "raspberrypi", "docker", "container",
    "server", "workstation", "desktop", "laptop"))


def _reserved_host(host):
    """A documentation / testing host name (RFC 2606 and RFC 6761)."""
    host = host.lower().rstrip(".")
    if host == "localhost" or host.rsplit(".", 1)[-1] in _RESERVED_TLDS:
        return True
    return any(host == d or host.endswith("." + d) for d in _RESERVED_DOMAINS)


def _forge_git_user(local, host):
    return local.lower() == "git" and host.lower().rstrip(".") in _PUBLIC_FORGES


def _exempt_email(local, domain):
    if _forge_git_user(local, domain):
        return True
    local, domain = local.lower(), domain.lower()
    if local in ("noreply", "no-reply"):
        return True
    if domain == "users.noreply.github.com":
        return True
    return _reserved_host(domain)


def _home_literal(home):
    if home is None:
        home = os.environ.get("HOME") or ""
    home = str(home).rstrip("/")
    if len(home) <= 1:
        return None
    return re.compile(re.escape(home) + r"(?![\w-])")


def _hostname_patterns(hostname):
    if hostname is None:
        try:
            hostname = socket.gethostname()
        except Exception:
            hostname = ""
    name = str(hostname or "").strip().rstrip(".")
    if name.lower().endswith(".local"):
        name = name[:-6]
    names = {name}
    if "." in name:
        names.add(name.split(".", 1)[0])  # the short name of an FQDN
    out = []
    for n in sorted(names):
        if len(n) >= 4 and n.lower() not in _GENERIC_HOSTNAMES:
            out.append(re.compile(r"(?<![\w-])" + re.escape(n) + r"(?![\w-])", re.IGNORECASE))
    return out


def _ipv4_kind(groups):
    octets = [int(g) for g in groups]
    if any(o > 255 for o in octets):
        return None
    addr = ipaddress.ip_address(".".join(str(o) for o in octets))
    if any(addr in net for net in _EXEMPT_V4):
        return None
    if any(addr in net for net in _PRIVATE_V4):
        return "private-ip"
    return "ip-address"


def _has_scheme(line, start, p):
    """True if a scheme (`[A-Za-z][A-Za-z0-9+.-]*`) ends at `p`: a letter in the run of scheme
    characters right before `p`, within the token. Runs before two `://` never overlap."""
    k = p
    while k > start and line[k - 1] in _SCHEME_CHARS:
        k -= 1
        if line[k].isalpha():
            return True
    return False


def _first_url_end(line, start):
    """End of the first `scheme://h` (a host character right after `://`) in the whitespace-
    delimited token that starts at `start`, or None. `file:///x` has no host, so it is not one.
    Reads the token only (never to the end of the line), so all tokens together stay linear."""
    ws = _TOKEN_END.search(line, start)
    end = ws.start() if ws else len(line)
    p = line.find("://", start, end)
    while p >= 0:
        if p + 3 < end and line[p + 3] != "/" and _has_scheme(line, start, p):
            return p + 4
        p = line.find("://", p + 1, end)
    return None


def _home_path_found(line):
    """True if `line` has `/Users/<name>` or `/home/<name>` as a segment of an absolute path,
    outside a URL with a host (`file:///Users/x` counts; `https://example.com/home/x` and
    `app/home/x` do not). Linear in the line: each backwards walk stops where the previous
    match's walk started, and each token's URL is looked up once."""
    prev_at = prev_run = -1        # the previous match's start and the start of its path run
    tok_at = tok_start = -1        # the previous match's start and the start of its token
    url_end = None                 # end of the first `scheme://h` in that token, or None
    for m in _HOME_PATH.finditer(line):
        name = m.group(1).rstrip(".")
        if not name or name == "Shared":
            continue
        i = j = m.start()
        while j > 0 and line[j - 1] in _PATH_CHARS:
            j -= 1
            if j == prev_at:       # joined the previous match's run: reuse its start
                j = prev_run
                break
        prev_at, prev_run = i, j
        if j < i and line[j] != "/":
            continue               # a relative path such as `src/home/view.py`
        lo = tok_at if tok_at >= 0 else 0
        ws = max(line.rfind(" ", lo, i), line.rfind("\t", lo, i))
        start = ws + 1 if ws >= 0 else (tok_start if tok_at >= 0 else 0)
        if start != tok_start or tok_at < 0:
            tok_start, url_end = start, _first_url_end(line, start)
        tok_at = i
        if url_end is None or url_end > i:
            return True
    return False


def _line_kinds(line, home_rx, host_rxs):
    kinds = set()
    if _home_path_found(line):
        kinds.add("home-path")
    if home_rx is not None and home_rx.search(line):
        kinds.add("home-path")
    for m in _EMAIL.finditer(line):
        if not _exempt_email(m.group(1), m.group(2)):
            kinds.add("email")
    for m in _IPV4.finditer(line):  # the look-behind skips `v1.2.3.4` and `1.2.3.4.5`
        kind = _ipv4_kind(m.groups())
        if kind:
            kinds.add(kind)
    for m in _IPV6.finditer(line):
        try:
            addr = ipaddress.IPv6Address(m.group(0).split("%", 1)[0])
        except ValueError:
            continue
        if any(addr in net for net in _PRIVATE_V6):
            kinds.add("private-ip")
    if _PRIVATE_HOST.search(line) or any(rx.search(line) for rx in host_rxs):
        kinds.add("private-hostname")
    if _SSH_COMMAND.search(line):  # the target may come before or after the command word
        for m in _USER_AT_HOST.finditer(line):
            if not _forge_git_user(m.group(1), m.group(2)) and not _reserved_host(m.group(2)):
                kinds.add("ssh-target")
                break
    if _SSH_PATH.search(line) or _KEY_NAME.search(line):
        kinds.add("ssh-key-path")
    for label, rx in SECRET_PATTERNS:
        if rx.search(line):
            kinds.add("secret:" + label)
    return kinds


def privacy_findings(text, home=None, hostname=None):
    """`[{"kind", "line"}]` for `text`, sorted by line then kind, one per (kind, line); never the
    matched text. `home=None` means `$HOME`; `hostname=None` means `socket.gethostname()`."""
    home_rx = _home_literal(home)
    host_rxs = _hostname_patterns(hostname)
    found = set()
    for number, line in enumerate(str(text).split("\n"), 1):
        for kind in _line_kinds(line, home_rx, host_rxs):
            found.add((number, kind))
    return [{"kind": kind, "line": number} for number, kind in sorted(found)]


def describe(findings):
    """Human-readable summary, e.g. `home path (line 3), email address (lines 7, 9)`."""
    order, lines = [], {}
    for f in findings:
        kind = f["kind"]
        if kind not in lines:
            order.append(kind)
            lines[kind] = []
        if f["line"] not in lines[kind]:
            lines[kind].append(f["line"])
    parts = []
    for kind in order:
        if kind.startswith("secret:"):
            label = "secret-looking %s" % kind[len("secret:"):]
        else:
            label = _LABELS.get(kind, kind)
        nums = lines[kind]
        where = "line %d" % nums[0] if len(nums) == 1 else "lines " + ", ".join(str(n) for n in nums)
        parts.append("%s (%s)" % (label, where))
    return ", ".join(parts)
