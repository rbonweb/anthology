#!/usr/bin/env bash
# Renames the upstream project's name to ATL (Anthology Token Layer) in file contents and paths.
#
#   scripts/atl-rename.sh [--root DIR] [--check] [PATH...]
#
# This is the one place the rename rule lives, so the rename can be re-run: over a branch that
# was cut before it, and over an upstream tree before scripts/sync-atl.sh merges it (a merge over
# a renamed base needs the upstream side renamed the same way).
#
# The rule (case is kept). The lower-case and upper-case names are replaced where no letter or
# digit stands before them, so a longer word that ends in the name is left alone; a letter
# after the name does not matter (the plural, a longer word that starts with it). One letter
# is allowed before the name when it ends a backslash escape (\n, \t, \r, \b, \f). Title case is
# replaced wherever it stands, so a CamelCase word is renamed:
#   the lower-case name, then upper case, then Title case   ->   atl, ATL, Atl
#   "Rust Token Killer"                                     ->   "Anthology Token Layer"
# These stay as they are, because they name the upstream project or a published artifact:
#   github.com/<upstream org>/<upstream repo>, its web site, the fork repo, the company name
#   "RTK AI Labs", the product "RTK Pro", the published tags 1.2.1-anthology.1 to .5, the phrase
#   "fork of <name>" and the Python test ids test_<name>_*.
# A line with a comment that starts with the word legacy (`// legacy`, `//! legacy`,
# `# legacy`, `<!-- legacy`) is never touched: it keeps the old name on purpose, as a name the code still
# has to recognise. The word in running text does not protect a line.
#
# PATH may be files or directories under the root; none means every file (the tracked files and
# the new ones, in a git checkout; every file otherwise). Binary files are skipped. The paths
# below are left alone: their content is history, a license text or test data, or they are this
# script. A top-level directory still moves, so a skipped file inside it follows its folder.
#
# --check changes nothing: it lists what would change and exits 1 if anything would.
set -eu

usage() {
    echo "usage: scripts/atl-rename.sh [--root DIR] [--check] [PATH...]" >&2
    exit 2
}

root=$(cd "$(dirname "$0")/.." && pwd)
check=0
while [ "$#" -gt 0 ]; do
    case $1 in
        --root)
            [ "$#" -ge 2 ] || usage
            root=$(cd "$2" && pwd)
            shift 2
            ;;
        --check)
            check=1
            shift
            ;;
        --help | -h) usage ;;
        --) shift; break ;;
        -*) usage ;;
        *) break ;;
    esac
done

cd "$root"

list_all() {
    if [ -e .git ]; then
        git ls-files -z --cached --others --exclude-standard
    else
        find . -type f -not -path './.git/*' -print0
    fi
}

list_paths() {
    if [ "$#" -eq 0 ]; then
        list_all
        return
    fi
    for p in "$@"; do
        p=${p#./}
        if [ -d "$p" ]; then
            find "$p" -type f -not -path '*/.git/*' -print0
        else
            printf '%s\0' "$p"
        fi
    done
}

# shellcheck disable=SC2016 # the program is Perl, not shell
program='
use strict;
use warnings;

my ($check) = @ARGV;
local $/ = "\0";
my @paths = <STDIN>;
chomp @paths;
s{^\./}{} for @paths;

# Kept whole: names of the upstream project and its published artifacts.
my $keep = qr{rtk-ai/rtk|rtk-ai\.app|rbonweb/rtk|RTK AI Labs|RTK Pro\b|rtk--v1\.2\.1-anthology\.[1-5]\b|fork of rtk}; # legacy
# The Python test ids stay; a Rust test function inside the binary crate does not.
my $keep_ids = qr{$keep|test_rtk_}; # legacy
# Where a name may start: after no letter or digit, or after a backslash escape letter.
my $start = qr{(?:(?<![A-Za-z0-9])|(?<=\\[nrtbf]))};

my @skip = (
    qr{^atl/LICENSE$}, qr{^atl/CHANGELOG\.md$}, qr{^CHANGELOG\.md$}, qr{^DECISIONS\.md$},
    qr{^docs/history/PLAN\.md$}, qr{^port/}, qr{^docs/superpowers/[^/]+/2026-11-20-},
    qr{^crates/anthology-guard/tests/data/}, qr{^scripts/atl-rename\.sh$},
    qr{(?:^|/)target/}, qr{^\.git/}, qr{\.gz$},
);

sub swap {
    my ($text, $binary_crate) = @_;
    my $kept = $binary_crate ? $keep : $keep_ids;
    $text =~ s/Rust Token Killer/Anthology Token Layer/g;
    $text =~ s{($kept)|${start}rtk|${start}RTK|Rtk}{ # legacy
        defined $1 ? $1 : $& eq "rtk" ? "atl" : $& eq "RTK" ? "ATL" : "Atl" # legacy
    }ge;
    return $text;
}

my $changed = 0;
my @moves;
for my $path (@paths) {
    next unless length $path && -f $path && !-l $path;
    my $canon = $path;
    $canon =~ s{^rtk/}{atl/}; # legacy
    my $skipped = grep { $canon =~ $_ } @skip;

    unless ($skipped) {
        open(my $in, "<:raw", $path) or die "atl-rename: cannot read $path: $!\n";
        my $content = do { local $/; <$in> };
        close $in;
        if (index($content, "\0") < 0) {
            my $out = join "", map { m{(?://[/!]?|#|<!--)\s*legacy\b} ? $_ : swap($_, $canon =~ m{^atl/}) } split /(?<=\n)/, $content;
            if ($out ne $content) {
                $changed++;
                print "content: $path\n";
                unless ($check) {
                    open(my $fh, ">:raw", $path) or die "atl-rename: cannot write $path: $!\n";
                    print $fh $out;
                    close $fh;
                }
            }
        }
    }

    my $to = $skipped ? $canon : join "/", map { swap($_) } split m{/}, $path;
    push @moves, [$path, $to] if $to ne $path;
}

for my $m (@moves) {
    my ($from, $to) = @$m;
    $changed++;
    print "path: $from -> $to\n";
    next if $check;
    (my $dir = $to) =~ s{/[^/]*$}{};
    system("mkdir", "-p", $dir) == 0 or die "atl-rename: mkdir $dir failed\n" if $dir ne $to;
    rename($from, $to) or die "atl-rename: cannot move $from to $to: $!\n";
    my $old = $from;
    while ($old =~ s{/[^/]*$}{}) {
        last unless rmdir $old;
    }
}

exit($check && $changed ? 1 : 0);
'

list_paths "$@" | perl -e "$program" -- "$check"
