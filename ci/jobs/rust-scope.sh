#!/bin/sh
# CI job `rust-scope`: which tests a pull request needs. Prints `key=value` lines for
# $GITHUB_OUTPUT and the reasons on stderr. Runs in the repository root.
#
#   PR_LABELS='["bug","full-ci"]' ci/jobs/rust-scope.sh BASE HEAD
#
# From the files the pull request changes (BASE...HEAD, from the merge base) it works out the
# changed crates, then, with `cargo metadata`, every crate that depends on them (a normal, build
# or dev dependency, ci/jobs/rust-graph.mjs); only those crates are tested. Everything runs
# (`full=true`) when
#   - there is no usable range (a push, a manual run, an unknown commit, an empty diff);
#   - the pull request has the label `full-ci`, or one of its commits has `[full-ci]` anywhere in
#     its message (a trailer, or the subject);
#   - a changed path is workspace-wide (a `full` rule, or a Cargo.toml or Cargo.lock change that
#     is not just a version), or no rule in ci/scope-rules.tsv places it, or a changed file is in
#     a crates/ folder that is not a workspace member;
#   - `cargo metadata` fails.
# Release metadata (memory notes, CHANGELOG.md, changelog.d, and version-only edits of Cargo.toml
# and the two Cargo.lock files; a registry or git dependency that moves also changes its checksum
# or source line, so it counts as code) needs only the repository checks, repo-tests (the
# anthology_lock and atl_lock checks among them). So does everything else that no crate reads:
# docs, plugins, scripts, evals (ci/scope-rules.tsv).
#
# Output:
#   full           true when everything runs
#   packages       the crates whose tests run, sorted, separated by spaces
#   surfaces       the difftest golden surfaces that replay: those whose crate is among the
#                  packages, those a rule names, and all of them for a difftest change
#   atl            true when atl-test runs: atl/ changed, or a crate atl builds did
#   atl_macos      true when atl-test-macos runs: atl/ itself changed (or a full run); its tests
#                  meet macOS-only path behaviour, such as the /private/var symlinks
#   fuzz           true when fuzz-build runs: fuzz/ changed, or a crate the targets build did
#   run_linux      true when any Linux shard runs
#   run_macos      true when the macOS shards run: always for a full run, otherwise when a
#                  `macos` rule (the shims, hooks, the guard, spawn and platform code) matched
#   linux_matrix   the Linux shards as a GitHub matrix: `rest` shards (the crates' tests with the
#                  golden replay left out, split by ci/test-weights.tsv) and one `replay` shard
#                  (the surfaces), for plain and for --all-features; {"include":[]} when none runs
#   macos_matrix   the macOS shards (plain and --all-features), each with the crates to test (empty
#                  for a full run: the whole workspace and the ledger listings); {"include":[]}
#                  when none runs
#   jobs           the repository jobs of ci.yml that run, as `,name,name,` (so ci.yml can test
#                  `contains(jobs, ',name,')`), sorted; every one on a full run. A job runs when a
#                  path it reads changed (`job:NAME` rules), or a crate it builds or runs did (the
#                  `crate:NAME` rows: the crate or one it depends on changed). The jobs gitleaks,
#                  deny and shellcheck are not here: they run on every pull request
set -eu

here=$(cd "$(dirname "$0")" && pwd)
rules=$here/../scope-rules.tsv
weights=$here/../test-weights.tsv
graph=$here/rust-graph.mjs
tab=$(printf '\t')
nl='
'

base=${1:-}
head=${2:-}
labels=${PR_LABELS:-}

meta=$(mktemp)
trap 'rm -f "$meta"' EXIT
# The workspace members: `<folder><TAB><name>` lines, empty when cargo or node cannot say.
dirs=
if ${CARGO:-cargo} metadata --no-deps --format-version 1 --offline >"$meta" 2>/dev/null &&
    out=$(node "$graph" "$meta" dirs 2>/dev/null); then
    dirs=$out
else
    echo "rust-scope: cargo metadata failed" >&2
    dirs=
fi

# Every golden: `<crate folder><TAB><surface>` lines (a seed corpus is not `.jsonl.gz`).
goldens=$(git ls-files -- 'crates/*/tests/golden/*.jsonl.gz' 2>/dev/null |
    awk -F/ '{ s = $NF; sub(/\.jsonl\.gz$/, "", s); print $1 "/" $2 "\t" s }' || true)

# The package a crate folder holds.
name_of_dir() {
    printf '%s\n' "$dirs" | awk -F'\t' -v d="$1" '$1 == d { print $2; exit }'
}

# The packages at the `path = "../crates/<folder>"` dependencies of manifest $1.
path_packages() {
    [ -f "$1" ] || return 0
    sed -n 's|.*path = "\.\./crates/\([^"/]*\)".*|crates/\1|p' "$1" | while read -r d; do
        name_of_dir "$d"
    done
}

# The repository jobs of ci.yml that rust-scope switches (the `jobs` output); the tests in
# crates/repo-tests/tests/ci_scope.rs hold this list to the jobs ci.yml reads it for.
all_jobs="cli-syntax ledger lock-fresh memory-diff mods plugin-validate rust-lint shims-distros startup-budget vendor-check version-bump"

# The effects of the `crate:NAME` rows whose crate is among $src_affected, as words.
crate_effects() {
    while IFS="$tab" read -r pattern effects; do
        case $pattern in
            crate:*)
                if printf '%s\n' "$src_affected" | grep -qx -- "${pattern#crate:}"; then
                    printf '%s ' "$effects"
                fi
                ;;
        esac
    done <"$rules"
}

# Prints the output lines. $1: true for a full run. The accumulated state is in the globals set
# below: changed, src_changed, jobs, direct_surfaces, all_surfaces, want_atl, want_atl_macos,
# want_fuzz, want_macos.
emit() {
    is_full=$1
    src_affected=
    if [ "$is_full" = true ]; then
        affected=$(printf '%s\n' "$dirs" | awk -F'\t' 'NF == 2 { print $2 }' | LC_ALL=C sort -u)
        all_surfaces=true
        want_atl=true
        want_atl_macos=true
        want_fuzz=true
        want_macos=true
        jobs=$all_jobs
    else
        if [ -n "$changed" ]; then
            # shellcheck disable=SC2086 # $changed is a list of package names
            affected=$(node "$graph" "$meta" affected $changed) || return 1
        else
            affected=
        fi
        # The crates whose source changed (not the repo-tests an edit to docs stands for) and the
        # crates that depend on them: the ones a `crate:NAME` row names are affected by them.
        if [ -n "$src_changed" ]; then
            # shellcheck disable=SC2086 # $src_changed is a list of package names
            src_affected=$(node "$graph" "$meta" affected $src_changed) || return 1
        fi
        for effect in $(crate_effects); do
            case $effect in
                job:*) jobs="$jobs ${effect#job:}" ;;
            esac
        done
    fi

    # The surfaces: every golden of an affected crate (all of them for a difftest change or a
    # full run), and the ones a rule names.
    surfaces=$({
        printf '%s' "$direct_surfaces"
        printf '%s\n' "$goldens" | while IFS="$tab" read -r d s; do
            [ -n "$s" ] || continue
            owner=$(name_of_dir "$d")
            if [ "$all_surfaces" = true ] ||
                { [ -n "$owner" ] && printf '%s\n' "$affected" | grep -qx -- "$owner"; }; then
                echo "$s"
            fi
        done
    } | grep . | LC_ALL=C sort -u || true)

    # atl and the fuzz targets build some of the workspace's crates.
    if [ "$is_full" != true ]; then
        for t in atl fuzz; do
            deps=$(path_packages "$t/Cargo.toml" | LC_ALL=C sort -u)
            [ -n "$deps" ] || continue
            # shellcheck disable=SC2086 # $deps is a list of package names
            closure=$(node "$graph" "$meta" closure $deps) || return 1
            for c in $changed; do
                if printf '%s\n' "$closure" | grep -qx -- "$c"; then
                    [ "$t" = atl ] && want_atl=true
                    [ "$t" = fuzz ] && want_fuzz=true
                fi
            done
        done
    fi

    packages=$(printf '%s\n' "$affected" | tr '\n' ' ' | sed 's/ *$//')
    surface_list=$(printf '%s\n' "$surfaces" | paste -sd, -)
    run_linux=false
    [ -z "$packages" ] && [ -z "$surface_list" ] || run_linux=true
    run_macos=false
    if [ "$want_macos" = true ] && { [ "$is_full" = true ] || [ -n "$packages" ]; }; then
        run_macos=true
    fi

    linux_matrix='{"include":[]}'
    if [ "$run_linux" = true ]; then
        shards=
        for features in plain all; do
            if [ -n "$packages" ] || [ "$is_full" = true ]; then
                # The crates over parallel shards: about 100 s of tests each, at most 3 shards
                # (a crate without a weight counts 30). With no crate list (cargo metadata failed
                # on a full run) one shard runs the whole workspace.
                bins=$(printf '%s\n' "$affected" | awk -v weights="$weights" '
                    BEGIN {
                        FS = "\t"
                        while ((getline line < weights) > 0) {
                            if (line ~ /^#/ || line == "") continue
                            split(line, f, "\t")
                            w[f[1]] = f[2]
                        }
                    }
                    NF > 0 && $0 != "" { n++; name[n] = $0; weight[n] = ($0 in w) ? w[$0] + 0 : 30; total += weight[n] }
                    END {
                        k = int((total + 99) / 100)
                        if (k > 3) k = 3
                        if (k < 1) k = 1
                        # heaviest first, each crate to the lightest shard
                        for (i = 1; i <= n; i++) done[i] = 0
                        for (round = 1; round <= n; round++) {
                            best = 0
                            for (i = 1; i <= n; i++)
                                if (!done[i] && (best == 0 || weight[i] > weight[best] ||
                                    (weight[i] == weight[best] && name[i] < name[best]))) best = i
                            done[best] = 1
                            lightest = 1
                            for (b = 2; b <= k; b++) if (load[b] + 0 < load[lightest] + 0) lightest = b
                            load[lightest] += weight[best]
                            list[lightest] = list[lightest] (list[lightest] == "" ? "" : "\n") name[best]
                        }
                        for (b = 1; b <= k; b++) {
                            if (list[b] == "") continue
                            cmd = "LC_ALL=C sort | paste -sd\" \" -"
                            print list[b] | cmd
                            close(cmd)
                        }
                    }')
                count=$(printf '%s\n' "$bins" | grep -c . || true)
                [ "$count" -gt 0 ] || { bins=; count=1; }
                i=0
                if [ -z "$bins" ]; then
                    shards="${shards:+$shards,}{\"features\":\"$features\",\"part\":\"rest\",\"name\":\"$features-rest\",\"packages\":\"\",\"surfaces\":\"\"}"
                else
                    for list in $(printf '%s\n' "$bins" | tr ' ' ','); do
                        i=$((i + 1))
                        name=$features-rest
                        [ "$count" -eq 1 ] || name=$name-$i
                        shards="${shards:+$shards,}{\"features\":\"$features\",\"part\":\"rest\",\"name\":\"$name\",\"packages\":\"$(printf '%s' "$list" | tr ',' ' ')\",\"surfaces\":\"\"}"
                    done
                fi
            fi
            if [ -n "$surface_list" ]; then
                shards="${shards:+$shards,}{\"features\":\"$features\",\"part\":\"replay\",\"name\":\"$features-replay\",\"packages\":\"\",\"surfaces\":\"$surface_list\"}"
            fi
        done
        linux_matrix="{\"include\":[$shards]}"
    fi

    macos_matrix='{"include":[]}'
    if [ "$run_macos" = true ]; then
        for_macos=$packages
        [ "$is_full" != true ] || for_macos=
        macos_matrix="{\"include\":[{\"features\":\"plain\",\"name\":\"plain\",\"packages\":\"$for_macos\"},{\"features\":\"all\",\"name\":\"all\",\"packages\":\"$for_macos\"}]}"
    fi

    # `,a,b,` (empty when no job runs), sorted and without repeats.
    # shellcheck disable=SC2086 # $jobs is a list of job names
    jobs_out=$(printf '%s\n' $jobs | grep . | LC_ALL=C sort -u | paste -sd, -)
    [ -z "$jobs_out" ] || jobs_out=,$jobs_out,

    echo "full=$is_full"
    echo "packages=$packages"
    echo "surfaces=$surface_list"
    echo "atl=$want_atl"
    echo "atl_macos=$want_atl_macos"
    echo "fuzz=$want_fuzz"
    echo "run_linux=$run_linux"
    echo "run_macos=$run_macos"
    echo "linux_matrix=$linux_matrix"
    echo "macos_matrix=$macos_matrix"
    echo "jobs=$jobs_out"
}

changed=
src_changed=
jobs=
direct_surfaces=
all_surfaces=false
want_atl=false
want_atl_macos=false
want_fuzz=false
want_macos=false

full() {
    echo "rust-scope: everything runs: $1" >&2
    emit true || {
        # The graph helper failed on a full run: one shard per feature set runs the workspace.
        dirs=
        emit true
    }
    exit 0
}

[ -n "$base" ] && [ -n "$head" ] || full "no pull request range (a push or a manual run)"
git cat-file -e "$base^{commit}" 2>/dev/null || full "unknown base commit $base"
git cat-file -e "$head^{commit}" 2>/dev/null || full "unknown head commit $head"
from=$(git merge-base "$base" "$head") || full "no merge base of $base and $head"
files=$(git diff --name-only --no-renames "$from" "$head" --) || full "git diff failed"
[ -n "$files" ] || full "no file changed"
[ -n "$dirs" ] || full "cargo metadata failed"

case $labels in
    *'"full-ci"'*) full "the full-ci label" ;;
esac
if git log --no-merges --format=%B "$from..$head" -- | grep -qF '[full-ci]'; then
    full "a commit asks for it with [full-ci]"
fi

# The text of file $2 at revision $1 with the lines a version bump may change ($3: toml or lock)
# each replaced by one placeholder line: a changed version compares equal, but a version line
# that is removed or added (or duplicated) leaves a different number of lines.
versionless() {
    git show "$1:$2" | awk -v kind="$3" '
        /^\[/ { table = $0 }
        kind == "toml" && (table == "[package]" || table == "[workspace.package]") &&
            /^version = "[^"]*"$/ { print "@version-line@"; next }
        kind == "lock" && /^version = "[^"]*"$/ { print "@version-line@"; next }
        { print }'
}

# Whether file $1 exists on both sides and differs in nothing but versions ($2 as above).
version_only() {
    git cat-file -e "$from:$1" 2>/dev/null && git cat-file -e "$head:$1" 2>/dev/null &&
        [ "$(versionless "$from" "$1" "$2")" = "$(versionless "$head" "$1" "$2")" ]
}

# Whether $1 is release metadata (the list in the header).
release_metadata() {
    case $1 in
        .claude/memory/* | .claude/agent-memory/* | CHANGELOG.md | changelog.d/*) return 0 ;;
        Cargo.toml) version_only "$1" toml ;;
        Cargo.lock | fuzz/Cargo.lock) version_only "$1" lock ;;
        *) return 1 ;;
    esac
}

# The effects of every rule that matches path $1, as words.
rule_effects() {
    while IFS="$tab" read -r pattern effects; do
        case $pattern in '' | '#'*) continue ;; esac
        # shellcheck disable=SC2254 # the pattern is data, matched as a glob
        case $1 in $pattern) printf '%s ' "$effects" ;; esac
    done <"$rules"
}

set -f
old_ifs=$IFS
IFS=$nl
for f in $files; do
    IFS=$old_ifs
    if release_metadata "$f"; then
        changed="$changed repo-tests"
        # Release metadata stays out of the crates and the goldens, but a job may read it (the
        # memory listing reads the notes, lock-fresh the root manifest and lock).
        for effect in $(rule_effects "$f"); do
            case $effect in job:*) jobs="$jobs ${effect#job:}" ;; esac
        done
        IFS=$nl
        continue
    fi
    case $f in
        Cargo.toml | Cargo.lock) full "$f changes more than a version" ;;
    esac
    effects=$(rule_effects "$f")
    placed=false
    [ -z "$effects" ] || placed=true
    case $f in
        crates/*)
            folder=${f#crates/}
            folder=crates/${folder%%/*}
            member=$(name_of_dir "$folder")
            [ -n "$member" ] || full "$f is in $folder, which is not a workspace member"
            changed="$changed $member"
            src_changed="$src_changed $member"
            [ "$member" != difftest ] || all_surfaces=true
            placed=true
            ;;
    esac
    [ "$placed" = true ] || full "no rule in ci/scope-rules.tsv places $f"
    for effect in $effects; do
        case $effect in
            full) full "$f is workspace-wide (ci/scope-rules.tsv)" ;;
            atl) want_atl=true ;;
            atl-macos) want_atl_macos=true ;;
            fuzz) want_fuzz=true ;;
            macos) want_macos=true ;;
            job:*) jobs="$jobs ${effect#job:}" ;;
            pkg:*) changed="$changed ${effect#pkg:}" ;;
            surface:'*') all_surfaces=true ;;
            surface:*) direct_surfaces="$direct_surfaces${effect#surface:}$nl" ;;
            *) full "ci/scope-rules.tsv has the unknown effect $effect for $f" ;;
        esac
    done
    IFS=$nl
done
IFS=$old_ifs
set +f

changed=$(printf '%s' "$changed" | tr ' ' '\n' | grep . | LC_ALL=C sort -u | tr '\n' ' ' || true)
src_changed=$(printf '%s' "$src_changed" | tr ' ' '\n' | grep . | LC_ALL=C sort -u | tr '\n' ' ' || true)
echo "rust-scope: changed crates:${changed:- none}" >&2
emit false || full "the dependency graph could not be read"
