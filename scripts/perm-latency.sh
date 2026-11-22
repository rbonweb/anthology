#!/bin/sh
# Dev only (spike S4 on macOS): how long the permissions mod's classifier call takes, the way the
# mod makes it (`/bin/sh plugins/anthology/bin/anthology perm classify` with
# ANTHOLOGY_NO_INSTALL=1 and the request on stdin). Prints p50 and p95 over N runs (200 by
# default). Run it on the host whose latency you want; it needs perl (macOS ships it) and an
# installed anthology binary that knows `perm classify`. It installs and downloads nothing.
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

shim="$root/plugins/anthology/bin/anthology"
request="{\"command\":\"ls -la\",\"cwd\":\"$root\",\"roots\":[\"$root\"],\"read_rules\":[]}"

# The first call shows whether the binary answers at all; its time is not counted.
first=$(printf '%s' "$request" | ANTHOLOGY_NO_INSTALL=1 /bin/sh "$shim" perm classify 2>&1) || {
    echo "perm-latency: the classifier did not prove 'ls -la': $first" >&2
    exit 1
}

ANTHOLOGY_NO_INSTALL=1 exec perl -MTime::HiRes=time -e '
    my ($shim, $request, $runs) = @ARGV;
    my @ms;
    for (1 .. $runs) {
        my $t = time;
        open(my $p, "|-", "/bin/sh", $shim, "perm", "classify") or die "spawn: $!";
        print $p $request;
        close($p);
        push @ms, (time - $t) * 1000;
    }
    @ms = sort { $a <=> $b } @ms;
    my $at = sub { $ms[int($_[0] * $#ms + 0.5)] };
    printf "perm classify over %d runs: p50 %.1f ms, p95 %.1f ms, max %.1f ms\n",
        $runs, $at->(0.50), $at->(0.95), $ms[-1];
' "$shim" "$request" "$runs"
