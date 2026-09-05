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

## Changing the brief

Editing `brief.md` **or `review.sh`** resets comparability — the script carries
the model, the reasoning effort and the mode. So:

- Change it in its own commit, touching nothing else.
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
| **v1** (`brief.md`) | — | each other |

v1 is a reconstruction of the 5-check brief from the session that produced stage
2a round 2. Every earlier brief was written into a session scratchpad and is
gone, so no historical count in `docs/superpowers/findings/` can be compared to
any other. That is what this directory exists to stop repeating.

## Common mistakes

| Mistake | Consequence |
| --- | --- |
| Writing a brief inline instead of running the script | Unpinned model and effort; no SHA; another orphaned round |
| Leaving `--out-of-scope` empty | Findings about work deliberately deferred to a later stage |
| Filing without the provenance block | The result is uncomparable and nobody can tell |
| Reading a count as a trend | Only valid across reviews citing one instrument SHA |
| Taking blocking findings as verdicts | Verify against the tree first; some do not reproduce |
