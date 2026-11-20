#!/bin/sh
# Run the Anthology behaviour evals and gate them against the recorded baseline.
# This is the one command: CI (.github/workflows/evals.yml) and the controller both call it.
#
# The committed evals/baseline.json is recorded from the CI run, not from a laptop: dispatch
# evals.yml once, download its `evals-results` artifact and commit candidate-baseline.json (the
# file `--ci` writes when no baseline exists) as evals/baseline.json with the `candidate` key
# removed. That way the baseline and every later comparison come from the same environment (same
# runner, same installed tools, e.g. rtk). `--record` is for local experiments.
set -eu

usage() {
  cat <<'USAGE'
Usage: scripts/run-evals.sh [--max-cost-usd USD] [--record] [--ci] [--case GLOB] [--tag TAG]
                            [--no-smoke] [--smoke-budget-usd USD] [-h]

  --max-cost-usd USD  hard cost ceiling for the run (default 20); a run that hits it is
                      partial and fails the comparison
  --record            write evals/baseline.json from this (full) run instead of comparing; for
                      local experiments (see below for the committed baseline)
  --ci                also write $OUT/candidate-baseline.json when no baseline exists
  --case GLOB         run only the cases whose name matches GLOB (passed through)
  --tag TAG           run only the cases carrying TAG (passed through)
  --no-smoke          skip the smoke cases (evals/smoke/smoke.py) that run after the evals
  --smoke-budget-usd USD
                      --max-budget-usd for each `claude -p` the smokes run (default 1.00)
  -h, --help          this text

The committed baseline is recorded from the CI candidate artifact: dispatch evals.yml once, then
commit its candidate-baseline.json as evals/baseline.json with the `candidate` key removed, so
the baseline and the weekly comparison share one environment.
With --case or --tag only part of the suite runs, so the comparison uses --allow-missing and
--record is refused. The Claude binary is $CLAUDE_BIN, else ci/claude-code/node_modules/.bin/claude
when it is executable, else `claude` on PATH. Results go to evals/results/<UTC timestamp>/.
The smoke cases run after the comparison, with the same Claude binary and results in
$OUT/smoke.json. They are skipped with --no-smoke and (with a message) with --case or --tag.
Exit status: the comparison's when it is non-zero (1 regression, 2 invalid or partial result,
3 no baseline); else 4 if any smoke failed; else 0. A smoke that SKIPs (no credentials) is not a failure.
USAGE
}

die() { echo "run-evals: $*" >&2; exit 2; }

MAX=20
RECORD=0
CI=0
CASE=""
TAG=""
SMOKE=1
SMOKE_BUDGET=1.00
while [ $# -gt 0 ]; do
  case "$1" in
    --max-cost-usd)
      [ $# -ge 2 ] || die "--max-cost-usd needs a value"
      MAX=$2
      shift
      ;;
    --record) RECORD=1 ;;
    --ci) CI=1 ;;
    --case)
      [ $# -ge 2 ] || die "--case needs a glob"
      CASE=$2
      shift
      ;;
    --tag)
      [ $# -ge 2 ] || die "--tag needs a tag"
      TAG=$2
      shift
      ;;
    --no-smoke) SMOKE=0 ;;
    --smoke-budget-usd)
      [ $# -ge 2 ] || die "--smoke-budget-usd needs a value"
      SMOKE_BUDGET=$2
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    *) echo "run-evals: unknown option $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

# A plain decimal number: digits with at most one dot, and at least one digit.
case "$MAX" in
  ''|*[!0-9.]*|*.*.*|.) die "--max-cost-usd must be a decimal number, got: $MAX" ;;
esac
awk -v max="$MAX" 'BEGIN { exit !(max > 0) }' || die "--max-cost-usd must be greater than 0, got: $MAX"

case "$SMOKE_BUDGET" in
  ''|*[!0-9.]*|*.*.*|.) die "--smoke-budget-usd must be a decimal number, got: $SMOKE_BUDGET" ;;
esac

FILTERED=0
if [ -n "$CASE" ] || [ -n "$TAG" ]; then
  FILTERED=1
  [ "$RECORD" = 0 ] || die "--record needs a full run; drop --case and --tag"
fi

# Every grant is the one harmless command a guard or rtk case needs, so a broken guard still
# cannot download a browser or touch anything. One grant per line; scripts and tests read this
# list, keep the quoting as it is. The one exception to "harmless command" is
# `Bash(anthology-mem save *)`, for the agent-memory-path case: the agents' memory-save helper
# (stream I), which only writes a note into the agent's own memory folder. `Edit(...)` and
# `NotebookEdit(...)` are granted so the main session holds those tools in agent-fields and
# agent-memory-path: the scout's own tool list is then what the max-0 checks really test.
# `Bash(ls *)` exists so that Bash is visible in browser-browseros-only, where a host-browser
# launch attempt must be recorded (and denied) rather than hidden.
GRANTS='Bash(npx playwright install --dry-run chromium)
Bash(npx playwright test --list)
Bash(agent-browser --help)
Bash(git init -q rtkrepo)
Bash(git -C rtkrepo status)
Bash(ls *)
Bash(anthology-mem save *)
Write(.claude/memory/**)
Edit(.claude/memory/**)
NotebookEdit(.claude/memory/**)'

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
export PYTHONDONTWRITEBYTECODE=1
# If a guard regresses, a granted npx command (npx playwright ...) must not be able to fetch
# packages from the registry.
export NPM_CONFIG_OFFLINE=true
PYTHON=${PYTHON:-python3}
BASELINE=evals/baseline.json

if [ -n "${CLAUDE_BIN:-}" ]; then
  CLAUDE=$CLAUDE_BIN
elif [ -x ci/claude-code/node_modules/.bin/claude ]; then
  CLAUDE=ci/claude-code/node_modules/.bin/claude
else
  CLAUDE=claude
fi
echo "run-evals: using $CLAUDE"
"$CLAUDE" --version

OUT=evals/results/$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$OUT"
echo "run-evals: results in $OUT"

# NOTE: the plugin subcommand is quoted below so it is never read as the sh builtin of that name.
set -- plugin "eval" . --trust-plugin --ablation none --no-publish --threshold 0 --runs 1 \
  --max-cost-usd "$MAX" --json "$OUT/result.json" --output-dir "$OUT"
if [ -n "$CASE" ]; then set -- "$@" --case "$CASE"; fi
if [ -n "$TAG" ]; then set -- "$@" --tag "$TAG"; fi
# --allow-tools takes a list, so it goes last.
set -- "$@" --allow-tools
OLD_IFS=$IFS
IFS='
'
set -f
for grant in $GRANTS; do
  set -- "$@" "$grant"
done
set +f
IFS=$OLD_IFS

set +e
"$CLAUDE" "$@"
EVAL_STATUS=$?
set -e
if [ "$EVAL_STATUS" = 2 ]; then
  echo "run-evals: the cost ceiling (\$$MAX) was reached, so the run is partial; the comparison will fail it" >&2
elif [ "$EVAL_STATUS" != 0 ]; then
  echo "run-evals: the plugin run exited with status $EVAL_STATUS" >&2
fi

if [ "$RECORD" = 1 ]; then
  [ "$EVAL_STATUS" = 0 ] || die "the plugin run exited with status $EVAL_STATUS; not recording a baseline from it"
  set +e
  "$PYTHON" scripts/eval_baseline.py record "$OUT/result.json" "$BASELINE"
  CODE=$?
  set -e
else
  set -- compare "$OUT/result.json" "$BASELINE"
  if [ "$FILTERED" = 1 ]; then set -- "$@" --allow-missing; fi
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then set -- "$@" --summary "$GITHUB_STEP_SUMMARY"; fi
  set +e
  "$PYTHON" scripts/eval_baseline.py "$@"
  CODE=$?
  set -e
  if [ "$CI" = 1 ] && [ ! -f "$BASELINE" ] && [ -f "$OUT/result.json" ]; then
    "$PYTHON" scripts/eval_baseline.py candidate "$OUT/result.json" "$OUT/candidate-baseline.json" || true
  fi
fi

# The U1-U9 smoke cases (no-model and multi-session checks the eval harness cannot express).
SMOKE_FAILED=0
if [ "$SMOKE" = 0 ]; then
  echo "run-evals: smoke cases skipped (--no-smoke)"
elif [ "$FILTERED" = 1 ]; then
  echo "run-evals: smoke cases skipped: --case/--tag runs only part of the suite"
else
  set +e
  "$PYTHON" evals/smoke/smoke.py --claude "$CLAUDE" --budget-usd "$SMOKE_BUDGET" --out "$OUT/smoke.json"
  SMOKE_STATUS=$?
  set -e
  if [ "$SMOKE_STATUS" != 0 ]; then
    SMOKE_FAILED=1
    echo "run-evals: a smoke case failed (status $SMOKE_STATUS); see $OUT/smoke.json" >&2
  fi
fi

# A plugin run that failed for another reason must not pass quietly.
if [ "$CODE" = 0 ] && [ "$EVAL_STATUS" != 0 ] && [ "$EVAL_STATUS" != 2 ]; then
  exit "$EVAL_STATUS"
fi
if [ "$CODE" = 0 ] && [ "$SMOKE_FAILED" = 1 ]; then
  exit 4
fi
exit "$CODE"
