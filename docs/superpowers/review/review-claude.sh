#!/usr/bin/env bash
# Dispatch the frozen adversarial plan-review brief to claude, for the
# intervals when codex is unavailable.
#
# This is a SECOND INSTRUMENT, not a drop-in substitute. It sends brief.md
# byte-identical to what review.sh would send, so the reviewer is the only
# deliberate variable — but the reviewer is one of the five things that decide
# a count, so results from this script are never comparable to a codex round.
# See README.md "Lineage".
set -euo pipefail

REVIEW_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BRIEF="$REVIEW_DIR/brief.md"

# --- The instrument. Changing any of these is a deliberate act; see README.md.
MODEL="claude-opus-5"
EFFORT="medium"
MODE="single pass"
# Read-only by construction: the reviewer locates and excerpts, never edits,
# never shells out, and never launches a nested reviewer.
# Comma-separated in ONE argument each: these flags are variadic, so a
# space-separated list swallows whatever follows it.
ALLOWED_TOOLS="Read,Grep,Glob"
DISALLOWED_TOOLS="Bash,Edit,Write,NotebookEdit,Task,Agent,WebFetch,WebSearch"

usage() {
  cat >&2 <<'USAGE'
usage: review-claude.sh --plan PATH --spec PATH --out PATH
                        [--prior PATH] [--context TEXT] [--out-of-scope TEXT]

  Same arguments as review.sh. Use only while codex is unavailable, and say so
  in the findings document: counts do not compare across reviewers.
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

# --- Refuse to run an unrecorded instrument. Same rule as review.sh: this
# script carries the model and the thinking budget, so an uncommitted edit
# here produces a result that cannot be attributed to anything.
if ! git -C "$REVIEW_DIR" diff --quiet -- "$REVIEW_DIR" 2>/dev/null \
   || ! git -C "$REVIEW_DIR" diff --cached --quiet -- "$REVIEW_DIR" 2>/dev/null; then
  echo "REFUSING: docs/superpowers/review/ has uncommitted changes." >&2
  echo "Commit them first, so this review's result can be attributed to a" >&2
  echo "specific instrument version. See README.md 'Changing the brief'." >&2
  exit 1
fi
BRIEF_SHA="$(git -C "$REVIEW_DIR" log -1 --format=%h -- "$REVIEW_DIR" 2>/dev/null || true)"
[[ -n "$BRIEF_SHA" ]] || { echo "REFUSING: the review tooling is not committed yet." >&2; exit 1; }

# --- Build the prompt. Identical substitution to review.sh; only these five
# slots vary, and brief.md itself is untouched.
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

CLAUDE_VERSION="claude $(claude --version 2>&1 | tr -d '\n')"

echo "dispatching: brief $BRIEF_SHA | $MODEL | effort $EFFORT | $CLAUDE_VERSION" >&2
# The prompt goes on stdin, never as a positional argument: --allowed-tools and
# --disallowed-tools are variadic and would consume it as a tool name.
printf '%s' "$PROMPT" | claude \
  --print \
  --model "$MODEL" \
  --effort "$EFFORT" \
  --allowed-tools "$ALLOWED_TOOLS" \
  --disallowed-tools "$DISALLOWED_TOOLS" \
  > "$OUT"

# --- The provenance block. Paste this into the findings document.
cat <<PROV

--- paste into the findings document, under the Result line ---

**Reviewer:** \`claude --print\`, read-only tool set, $MODE
**Instrument:** \`docs/superpowers/review/\` @ \`$BRIEF_SHA\`;
model \`$MODEL\`; reasoning effort \`$EFFORT\`; \`$CLAUDE_VERSION\`.
Dispatched by \`review-claude.sh\` because codex was unavailable.
**Not comparable to any codex round**, including reviews citing this same SHA:
the reviewer is part of the instrument. This reviewer also inherits the repo's
CLAUDE.md/AGENTS.md and the user's global instructions, which codex does not.
PROV
