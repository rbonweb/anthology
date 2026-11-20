"""Privacy scan for notes that would be written into repo memory (which may be public).

`privacy_findings(text)` returns `[{"kind": str, "line": int}, ...]`: labels and 1-based line
numbers only, never the matched text. Every kind blocks a write into repo memory:

- `home-path`         `/Users/<name>` or `/home/<name>` anywhere in an absolute path (not in a
                      `scheme://host/...` URL), and the literal value of `$HOME`
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
module.
"""
from __future__ import annotations

import ipaddress
import os
import re
import socket

# The anthology-guardrails G6 patterns, copied exactly (same source and flags; a test checks it),
# then the export/import patterns, then a few more common token formats.
SECRET_PATTERNS = (
    ("aws-access-key", re.compile(r"(?:AKIA|ASIA)[0-9A-Z]{16}")),
    ("github-token", re.compile(r"\bgh[pousr]_[A-Za-z0-9]+")),
    ("github-token", re.compile(r"github_pat_")),
    ("api-key", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}")),
    ("slack-token", re.compile(r"\bxox[baprs]-")),
    ("private-key", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    ("credential-assignment", re.compile(r"password\s*[:=]", re.IGNORECASE)),
    ("credential-assignment", re.compile(r"api[_-]?key\s*[:=]", re.IGNORECASE)),
    ("aws-access-key", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("github-token", re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}")),
    ("github-token", re.compile(r"github_pat_[A-Za-z0-9_]{20,}")),
    ("slack-token", re.compile(r"xox[abprs]-[A-Za-z0-9-]{10,}")),
    ("api-key", re.compile(r"sk-(?:ant-)?[A-Za-z0-9_-]{20,}")),
    ("credential-assignment",
     re.compile(r"(password|passwd|secret|api[_-]?key|token)\s*[:=]\s*\S{8,}", re.I)),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")),
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

_HOME_PATH = re.compile(r"/(?i:users|home)/([A-Za-z0-9._-]+)")
_PATH_RUN = re.compile(r"[A-Za-z0-9._~$@%+/-]*\Z")  # the path characters just before a match
_URL_HOST = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://[^/\s]")  # `scheme://host`, not `file:///`
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


def _home_path_hit(line, m):
    """`/Users/<name>` or `/home/<name>` as a segment of an absolute path, outside a URL with a
    host (`file:///Users/x` counts; `https://example.com/home/x` and `app/home/x` do not)."""
    name = m.group(1).rstrip(".")
    if not name or name.lower() == "shared":
        return False
    run = _PATH_RUN.search(line, 0, m.start()).group(0)
    if run and not run.startswith("/"):
        return False  # a relative path such as `src/home/view.py`
    token_start = max(line.rfind(" ", 0, m.start()), line.rfind("\t", 0, m.start())) + 1
    return _URL_HOST.search(line, token_start, m.start()) is None


def _line_kinds(line, home_rx, host_rxs):
    kinds = set()
    if any(_home_path_hit(line, m) for m in _HOME_PATH.finditer(line)):
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
