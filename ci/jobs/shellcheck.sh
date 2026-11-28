#!/bin/sh
# CI job `shellcheck` (design §14.1; plan BS1 acceptance 10). `shellcheck -s sh` on every shim
# under port/staging/plugins and on the BS job scripts, in the pinned image
# (ci/images/shellcheck.Dockerfile). Sourced files are followed from each script's directory.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/shellcheck.Dockerfile")
if [ -z "$image" ]; then
    echo "shellcheck: no FROM line in ci/images/shellcheck.Dockerfile" >&2
    exit 1
fi

# The shims are in port/staging/ until CO2 moves them into plugins/ (the switch of
# anthology_testkit::staged).
staging=plugins
[ ! -d "$root/port/staging" ] || staging=port/staging/plugins
set -- \
    "$staging/anthology/hooks/lib.sh" \
    "$staging/anthology/hooks/bootstrap.sh" \
    "$staging/anthology/hooks/session-start.sh" \
    "$staging/anthology/hooks/no-attribution.sh" \
    "$staging/anthology/hooks/slop-gate.sh" \
    "$staging/anthology/hooks/commit-privacy.sh" \
    "$staging/anthology/bin/anthology" \
    "$staging/anthology/bin/anthology-mem" \
    "$staging/anthology-guardrails/hooks/lib.sh" \
    "$staging/anthology-guardrails/hooks/bootstrap.sh" \
    "$staging/anthology-guardrails/hooks/guard.sh" \
    ci/jobs/shellcheck.sh \
    ci/jobs/shims-distros.sh \
    ci/jobs/mods.sh \
    ci/jobs/lock-fresh.sh \
    ci/jobs/rust-scope.sh \
    ci/jobs/rust-gate.sh \
    ci/jobs/rust-cache.sh \
    ci/jobs/rust-test-linux.sh \
    ci/jobs/rust-test-macos.sh \
    ci/jobs/atl-test-macos.sh

# Scripts written for bash (not POSIX sh), checked as bash: the release script, the Rust test
# runner (its shebang is bash and it uses ${var:0:12}) and its remote-host wrapper.
bash_scripts="scripts/release.sh scripts/rust-test.sh scripts/remote-build.sh"

for f in "$@" $bash_scripts; do
    if [ ! -f "$root/$f" ]; then
        echo "shellcheck: $f is missing" >&2
        exit 1
    fi
done

docker run --rm -v "$root":/w:ro -w /w "$image" \
    --shell=sh --external-sources --source-path=SCRIPTDIR "$@"
# shellcheck disable=SC2086 # $bash_scripts is a list of paths without spaces
docker run --rm -v "$root":/w:ro -w /w "$image" \
    --shell=bash --external-sources --source-path=SCRIPTDIR $bash_scripts
echo "shellcheck: $# files clean, plus $bash_scripts as bash"
