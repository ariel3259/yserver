#!/usr/bin/env bash
# Dispatch the frozen adversarial plan-review brief to codex.
#
# The point of this script is that the INSTRUMENT is fixed and recorded, not
# just the brief. Five things vary a review's result; all five are pinned here
# and printed into a provenance block for the findings document.
set -euo pipefail

REVIEW_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BRIEF="$REVIEW_DIR/brief.md"

# --- The instrument. Changing any of these is a deliberate act; see README.md.
MODEL="gpt-5.6-sol"
EFFORT="medium"
MODE="single pass"

usage() {
  cat >&2 <<'USAGE'
usage: review.sh --plan PATH --spec PATH --out PATH
                 [--prior PATH] [--context TEXT] [--out-of-scope TEXT]

  --plan          plan under review
  --spec          spec it must obey
  --out           where the review is written
  --prior         previous review of this plan; omit for a first review
  --context       codebase orientation, task renumbering, anything the
                  reviewer needs so it does not report a non-defect
  --out-of-scope  what must NOT be reported as a defect (deferred scope)
USAGE
  exit 2
}

PLAN= SPEC= OUT= PRIOR= CONTEXT= OOS=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --plan) PLAN="$2"; shift 2 ;;
    --spec) SPEC="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --prior) PRIOR="$2"; shift 2 ;;
    --context) CONTEXT="$2"; shift 2 ;;
    --out-of-scope) OOS="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done
[[ -n "$PLAN" && -n "$SPEC" && -n "$OUT" ]] || usage
for f in "$PLAN" "$SPEC" ${PRIOR:+"$PRIOR"}; do
  [[ -f "$f" ]] || { echo "no such file: $f" >&2; exit 1; }
done

# --- Refuse to run an unrecorded instrument.
# A brief that is uncommitted or modified cannot be cited by SHA, so its
# result cannot be compared to any other round. That is the whole failure
# this skill exists to prevent, so it is an error rather than a warning.
if ! git -C "$REVIEW_DIR" diff --quiet -- "$BRIEF" 2>/dev/null \
   || ! git -C "$REVIEW_DIR" diff --cached --quiet -- "$BRIEF" 2>/dev/null; then
  echo "REFUSING: brief.md has uncommitted changes." >&2
  echo "Commit it first, so this review's result can be attributed to a" >&2
  echo "specific brief version. See README.md 'Changing the brief'." >&2
  exit 1
fi
BRIEF_SHA="$(git -C "$REVIEW_DIR" log -1 --format=%h -- "$BRIEF" 2>/dev/null || true)"
[[ -n "$BRIEF_SHA" ]] || { echo "REFUSING: brief.md is not committed yet." >&2; exit 1; }

# --- Build the prompt. Only these five slots vary.
if [[ -n "$PRIOR" ]]; then
  PRIOR_BLOCK="PRIOR REVIEW. The plan claims to incorporate it:
  $PRIOR"
else
  PRIOR_BLOCK="PRIOR REVIEW: none. This is the first review of this plan; skip check 1."
fi

PROMPT="$(
  BRIEF_TEXT="$(cat "$BRIEF")"
  BRIEF_TEXT="${BRIEF_TEXT//\{\{PLAN\}\}/$PLAN}"
  BRIEF_TEXT="${BRIEF_TEXT//\{\{SPEC\}\}/$SPEC}"
  BRIEF_TEXT="${BRIEF_TEXT//\{\{PRIOR\}\}/$PRIOR_BLOCK}"
  BRIEF_TEXT="${BRIEF_TEXT//\{\{CONTEXT\}\}/${CONTEXT:-No additional context.}}"
  BRIEF_TEXT="${BRIEF_TEXT//\{\{OUT_OF_SCOPE\}\}/${OOS:-  Nothing is out of scope.}}"
  printf '%s' "$BRIEF_TEXT"
)"

CODEX_VERSION="$(codex --version 2>&1 | tr -d '\n')"

echo "dispatching: brief $BRIEF_SHA | $MODEL | effort $EFFORT | $CODEX_VERSION" >&2
codex exec \
  --sandbox read-only \
  --model "$MODEL" \
  -c model_reasoning_effort="\"$EFFORT\"" \
  --output-last-message "$OUT" \
  "$PROMPT" < /dev/null

# --- The provenance block. Paste this into the findings document.
cat <<PROV

--- paste into the findings document, under the Result line ---

**Reviewer:** \`codex exec --sandbox read-only\`, $MODE
**Instrument:** brief \`.claude/skills/adversarial-plan-review/brief.md\` @ \`$BRIEF_SHA\`;
model \`$MODEL\`; reasoning effort \`$EFFORT\`; \`$CODEX_VERSION\`.
Counts are comparable only to other reviews citing this same brief SHA.
PROV
