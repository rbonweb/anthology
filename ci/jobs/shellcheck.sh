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

staging=port/staging/plugins
set -- \
    "$staging/anthology/hooks/lib.sh" \
    "$staging/anthology/hooks/bootstrap.sh" \
    "$staging/anthology/hooks/session-start.sh" \
    "$staging/anthology/bin/anthology" \
    "$staging/anthology/bin/anthology-mem" \
    "$staging/anthology-guardrails/hooks/lib.sh" \
    "$staging/anthology-guardrails/hooks/bootstrap.sh" \
    "$staging/anthology-guardrails/hooks/guard.sh" \
    ci/jobs/shellcheck.sh \
    ci/jobs/shims-distros.sh

for f in "$@"; do
    if [ ! -f "$root/$f" ]; then
        echo "shellcheck: $f is missing" >&2
        exit 1
    fi
done

docker run --rm -v "$root":/w:ro -w /w "$image" \
    --shell=sh --external-sources --source-path=SCRIPTDIR "$@"
echo "shellcheck: $# files clean"
