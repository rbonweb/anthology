#!/bin/sh
# Dev only (spike S4 on macOS): how long the permissions mod's classifier call takes, the way the
# mod makes it: the installed binary run directly (`<anthology home>/bin/<lock version>/anthology
# perm classify`, never the bin/anthology shim) with ANTHOLOGY_NO_INSTALL=1 and the request on
# stdin. Prints p50 and p95 over N runs (200 by default). Run it on the host whose latency you
# want; it needs perl (macOS ships it) and an installed anthology binary that knows `perm
# classify` (release 0.1.5 or later). It installs and downloads nothing.
#
#   scripts/perm-latency.sh [runs]
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
runs=${1:-200}
case $runs in
    '' | *[!0-9]*)
        echo "usage: scripts/perm-latency.sh [runs]" >&2
        exit 2
        ;;
esac

# Find the binary as the mod does (hooks/mods/util.ts `installedAnthology`): the lock's version,
# under $CLAUDE_CONFIG_DIR/anthology or else $HOME/.claude/anthology, the binary and its .sha256
# non-empty, no link on the way.
lock="$root/plugins/anthology/anthology.lock"
version=$(sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' "$lock" | head -n 1)
case $version in
    '' | *[!0-9A-Za-z.-]*)
        echo "perm-latency: no release version in $lock" >&2
        exit 1
        ;;
esac
if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then
    home="$CLAUDE_CONFIG_DIR/anthology"
else
    home="${HOME:-}/.claude/anthology"
fi
case $home in
    /*) ;;
    *)
        echo "perm-latency: the anthology home is not an absolute path: $home" >&2
        exit 1
        ;;
esac
bin="$home/bin/$version/anthology"
if [ -L "$home/bin" ] || [ -L "$home/bin/$version" ] || [ -L "$bin" ] || [ ! -s "$bin" ] ||
    [ ! -s "$home/bin/$version/.sha256" ]; then
    echo "perm-latency: no installed anthology $version at $bin (this script never installs it)" >&2
    exit 1
fi

request="{\"command\":\"ls -la\",\"cwd\":\"$root\",\"roots\":[\"$root\"],\"read_rules\":[]}"

# The first call shows whether the binary answers at all; its time is not counted.
first=$(printf '%s' "$request" | ANTHOLOGY_NO_INSTALL=1 "$bin" perm classify 2>&1) || {
    echo "perm-latency: the classifier did not prove 'ls -la': $first" >&2
    exit 1
}

ANTHOLOGY_NO_INSTALL=1 exec perl -MTime::HiRes=time -e '
    my ($bin, $request, $runs) = @ARGV;
    my @ms;
    for (1 .. $runs) {
        my $t = time;
        open(my $p, "|-", $bin, "perm", "classify") or die "spawn: $!";
        print $p $request;
        close($p);
        push @ms, (time - $t) * 1000;
    }
    @ms = sort { $a <=> $b } @ms;
    my $at = sub { $ms[int($_[0] * $#ms + 0.5)] };
    printf "perm classify over %d runs: p50 %.1f ms, p95 %.1f ms, max %.1f ms\n",
        $runs, $at->(0.50), $at->(0.95), $ms[-1];
' "$bin" "$request" "$runs"
