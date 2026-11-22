# lib.sh: the Anthology bootstrap and shim library (design §8, §9; plan GC-14, GC-17).
# POSIX sh. Sourced by hooks/bootstrap.sh, hooks/guard.sh, hooks/session-start.sh and bin/*,
# and byte-identical in the anthology and anthology-guardrails plugins (test-enforced).
#
# Everything is a function (prefix anth_) or an ANTH_ variable. Nothing here ends the shell:
# the only `exit` is inside the download watchdog's background subshell. Callers decide how to
# end, so guard.sh keeps its fail-closed shape (exactly two `exit 0` sites, design §9.2).
#
# The caller sets the plugin root with `anth_paths <root>` first.
# shellcheck disable=SC2034 # the ANTH_ variables are read by the scripts that source this file

ANTH_TOKEN='anthology-guard:allow:v1'
ANTH_BUDGET=20
ANTH_PROBE_TIMEOUT=5
ANTH_MAX_BYTES=67108864
ANTH_TAG_PREFIX='anthology-bin--v'
ANTH_R_HOME='HOME or CLAUDE_CONFIG_DIR is not an absolute path'
ANTH_NL='
'
ANTH_P='; if this keeps happening, disable anthology-guardrails with /plugin'
ANTH_READONLY='Read Grep Glob LS NotebookRead WebFetch WebSearch TodoWrite TaskCreate TaskUpdate TaskGet TaskList TaskOutput TaskStop Task Agent Skill ToolSearch AskUserQuestion EnterPlanMode ExitPlanMode BashOutput KillShell ListAgents CronList SubagentHandback'
# The node downloader (design §8.4 step 6.5): https on every hop, at most 5 redirects, at most
# 64 MiB, its own 60 s total timeout. A fixed constant: a test compares it byte for byte.
ANTH_NODE_GET='const h=require("https"),f=require("fs"),[,u,o]=process.argv,M=67108864;let n=0;const q=c=>process.exit(c);setTimeout(()=>q(1),60000);function g(x){let p;try{p=new URL(x)}catch(e){return q(1)}if(p.protocol!=="https:")return q(1);h.get(p,{headers:{"user-agent":"anthology-bootstrap"}},r=>{const s=r.statusCode,l=r.headers.location;if(s>=300&&s<400&&l){r.resume();if(++n>5)return q(1);let t;try{t=new URL(l,p).href}catch(e){return q(1)}return g(t)}if(s!==200||Number(r.headers["content-length"]||0)>M){r.resume();return q(1)}let z=0;const w=f.createWriteStream(o);r.on("data",c=>{z+=c.length;if(z>M)q(1)});r.on("error",()=>q(1));r.on("aborted",()=>q(1));w.on("error",()=>q(1));w.on("close",()=>q(r.complete?0:1));r.pipe(w)}).on("error",()=>q(1))}g(u)'

# Every result variable starts empty, whatever the environment held.
ANTH_ROOT='' ANTH_LOCK='' ANTH_AH='' ANTH_BINDIR='' ANTH_VDIR='' ANTH_BIN=''
ANTH_OS='' ANTH_ARCH='' ANTH_TARGET='' ANTH_VERSION='' ANTH_TAG='' ANTH_REPO='' ANTH_SHA=''
ANTH_STATUS='' ANTH_REASON='' ANTH_STATE='' ANTH_WHY='' ANTH_MSG='' ANTH_RUN='' ANTH_BUF=''
ANTH_RC='' ANTH_ERR='' ANTH_H='' ANTH_SHA_CMD='' ANTH_STRIKES='' ANTH_QUAR='' ANTH_TMP=''

# --- paths (GC-1) -------------------------------------------------------------------------

# anth_paths ROOT: the plugin root, its lock, and <ah>/bin. <ah> is $CLAUDE_CONFIG_DIR/anthology
# when that is set, else $HOME/.claude/anthology, and it must be an absolute path: a relative
# one would pick a binary under the hook's working folder, which the hot path runs unhashed,
# and an unset or empty HOME is not a home (there is no passwd lookup here). Otherwise ANTH_AH
# and every path below it stay empty and anth_home_ok fails, as
# `anthology_core::paths::install_home` does.
anth_paths() {
    ANTH_ROOT=$1
    ANTH_LOCK=$1/anthology.lock
    if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then
        ANTH_AH=$CLAUDE_CONFIG_DIR/anthology
    elif [ -n "${HOME:-}" ]; then
        ANTH_AH=$HOME/.claude/anthology
    else
        ANTH_AH=
    fi
    case $ANTH_AH in
        /*) ANTH_BINDIR=$ANTH_AH/bin ;;
        *) ANTH_AH='' ANTH_BINDIR='' ;;
    esac
}

# anth_home_ok: <ah> is usable (after anth_paths).
anth_home_ok() {
    [ -n "$ANTH_AH" ]
}

# anth_vpaths: <ah>/bin/<version> and its binary (after anth_lock); both empty without <ah>.
anth_vpaths() {
    ANTH_VDIR='' ANTH_BIN=''
    anth_home_ok || return 0
    ANTH_VDIR=$ANTH_BINDIR/$ANTH_VERSION
    ANTH_BIN=$ANTH_VDIR/anthology
}

# --- platform (design §8.4 step 1) --------------------------------------------------------

# anth_clean TEXT MAX: TEXT with every character outside [A-Za-z0-9_.-] turned into `?`, cut to
# MAX characters (R11), so it is JSON-safe in a reason.
anth_clean() {
    printf '%s' "$1" | LC_ALL=C tr -c 'A-Za-z0-9_.-' '?' | cut -c "1-$2"
}

# anth_printable TEXT MAX: non-printable-ASCII characters turned into `?`, cut to MAX.
anth_printable() {
    printf '%s' "$1" | LC_ALL=C tr -c ' -~' '?' | cut -c "1-$2"
}

# anth_platform: ANTH_TARGET from uname (the atl table), or status 1 with ANTH_OS/ANTH_ARCH
# cleaned for the `unsupported platform <os>/<arch>` reason.
anth_platform() {
    ANTH_TARGET=
    ANTH_OS=$(uname -s 2>/dev/null)
    ANTH_ARCH=$(uname -m 2>/dev/null)
    case $ANTH_OS in
        [Dd][Aa][Rr][Ww][Ii][Nn])
            case $ANTH_ARCH in
                [Aa][Rr][Mm]64 | [Aa][Aa][Rr][Cc][Hh]64) ANTH_TARGET=aarch64-apple-darwin ;;
                [Xx]86_64) ANTH_TARGET=x86_64-apple-darwin ;;
            esac
            ;;
        [Ll][Ii][Nn][Uu][Xx])
            case $ANTH_ARCH in
                [Xx]86_64 | [Aa][Mm][Dd]64) ANTH_TARGET=x86_64-unknown-linux-musl ;;
                [Aa][Aa][Rr][Cc][Hh]64 | [Aa][Rr][Mm]64) ANTH_TARGET=aarch64-unknown-linux-musl ;;
            esac
            ;;
    esac
    [ -z "$ANTH_TARGET" ] || return 0
    ANTH_OS=$(anth_clean "$ANTH_OS" 32)
    ANTH_ARCH=$(anth_clean "$ANTH_ARCH" 32)
    return 1
}

# --- anthology.lock (design §8.2, GC-17) --------------------------------------------------

# anth_only TEXT CHARS: TEXT is non-empty and made only of CHARS (a bracket-expression body).
anth_only() {
    case $1 in
        '') return 1 ;;
    esac
    eval "case \$1 in *[!$2]*) return 1 ;; esac"
    return 0
}

anth_version_ok() {
    _v_rest=$1
    for _v_part in 1 2; do
        _v_num=${_v_rest%%.*}
        [ "$_v_num" != "$_v_rest" ] || return 1
        anth_only "$_v_num" 0123456789 || return 1
        _v_rest=${_v_rest#*.}
    done
    _v_num=${_v_rest%%-*}
    anth_only "$_v_num" 0123456789 || return 1
    [ "$_v_num" != "$_v_rest" ] || return 0
    anth_only "${_v_rest#*-}" 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.
}

anth_repo_ok() {
    _r_owner=${1%%/*}
    _r_name=${1#*/}
    [ "$_r_owner" != "$1" ] || return 1
    for _r_part in "$_r_owner" "$_r_name"; do
        anth_only "$_r_part" 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_.- ||
            return 1
        case $_r_part in
            . | ..) return 1 ;;
        esac
    done
    return 0
}

anth_hex64() {
    [ "${#1}" -eq 64 ] && anth_only "$1" 0123456789abcdef
}

# anth_lock: reads and validates $ANTH_LOCK in the exact layout `anthology-dev lock write`
# produces (with or without the final newline); sets ANTH_VERSION, ANTH_TAG, ANTH_REPO and
# ANTH_SHA (this target's hash). Needs ANTH_TARGET.
anth_lock() {
    ANTH_VERSION='' ANTH_TAG='' ANTH_REPO='' ANTH_SHA=''
    [ -f "$ANTH_LOCK" ] && [ -r "$ANTH_LOCK" ] || return 1
    _l_n=0 _l_ok=1 _l_bytes=0 _l_v='' _l_r=''
    while :; do
        _l_line=
        if IFS= read -r _l_line; then
            _l_eol=1
        else
            [ -n "$_l_line" ] || break
            _l_eol=0
        fi
        _l_n=$((_l_n + 1))
        _l_bytes=$((_l_bytes + ${#_l_line} + _l_eol))
        case $_l_n in
            1) [ "$_l_line" = '{' ] || _l_ok=0 ;;
            2)
                case $_l_line in
                    '  "version": "'*'",')
                        _l_v=${_l_line#'  "version": "'}
                        _l_v=${_l_v%'",'}
                        ;;
                    *) _l_ok=0 ;;
                esac
                ;;
            3) [ "$_l_line" = "  \"tag\": \"$ANTH_TAG_PREFIX$_l_v\"," ] || _l_ok=0 ;;
            4)
                case $_l_line in
                    '  "repo": "'*'",')
                        _l_r=${_l_line#'  "repo": "'}
                        _l_r=${_l_r%'",'}
                        ;;
                    *) _l_ok=0 ;;
                esac
                ;;
            5) [ "$_l_line" = '  "sha256": {' ] || _l_ok=0 ;;
            6 | 7 | 8 | 9)
                case $_l_n in
                    6) _l_t=aarch64-apple-darwin _l_end=, ;;
                    7) _l_t=x86_64-apple-darwin _l_end=, ;;
                    8) _l_t=x86_64-unknown-linux-musl _l_end=, ;;
                    *) _l_t=aarch64-unknown-linux-musl _l_end= ;;
                esac
                _l_h=${_l_line#"    \"$_l_t\": \""}
                _l_h=${_l_h%"\"$_l_end"}
                if [ "$_l_line" = "    \"$_l_t\": \"$_l_h\"$_l_end" ] && anth_hex64 "$_l_h"; then
                    [ "$_l_t" != "$ANTH_TARGET" ] || ANTH_SHA=$_l_h
                else
                    _l_ok=0
                fi
                ;;
            10) [ "$_l_line" = '  }' ] || _l_ok=0 ;;
            11) [ "$_l_line" = '}' ] || _l_ok=0 ;;
            *) _l_ok=0 ;;
        esac
        [ "$_l_ok" = 1 ] && [ "$_l_eol" = 1 ] || break
    done <"$ANTH_LOCK"
    # The byte count catches what `read` hides (a NUL byte, for one).
    if [ "$_l_ok" = 1 ] && [ "$_l_n" = 11 ] && anth_version_ok "$_l_v" && anth_repo_ok "$_l_r" &&
        [ -n "$ANTH_SHA" ] && anth_size "$ANTH_LOCK" && [ "$ANTH_H" = "$_l_bytes" ]; then
        ANTH_VERSION=$_l_v
        ANTH_TAG=$ANTH_TAG_PREFIX$_l_v
        ANTH_REPO=$_l_r
        return 0
    fi
    ANTH_SHA=
    return 1
}

# anth_resolve ROOT: paths, platform and lock; status 0 when all three are usable.
anth_resolve() {
    anth_paths "$1"
    anth_platform || return 1
    anth_lock || return 1
    anth_home_ok || return 1
    anth_vpaths
}

# anth_url: the release asset URL (GC-17).
anth_url() {
    printf 'https://github.com/%s/releases/download/%s/anthology-%s.tar.gz' \
        "$ANTH_REPO" "$ANTH_TAG" "$ANTH_TARGET"
}

# --- install state (design §8.3) ----------------------------------------------------------

# anth_installed: the guard hot path's "installed" (builtins only, no hashing).
anth_installed() {
    [ -d "$ANTH_BINDIR" ] && [ ! -h "$ANTH_BINDIR" ] &&
        [ -d "$ANTH_VDIR" ] && [ ! -h "$ANTH_VDIR" ] &&
        [ -f "$ANTH_BIN" ] && [ ! -h "$ANTH_BIN" ] && [ -x "$ANTH_BIN" ] && [ -s "$ANTH_BIN" ] &&
        [ -s "$ANTH_VDIR/.sha256" ]
}

# anth_bin_unusable: a <version>/anthology exists but must never run; ANTH_WHY says why.
anth_bin_unusable() {
    ANTH_WHY=
    [ -e "$ANTH_BIN" ] || [ -h "$ANTH_BIN" ] || return 1
    if [ -h "$ANTH_BIN" ] || [ ! -f "$ANTH_BIN" ]; then
        ANTH_WHY='it is not executable'
    elif [ ! -s "$ANTH_BIN" ]; then
        ANTH_WHY='it is empty'
    elif [ ! -x "$ANTH_BIN" ]; then
        ANTH_WHY='it is not executable'
    else
        return 1
    fi
    return 0
}

# anth_size FILE: ANTH_H = its size in bytes (digits only), or status 1.
anth_size() {
    ANTH_H=$(wc -c <"$1" 2>/dev/null) || ANTH_H=
    ANTH_H=${ANTH_H##*[!0123456789]}
    anth_only "$ANTH_H" 0123456789
}

# anth_first_line FILE: ANTH_H = the first line of FILE (empty if unreadable).
anth_first_line() {
    ANTH_H=
    [ -f "$1" ] || return 1
    IFS= read -r ANTH_H <"$1" || [ -n "$ANTH_H" ]
}

# anth_sha_tool: ANTH_SHA_CMD = sha256sum, shasum or openssl (design §8.4 step 7).
anth_sha_tool() {
    ANTH_SHA_CMD=
    for _t_cmd in sha256sum shasum openssl; do
        if command -v "$_t_cmd" >/dev/null 2>&1; then
            ANTH_SHA_CMD=$_t_cmd
            return 0
        fi
    done
    return 1
}

# anth_sha256 FILE: ANTH_H = its sha256 (64 lowercase hex), or status 1.
anth_sha256() {
    ANTH_H=
    case $ANTH_SHA_CMD in
        sha256sum) _h_out=$(sha256sum <"$1" 2>/dev/null) ;;
        shasum) _h_out=$(shasum -a 256 <"$1" 2>/dev/null) ;;
        openssl) _h_out=$(openssl dgst -sha256 -r <"$1" 2>/dev/null) ;;
        *) return 1 ;;
    esac || return 1
    _h_out=${_h_out%% *}
    anth_hex64 "$_h_out" || return 1
    ANTH_H=$_h_out
}

# anth_current: installed, .sha256 is the lock's hash, .bin-sha256 is the binary's hash.
anth_current() {
    anth_installed || return 1
    anth_first_line "$ANTH_VDIR/.sha256" && [ "$ANTH_H" = "$ANTH_SHA" ] || return 1
    anth_first_line "$ANTH_VDIR/.bin-sha256" || return 1
    _c_want=$ANTH_H
    anth_sha256 "$ANTH_BIN" && [ "$ANTH_H" = "$_c_want" ]
}

# --- quarantine and strikes (design §9.2, m-R1, m2-2) -------------------------------------

# anth_broken_name: ANTH_H = a free .broken-<version>-<epoch>-<pid>[-<n>] path.
anth_broken_name() {
    _b_epoch=$(date +%s 2>/dev/null)
    anth_only "$_b_epoch" 0123456789 || _b_epoch=0
    ANTH_H=$ANTH_BINDIR/.broken-$ANTH_VERSION-$_b_epoch-$$
    _b_n=1
    while [ -e "$ANTH_H" ] || [ -h "$ANTH_H" ]; do
        _b_n=$((_b_n + 1))
        ANTH_H=$ANTH_BINDIR/.broken-$ANTH_VERSION-$_b_epoch-$$-$_b_n
    done
}

# anth_quarantine: moves <version> aside. ANTH_QUAR = moved, gone (a parallel call moved it
# first) or failed (the mv failed and <version> is still there).
anth_quarantine() {
    anth_broken_name
    if mv "$ANTH_VDIR" "$ANTH_H" 2>/dev/null; then
        ANTH_QUAR=moved
    elif [ -e "$ANTH_VDIR" ] || [ -h "$ANTH_VDIR" ]; then
        ANTH_QUAR=failed
    else
        ANTH_QUAR=gone
    fi
}

# anth_decimal S: S is a positive canonical decimal: digits, the first one not 0.
anth_decimal() {
    anth_only "$1" 0123456789 || return 1
    case $1 in
        0*) return 1 ;;
    esac
    return 0
}

# anth_strikes: ANTH_STRIKES = .broken-<version>-<epoch>-* directories whose name epoch is
# within 86,400 s of now. Only the name counts, never an mtime.
anth_strikes() {
    ANTH_STRIKES=0
    _s_now=$(date +%s 2>/dev/null)
    anth_only "$_s_now" 0123456789 || return 0
    for _s_d in "$ANTH_BINDIR"/.broken-"$ANTH_VERSION"-*; do
        [ -d "$_s_d" ] || continue
        _s_rest=${_s_d##*/}
        _s_rest=${_s_rest#".broken-$ANTH_VERSION-"}
        # Exactly the shapes anth_broken_name makes, <epoch>-<pid>[-<n>], each a canonical
        # decimal. Anything else (a prerelease's quarantine, a hand-made name) is no strike. A
        # leading zero is rejected before the arithmetic: as an octal error it would end the
        # caller's shell (guard.sh must only ever exit 0 or 2).
        _s_epoch=${_s_rest%%-*}
        _s_pid=${_s_rest#*-}
        [ "$_s_pid" != "$_s_rest" ] || continue
        _s_n=${_s_pid#*-}
        [ "$_s_n" != "$_s_pid" ] || _s_n=2
        _s_pid=${_s_pid%%-*}
        if ! anth_decimal "$_s_epoch" || ! anth_decimal "$_s_pid" || ! anth_decimal "$_s_n"; then
            continue
        fi
        [ "${#_s_epoch}" -le 12 ] || continue
        _s_age=$((_s_now - _s_epoch))
        if [ "$_s_age" -lt 86400 ] && [ "$_s_age" -gt -86400 ]; then
            ANTH_STRIKES=$((ANTH_STRIKES + 1))
        fi
    done
}

# --- the watchdog (design §8.4, m-R4) -----------------------------------------------------

# anth_watch SECONDS CMD...: runs CMD (an external command, so `kill` reaches it) with stdin
# from /dev/null and kills it after SECONDS. The watchdog holds no caller descriptor, and a
# fast CMD returns at once.
anth_watch() {
    _w_limit=$1
    shift
    "$@" </dev/null &
    _w_p=$!
    # bootstrap.sh's EXIT trap removes $ANTH_TMP, and a fast-failing probe can send TERM to the
    # watcher before it has reset the traps it inherited: bash then runs that trap in the
    # subshell and deletes the staging folder under the parent. The subshell sees it empty.
    _w_t=$ANTH_TMP ANTH_TMP=
    (
        exec </dev/null >/dev/null 2>&1
        _w_s=
        trap 'kill "$_w_s" 2>/dev/null; exit 0' TERM
        sleep "$_w_limit" &
        _w_s=$!
        wait "$_w_s"
        kill -9 "$_w_p" 2>/dev/null
    ) &
    _w_w=$!
    ANTH_TMP=$_w_t
    wait "$_w_p" 2>/dev/null
    _w_rc=$?
    kill "$_w_w" 2>/dev/null
    wait "$_w_w" 2>/dev/null
    return "$_w_rc"
}

# --- getting the archive (design §8.4 step 6) ---------------------------------------------

# anth_download URL OUT BUDGET: one downloader, the first present of curl, GNU wget, BusyBox
# wget and node, with the whole budget (R7). Status 0 ok, 1 failed, 2 no downloader.
anth_download() {
    if command -v curl >/dev/null 2>&1; then
        anth_watch "$3" curl -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 10 \
            --max-time "$3" --max-filesize "$ANTH_MAX_BYTES" -A anthology-bootstrap \
            -o "$2" "$1" >/dev/null 2>&1 || return 1
        return 0
    fi
    if command -v wget >/dev/null 2>&1; then
        if wget --version 2>/dev/null | grep -q 'GNU Wget'; then
            anth_watch "$3" wget --https-only -q -T 10 -U anthology-bootstrap -O "$2" "$1" \
                >/dev/null 2>&1 || return 1
            return 0
        fi
        if wget --help 2>&1 | grep -q BusyBox; then
            anth_watch "$3" wget -q -T 10 -O "$2" "$1" >/dev/null 2>&1 || return 1
            return 0
        fi
    fi
    if command -v node >/dev/null 2>&1; then
        anth_watch "$3" env NODE_USE_ENV_PROXY=1 node -e "$ANTH_NODE_GET" "$1" "$2" \
            >/dev/null 2>&1 || return 1
        return 0
    fi
    return 2
}

# anth_no_downloader_reason: the GC-14 reason with the manual command.
anth_no_downloader_reason() {
    ANTH_REASON="no downloader (curl, wget or node); download $(anth_url) into the anthology/bin folder of your Claude config and start a new session"
}

# anth_cannot_write: the reason when the install folder cannot be written.
anth_cannot_write() {
    ANTH_REASON='cannot write the install folder'
    return 1
}

# anth_probe FILE TMP: the --version probe under the watchdog (N-I1, R8).
anth_probe() {
    anth_watch "$ANTH_PROBE_TIMEOUT" "$1" --version >"$2/version" 2>/dev/null || return 1
    _p_want="anthology $ANTH_VERSION ($ANTH_TARGET)"
    _p_line=
    IFS= read -r _p_line <"$2/version" || return 1
    [ "$_p_line" = "$_p_want" ] && anth_size "$2/version" && [ "$ANTH_H" = $((${#_p_want} + 1)) ]
}

# anth_extract TMP: $TMP/x/anthology from $TMP/a.tgz, chmod 0755; status 1 if the archive has
# no regular, non-empty `anthology` member.
anth_extract() {
    rm -rf "$1/x"
    mkdir "$1/x" 2>/dev/null || return 1
    tar -xzf "$1/a.tgz" -C "$1/x" anthology >/dev/null 2>&1 || return 1
    [ -f "$1/x/anthology" ] && [ ! -h "$1/x/anthology" ] && [ -s "$1/x/anthology" ] || return 1
    chmod 0755 "$1/x/anthology" 2>/dev/null
}

# anth_place TMP BINSHA: binary, then .bin-sha256, then .sha256 last (temp file + mv each).
anth_place() {
    [ ! -h "$ANTH_VDIR" ] || return 1
    if [ ! -d "$ANTH_VDIR" ]; then
        mkdir "$ANTH_VDIR" 2>/dev/null || [ -d "$ANTH_VDIR" ] || return 1
        [ ! -h "$ANTH_VDIR" ] || return 1
    fi
    if [ -e "$ANTH_BIN" ] || [ -h "$ANTH_BIN" ]; then
        [ -f "$ANTH_BIN" ] && [ ! -h "$ANTH_BIN" ] || return 1
    fi
    mv -f "$1/x/anthology" "$ANTH_BIN" 2>/dev/null || return 1
    printf '%s\n' "$2" >"$1/bin-sha256" 2>/dev/null &&
        mv -f "$1/bin-sha256" "$ANTH_VDIR/.bin-sha256" 2>/dev/null || return 1
    printf '%s\n' "$ANTH_SHA" >"$1/sha256" 2>/dev/null &&
        mv -f "$1/sha256" "$ANTH_VDIR/.sha256" 2>/dev/null || return 1
    anth_installed
}

# anth_install_in TMP BUDGET: steps 6 to 10 inside the staging folder TMP.
anth_install_in() {
    if ! anth_sha_tool; then
        ANTH_REASON='no sha256 tool'
        return 1
    fi
    _i_seed=$ANTH_BINDIR/anthology-$ANTH_TARGET.tar.gz
    _i_got='' _i_stale=''
    if [ -f "$_i_seed" ]; then
        if cp "$_i_seed" "$1/a.tgz" 2>/dev/null && anth_sha256 "$1/a.tgz" &&
            [ "$ANTH_H" = "$ANTH_SHA" ]; then
            _i_got=1
        else
            # A seed that does not match the pin is not used and not deleted (m-R3).
            _i_stale=1
            rm -f "$1/a.tgz"
        fi
    fi
    if [ -z "$_i_got" ]; then
        anth_download "$(anth_url)" "$1/a.tgz" "$2"
        case $? in
            0)
                if anth_sha256 "$1/a.tgz" && [ "$ANTH_H" = "$ANTH_SHA" ]; then
                    _i_got=1
                else
                    ANTH_REASON='sha256 does not match anthology.lock'
                fi
                ;;
            2) anth_no_downloader_reason ;;
            *) ANTH_REASON='download failed' ;;
        esac
        if [ -z "$_i_got" ]; then
            [ -z "$_i_stale" ] ||
                ANTH_REASON="seed file anthology-$ANTH_TARGET.tar.gz does not match anthology.lock"
            return 1
        fi
    fi
    if ! anth_extract "$1"; then
        ANTH_REASON='the archive has no anthology binary'
        return 1
    fi
    if ! anth_sha256 "$1/x/anthology"; then
        ANTH_REASON='no sha256 tool'
        return 1
    fi
    _i_binsha=$ANTH_H
    if ! anth_probe "$1/x/anthology" "$1"; then
        # The staged binary is set aside like a quarantine, so it counts as a strike (m2-1).
        _i_n=0
        while :; do
            anth_broken_name
            if mkdir "$ANTH_H" 2>/dev/null; then
                mv -f "$1/x/anthology" "$ANTH_H/anthology" 2>/dev/null
                break
            fi
            _i_n=$((_i_n + 1))
            [ "$_i_n" -lt 3 ] || break
        done
        ANTH_REASON='the binary does not run on this machine'
        return 1
    fi
    anth_place "$1" "$_i_binsha" && return 0
    [ -w "$ANTH_BINDIR" ] || anth_cannot_write || return 1
    # Something else changed <version> under us (a parallel quarantine): one more try (m2-2).
    if anth_extract "$1" && anth_place "$1" "$_i_binsha"; then
        return 0
    fi
    [ -w "$ANTH_BINDIR" ] || anth_cannot_write || return 1
    ANTH_REASON='install interrupted by a parallel change'
    return 1
}

# anth_install BUDGET: a fresh staging folder in <ah>/bin, removed on every path.
anth_install() {
    ANTH_TMP=$(mktemp -d "$ANTH_BINDIR/.tmp-XXXXXX" 2>/dev/null) || ANTH_TMP=
    if [ -z "$ANTH_TMP" ] || [ ! -d "$ANTH_TMP" ]; then
        ANTH_TMP=
        anth_cannot_write
        return 1
    fi
    anth_install_in "$ANTH_TMP" "$1"
    _n_rc=$?
    rm -rf "$ANTH_TMP"
    ANTH_TMP=
    return "$_n_rc"
}

# --- rate limit and cap (design §8.4 step 5, A28) -----------------------------------------

# anth_fresh FILE MINUTES: FILE exists and was modified less than MINUTES minutes ago.
anth_fresh() {
    [ -e "$1" ] || [ -h "$1" ] || return 1
    [ -n "$(find "$1" -prune -mmin "-$2" 2>/dev/null)" ]
}

# anth_stamp FILE MINUTES REASON: the noclobber stamp. A fresh stamp stops with REASON; a
# stale one is retired atomically (only one racer's rename finds it) and a new one created.
#
# Parallel calls race here: between one call's freshness test and its retire, another can
# create a fresh stamp, which the first then retires; a third call's create then fails while
# no stamp exists. So a failed create with no fresh stamp is tried again, up to five times,
# before it is "cannot write". In a folder that cannot be written every try fails at once, so
# a stale stamp there is still "cannot write", never REASON forever (m2-3).
anth_stamp() {
    _t_try=0
    while [ "$_t_try" -lt 5 ]; do
        _t_try=$((_t_try + 1))
        if anth_fresh "$1" "$2"; then
            ANTH_REASON=$3
            return 1
        fi
        if [ -e "$1" ] || [ -h "$1" ]; then
            mv -f "$1" "$1.stale.$$" 2>/dev/null && rm -f "$1.stale.$$"
        fi
        if (set -C && : >"$1") 2>/dev/null; then
            return 0
        fi
        # A failed create is REASON only when the stamp is fresh.
        if anth_fresh "$1" "$2"; then
            ANTH_REASON=$3
            return 1
        fi
    done
    anth_cannot_write
}

# --- the bootstrap (design §8.4) ----------------------------------------------------------

# anth_bootstrap MODE BUDGET: ANTH_STATUS = present or installed (status 0), or ANTH_REASON
# (status 1). Needs anth_paths.
anth_bootstrap() {
    ANTH_STATUS='' ANTH_REASON=''
    if ! anth_platform; then
        ANTH_REASON="unsupported platform $ANTH_OS/$ANTH_ARCH"
        return 1
    fi
    if ! anth_lock; then
        ANTH_REASON='anthology.lock is not valid'
        return 1
    fi
    if ! anth_home_ok; then
        ANTH_REASON=$ANTH_R_HOME
        return 1
    fi
    anth_vpaths
    # 3. The install folder; a symlink is refused (m-R2).
    [ ! -h "$ANTH_BINDIR" ] || anth_cannot_write || return 1
    mkdir -p "$ANTH_BINDIR" 2>/dev/null
    [ -d "$ANTH_BINDIR" ] && [ ! -h "$ANTH_BINDIR" ] || anth_cannot_write || return 1
    # 4. Current? A partial <version> (no .sha256) is installed over in place (R6).
    if [ -e "$ANTH_VDIR" ] || [ -h "$ANTH_VDIR" ]; then
        _a_aside=
        if [ -h "$ANTH_VDIR" ] || [ ! -d "$ANTH_VDIR" ]; then
            _a_aside=1
        elif [ -s "$ANTH_VDIR/.sha256" ]; then
            if ! anth_sha_tool; then
                ANTH_REASON='no sha256 tool'
                return 1
            fi
            if anth_current; then
                touch "$ANTH_VDIR/.last-used" 2>/dev/null
                ANTH_STATUS=present
                return 0
            fi
            _a_aside=1
        elif [ -h "$ANTH_BIN" ] || { [ -e "$ANTH_BIN" ] && [ ! -f "$ANTH_BIN" ]; }; then
            _a_aside=1
        fi
        if [ -n "$_a_aside" ]; then
            anth_quarantine
            if [ "$ANTH_QUAR" = failed ] && [ ! -w "$ANTH_BINDIR" ]; then
                anth_cannot_write
                return 1
            fi
        fi
    fi
    # 5. The strike cap and the rate limit.
    anth_strikes
    case $1 in
        session)
            # Never .attempt; capped, except one fresh attempt per hour (A28, m2-6).
            if [ "$ANTH_STRIKES" -ge 3 ]; then
                anth_stamp "$ANTH_BINDIR/.session-retry" 60 three-strikes || return 1
            fi
            ;;
        *)
            if [ "$ANTH_STRIKES" -ge 3 ]; then
                ANTH_REASON=three-strikes
                return 1
            fi
            anth_stamp "$ANTH_BINDIR/.attempt" 1 rate-limited || return 1
            ;;
    esac
    # 6 to 10.
    anth_install "$2" || return 1
    ANTH_STATUS=installed
    return 0
}

# anth_system_message PLUGIN REASON: the bootstrap's GC-14 systemMessage line.
anth_system_message() {
    if [ "$1" = guardrails ]; then
        printf '{"systemMessage":"anthology-guardrails: could not install the guard binary (%s); shell commands, file edits and MCP tools are blocked until it installs. Start a new session while online, or run /anthology:setup; if this keeps happening, disable anthology-guardrails with /plugin."}\n' "$2"
    else
        printf '{"systemMessage":"anthology: could not install the anthology binary (%s); the routing guide, memory and atl are off until it installs. Start a new session while online, or run /anthology:setup."}\n' "$2"
    fi
}

# anth_bootstrap_main ARGS: the bootstrap.sh CLI (GC-17, GC-5). Needs anth_paths.
#   <session|guard|wrapper> --plugin <anthology|guardrails> [--quiet] [--budget SECONDS]
anth_bootstrap_main() {
    _m_mode='' _m_plugin='' _m_quiet='' _m_budget=$ANTH_BUDGET _m_bad=''
    case ${1:-} in
        session | guard | wrapper) _m_mode=$1 ;;
        *) _m_bad=1 ;;
    esac
    [ "$#" -eq 0 ] || shift
    while [ "$#" -gt 0 ] && [ -z "$_m_bad" ]; do
        case $1 in
            --plugin)
                case ${2:-} in
                    anthology | guardrails) _m_plugin=$2 ;;
                    *) _m_bad=1 ;;
                esac
                [ "$#" -lt 2 ] || shift
                ;;
            --quiet) _m_quiet=1 ;;
            --budget)
                case ${2:-} in
                    '' | *[!0-9]* | 0*) _m_bad=1 ;;
                    *) _m_budget=$2 ;;
                esac
                [ "$#" -lt 2 ] || shift
                ;;
            *) _m_bad=1 ;;
        esac
        shift
    done
    if [ -n "$_m_bad" ] || [ -z "$_m_plugin" ] || [ "${#_m_budget}" -gt 5 ]; then
        printf '%s\n' 'usage: bootstrap.sh <session|guard|wrapper> --plugin <anthology|guardrails> [--quiet] [--budget SECONDS]' >&2
        return 2
    fi
    anth_bootstrap "$_m_mode" "$_m_budget"
    _m_rc=$?
    if [ -n "$_m_quiet" ] || [ "$_m_mode" != session ]; then
        if [ "$_m_rc" -eq 0 ]; then
            printf 'status=%s\n' "$ANTH_STATUS"
            return 0
        fi
        printf 'status=failed:%s\n' "$ANTH_REASON"
        return 1
    fi
    [ "$_m_rc" -eq 0 ] || anth_system_message "$_m_plugin" "$ANTH_REASON"
    return 0
}

# --- the guard shim's pieces (design §9.2; guard.sh holds the decisions) ------------------

anth_msg_not_installed() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary is not installed ($1), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it$ANTH_P"
}

anth_msg_three_strikes() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary $ANTH_VERSION failed to run 3 times in 24 hours, so tool calls no longer reinstall it; a new session tries once more at most once an hour, or disable anthology-guardrails with /plugin until a fixed version ships"
}

anth_msg_cannot_write() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary cannot be installed because $ANTH_BINDIR is not writable; fix that folder's permissions, then start a new session$ANTH_P"
}

anth_msg_unsupported() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): there is no guard binary for this platform ($ANTH_OS/$ANTH_ARCH); Anthology supports macOS and Linux on x86_64 and arm64. Disable anthology-guardrails (/plugin) to use Claude Code here"
}

anth_msg_home() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary cannot be found because $ANTH_R_HOME; set it to an absolute path, then start a new session$ANTH_P"
}

anth_msg_lock() {
    ANTH_MSG="Blocked by anthology-guardrails (setup): anthology.lock in the anthology-guardrails plugin is not valid; reinstall anthology-guardrails$ANTH_P"
}

# anth_msg_reason REASON: the shim's block for a failed bootstrap.
anth_msg_reason() {
    case $1 in
        rate-limited)
            ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary is not installed yet and the last install attempt was less than a minute ago; retry in a minute, or run /anthology:setup$ANTH_P"
            ;;
        three-strikes) anth_msg_three_strikes ;;
        'cannot write the install folder') anth_msg_cannot_write ;;
        'no downloader '*)
            ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary is not installed and no downloader (curl, wget or node) was found; install it by hand: download $(anth_url) into $ANTH_BINDIR/ and start a new session; or disable anthology-guardrails with /plugin"
            ;;
        'the binary does not run on this machine')
            ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary $ANTH_VERSION does not run on this machine (wrong architecture, or a home folder mounted noexec); disable anthology-guardrails with /plugin, then fix the cause and start a new session"
            ;;
        'anthology.lock is not valid') anth_msg_lock ;;
        "$ANTH_R_HOME") anth_msg_home ;;
        'unsupported platform '*) anth_msg_unsupported ;;
        *) anth_msg_not_installed "$1" ;;
    esac
}

# anth_quarantine_msg WHY: quarantine <version>, then the matching block (m2-2, m2-5).
anth_quarantine_msg() {
    anth_quarantine
    if [ "$ANTH_QUAR" = failed ]; then
        ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary did not work ($1) and could not be set aside because $ANTH_BINDIR is not writable; fix that folder's permissions, then start a new session$ANTH_P"
        return 0
    fi
    anth_strikes
    if [ "$ANTH_STRIKES" -ge 3 ]; then
        anth_msg_three_strikes
    else
        ANTH_MSG="Blocked by anthology-guardrails (setup): the guard binary did not work ($1) and was set aside; it is reinstalled on a later call (at most once a minute), or run /anthology:setup$ANTH_P"
    fi
}

# anth_guard_state ROOT: ANTH_STATE = hot (installed), broken (a binary that must not run;
# ANTH_WHY), missing, or closed (platform unsupported, lock invalid or no <ah>; ANTH_MSG). No binary
# runs here.
anth_guard_state() {
    ANTH_STATE='' ANTH_MSG='' ANTH_WHY='' ANTH_RUN=''
    anth_paths "$1"
    if ! anth_platform; then
        anth_msg_unsupported
        ANTH_STATE=closed
        return 0
    fi
    if ! anth_lock; then
        anth_msg_lock
        ANTH_STATE=closed
        return 0
    fi
    if ! anth_home_ok; then
        anth_msg_home
        ANTH_STATE=closed
        return 0
    fi
    anth_vpaths
    if anth_installed; then
        ANTH_STATE=hot
    elif anth_bin_unusable; then
        ANTH_STATE=broken
    else
        ANTH_STATE=missing
    fi
    return 0
}

# anth_guard_buffer: ANTH_BUF = a temp file holding stdin; status 1 if that failed.
anth_guard_buffer() {
    ANTH_BUF=$(mktemp "${TMPDIR:-/tmp}/anthology-guard.XXXXXX" 2>/dev/null) || ANTH_BUF=
    [ -n "$ANTH_BUF" ] && [ -f "$ANTH_BUF" ] || {
        ANTH_BUF=
        return 1
    }
    cat >"$ANTH_BUF" 2>/dev/null && return 0
    rm -f "$ANTH_BUF"
    ANTH_BUF=
    return 1
}

# anth_tool_name FILE: prints the value of the payload's only "tool_name" key when it is a
# plain string of letters (design §9.3); prints nothing when there are none, several, or the
# value is anything else.
anth_tool_name() {
    LC_ALL=C tr '\n\r\000' '   ' <"$1" 2>/dev/null | LC_ALL=C awk '
        { s = s $0 }
        END {
            n = 0
            v = ""
            while (match(s, /"tool_name"[ \t]*:/)) {
                n++
                s = substr(s, RSTART + RLENGTH)
                if (n == 1) {
                    r = s
                    sub(/^[ \t]*/, "", r)
                    if (match(r, /^"[A-Za-z]+"/)) v = substr(r, 2, RLENGTH - 2)
                }
            }
            if (n == 1 && v != "") print v
        }' 2>/dev/null
}

# anth_readonly FILE: status 0 when the payload's tool is on the read-only allowlist (§9.4).
anth_readonly() {
    _o_name=$(anth_tool_name "$1")
    [ -n "$_o_name" ] || return 1
    case " $ANTH_READONLY " in
        *" $_o_name "*) return 0 ;;
    esac
    return 1
}

# anth_guard_offline: for a non-hot state and a non-read-only call. ANTH_RUN=1 when the
# binary is now installed and must run on the buffer; otherwise ANTH_MSG is the block (empty:
# the caller's final block).
anth_guard_offline() {
    ANTH_RUN=
    case $ANTH_STATE in
        closed) return 0 ;;
        broken)
            anth_quarantine_msg "$ANTH_WHY"
            return 0
            ;;
        missing) ;;
        *)
            ANTH_MSG=
            return 0
            ;;
    esac
    anth_strikes
    if [ "$ANTH_STRIKES" -ge 3 ]; then
        anth_msg_three_strikes
        return 0
    fi
    if [ -n "${ANTHOLOGY_GUARD_NO_FETCH:-}" ]; then
        anth_msg_not_installed 'not attempted'
        return 0
    fi
    _g_st=$(anth_bootstrap_main guard --plugin guardrails --quiet </dev/null 2>/dev/null)
    case $_g_st in
        status=present | status=installed)
            if anth_installed; then
                ANTH_RUN=1
            else
                ANTH_MSG=
            fi
            ;;
        status=failed:?*) anth_msg_reason "${_g_st#status=failed:}" ;;
        *) ANTH_MSG= ;;
    esac
    return 0
}

# anth_guard_run [FILE]: runs the binary as `guard hook --shim` (stdin from FILE, else
# inherited; stdout to /dev/null). ANTH_RC = its status, ANTH_ERR = its stderr, byte for byte
# (no trailing newline stripped: the sentinel keeps them). Command substitution would drop a
# NUL, so every NUL becomes \001 first: stray bytes can never complete the token. The first
# command of the pipeline still inherits stdin. ANTH_RC is digits only, else ? (a late
# writer after the sentinel). The binary runs in a subshell, so the shell's own notes, such as
# a report that it was killed by a signal, go to /dev/null, not into ANTH_ERR or the hook's
# stderr.
anth_guard_run() {
    ANTH_RC='' ANTH_ERR=''
    if [ -n "${1:-}" ]; then
        ANTH_ERR=$({
            ("$ANTH_BIN" guard hook --shim 2>&3 3>&- >/dev/null <"$1")
            printf 'x%s' "$?" >&3
        } 3>&1 2>/dev/null | LC_ALL=C tr '\000' '\001')
    else
        ANTH_ERR=$({
            ("$ANTH_BIN" guard hook --shim 2>&3 3>&- >/dev/null)
            printf 'x%s' "$?" >&3
        } 3>&1 2>/dev/null | LC_ALL=C tr '\000' '\001')
    fi
    ANTH_RC=${ANTH_ERR##*x}
    ANTH_ERR=${ANTH_ERR%x*}
    anth_only "$ANTH_RC" 0123456789 || ANTH_RC='?'
}

# anth_guard_settle: after a run that was not the allow token. ANTH_MSG = the binary's own
# block (exit 2, `Blocked by anthology-guardrails (` prefix), or the quarantine block.
anth_guard_settle() {
    ANTH_MSG=
    if [ "$ANTH_RC" = 2 ]; then
        case $ANTH_ERR in
            'Blocked by anthology-guardrails ('*)
                ANTH_MSG=$ANTH_ERR
                while :; do
                    case $ANTH_MSG in
                        *"$ANTH_NL") ANTH_MSG=${ANTH_MSG%"$ANTH_NL"} ;;
                        *) break ;;
                    esac
                done
                return 0
                ;;
        esac
    fi
    _e_first=${ANTH_ERR%%"$ANTH_NL"*}
    if [ -n "$_e_first" ]; then
        anth_quarantine_msg "exit $ANTH_RC: $(anth_printable "$_e_first" 120)"
    else
        anth_quarantine_msg "exit $ANTH_RC"
    fi
}
