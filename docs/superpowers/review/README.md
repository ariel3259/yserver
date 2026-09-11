# Adversarial plan review

AGENTS.md sends design docs to `docs/superpowers/specs` and implementation
plans to `docs/superpowers/plans`. CLAUDE.md requires those to be reviewed
through `codex` before execution. This directory holds the frozen brief and
the dispatcher for that review.

## Core principle

**The count measures the instrument as much as the plan.** Five things decide
how many findings come back — the brief, the model, the reasoning effort, the
codex version, and whether the review is one pass or several independent
slices. Change any silently and two rounds' numbers stop meaning anything next
to each other, while still looking comparable.

## Run it

The current brief is **v2: bounded design review**. It checks architecture,
cross-task contracts, ownership/safety, specification compliance and the
verification strategy. It does not compile snippets, simulate compilation or
audit every symbol/fixture. Actual compilation, tests and portability checks
remain mandatory during implementation of each task; this change removes no
implementation or CI gate.

The reviewer reads the plan/prior once and uses at most 12 additional targeted
spec/source excerpts of at most 120 lines each, with bounded searches. It must
report incomplete coverage when that allowance is insufficient, not invent a
clean verdict. This is a prompt-level reading allowance, **not an enforced
token, time or dollar cap**. Model, reasoning effort and single-pass dispatch
remain unchanged.

Do not automatically chain correction/review rounds or retry interrupted
reviews. After a pass, verify findings locally, fix the design issues, and
report the remaining specific questions and observed usage to the user. Obtain
explicit approval before another external pass; use that pass for unresolved
design questions, not for eliminating a compiler-error backlog. A changed
instrument does not itself authorize another run. Never launch a nested
reviewer from inside the reviewer.

```bash
docs/superpowers/review/review.sh \
  --plan docs/superpowers/plans/<plan>.md \
  --spec docs/superpowers/specs/<spec>.md \
  --out  docs/superpowers/findings/<date>-<subject>-review.md \
  --prior docs/superpowers/findings/<previous-review>.md \
  --context "Stage 1 is merged. Revision 2 renumbered tasks: the prior
             review's Task 4 is now Tasks 4 and 5." \
  --out-of-scope "  The absent commit owner, call-site conversion, admission,
                  clock-record, damage and completion logic — spec section 18
                  assigns those to 2b and 2c."
```

Omit `--prior` for a first review; the brief then skips its incorporation audit.

## When codex is unavailable

`review-claude.sh` takes the same arguments and sends `brief.md` byte-identical
to what `review.sh` sends, so the reviewer is the only deliberate variable. That
is still enough to break comparability: the reviewer is one of the five things
that decide a count. Results from it are a separate lineage, never a continuation
of a codex round, and its provenance block says so. Two differences beyond the
model are worth knowing when reading its findings: it runs with a read-only tool
set (`Read`, `Grep`, `Glob`) rather than a sandbox, and it inherits the repo's
`CLAUDE.md`/`AGENTS.md` and the user's global instructions, which codex does not.

Prefer it over an ad-hoc inline review: an unrecorded pass produces a count that
means nothing and cannot be cited later. Prefer waiting for codex over either
when the schedule allows and the question is whether a correction round is
converging.

`--context` and `--out-of-scope` are what stop the reviewer reporting deliberate
scope boundaries as defects. Spend real effort on them — the alternative is
findings you have to argue away by hand.

The script pins the model and reasoning effort on the command line rather than
inheriting `~/.codex/config.toml`, which lives outside the repo and can change
without anyone noticing.

## The findings document

The script prints a provenance block when it finishes. **Paste it in.** A review
whose findings document does not name its instrument SHA cannot be compared to any
other round, which is the same as not having measured anything.

Then, before filing: **check the blocking findings against the tree yourself.**
Reviewer claims about existing code have been wrong before in this project — a
finding is evidence, not a verdict. Record which ones you verified.

Preserve the coverage classification and deferred implementation checks. A
zero-finding INCOMPLETE result is not a clean review. A process interrupted by
quota, cancellation or another failure has no completed verdict: preserve its
log, do not fabricate findings/provenance, and ask before retrying.

## Changing the brief

Editing `brief.md` **or `review.sh`** resets comparability — the script carries
the model, the reasoning effort and the mode. So:

- Change it in its own commit, touching only this instrument directory
  (including its documentation), not the plan, findings or implementation.
- Say in the message what changed and why.
- Note in the next findings document that counts before and after are not
  comparable.

`review.sh` refuses to run against any uncommitted change in this directory, so
this is enforced rather than remembered.

## Lineage

| Brief | Reviews under it | Comparable to |
| --- | --- | --- |
| Pre-v1: 3 checks, never preserved | stage 2 monolith rounds 1-2 (24, 26 blocking); stage 2a round 1 (8) | nothing — text lost |
| Pre-v1: 5 checks, never preserved | stage 2a round 2 (10 blocking) | nothing — text lost |
| v1, instrument `38aee673` | Includes stage 2b-ii rounds 1–2 on 2026-09-06 | reviews citing that instrument only |
| **v2**, bounded design review (2026-09-06) | Starts after this instrument change is committed | reviews citing the new instrument SHA only; never v1 |
| **v2 / claude dispatcher** (2026-09-10) | Reviews run through `review-claude.sh` while codex is unavailable | other `review-claude.sh` reviews citing the same SHA; never a codex round |

v1 reconstructed the five-check brief used for stage 2a round 2. Earlier briefs
were not preserved, so their historical counts are not comparable. v2 removes
the exhaustive baseline/symbol audit and simulated compilation, bounds reading,
and requires explicit coverage limits. It follows the project lesson: review
the design, then use the compiler and tests while executing the plan. This is
a deliberate instrument change; v1 and v2 counts are not a quality trend.

The motivation is recorded usage, not a dollar estimate: the stage 2b-ii v1
logs reported 261,215 tokens for round 1, 197,542 for round 2, and 179,003 for an
interrupted round-3 attempt. Those figures exclude the author session and a
later cancelled retry. The old brief required whole-plan compilability analysis
and exhaustive baseline checking; the new scope avoids requiring that work in
each review. Actual savings must be measured, not assumed.

## Common mistakes

| Mistake | Consequence |
| --- | --- |
| Writing a brief inline instead of running the script | Unpinned model and effort; no SHA; another orphaned round |
| Comparing a `review-claude.sh` count to a codex count | Different reviewer, different lineage; the trend is imaginary |
| Leaving `--out-of-scope` empty | Findings about work deliberately deferred to a later stage |
| Filing without the provenance block | The result is uncomparable and nobody can tell |
| Reading a count as a trend | Only valid across reviews citing one instrument SHA |
| Taking blocking findings as verdicts | Verify against the tree first; some do not reproduce |
