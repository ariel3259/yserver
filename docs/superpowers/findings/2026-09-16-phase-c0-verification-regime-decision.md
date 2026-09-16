# Phase C.0 verification regime — the soak replaced by runtime qualification

**Retroactive decision record.** Written 2026-09-16 by Opus at the user's
request, for a decision taken on 2026-09-10 and never recorded as a finding.
Dating this file to the day it was written is deliberate: back-dating it would
suggest the record existed before the implementation it preceded, and it did
not.

## The decision

Commit `39c9b75e`, **2026-09-10 20:05 -0300**,
`docs(c0): replace the hardware campaign with runtime qualification`, rewrote
section 16.3 of `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
as "Evidence regime — revision 3" and added an entry to `docs/status.md`.

Before it, section 16.3 required eight-hour zero-poison soaks and per-stratum
coordinate quotas on two nominated boards. After it:

- **The normative gate is runtime qualification, per device incarnation, on
  every boot** — structural discovery through `CompletionCaps`, canonical
  out-fence status, bounded checked deadlines, and fail-closed terminalization
  of the four completion-safety classes (incarnation poison, `HwDetachUnknown`,
  executor/host-call watchdog expiry, a missing or failed off-transition fence).
  The soak's release budget — zero occurrences of those classes — is kept; what
  changes is who enforces it and when.
- **A bounded delivery check replaces the soak** as the physical run. Each
  available device repeats a scripted transition set — DPMS off/on, VT release
  and acquire, direct/composed entry and exit, fullscreen entry and exit, CRTC
  disable — until every transition class has been exercised, with section 15
  telemetry exported. Its purpose is delivery, not rarity: proving the driver
  actually returns a signalled off-transition fence and resolves cursor-plane
  detach, which a deterministic deadline can survive but not observe. There is
  no hour budget.
- **Physical evidence falsifies the machinery; it never certifies a cohort.** A
  physical row may block merge by exposing a defect in shared machinery. It may
  never establish that another driver, kernel or GPU behaves.
- **No campaign is required to merge C.0.** The audited-cohort table ships
  empty, so `OwnerMediatedLegacyMove` stays specified and unreachable.

## Why

From the commit message: the soak cost about eighteen hours of dedicated device
time per campaign on the author's only machine; any repair reran every reachable
row; and it produced no evidence at all for the architectures the project does
not own — Intel, Asahi, other AMD generations, other NVIDIA cohorts. "A project
that cannot run a validation lab does not get a smaller lab; it moves the gate."

## When, relative to implementation

| When | Commit | What |
| --- | --- | --- |
| 09-10 20:05 | `39c9b75e` | Section 16.3 revision 3 |
| 09-10 23:39 | `057dcfb0` | Stage 2c-i handed to the implementing model |
| 09-10 23:46 | `38ce1eb4` | First stage 2c-i implementation commit |

The decision preceded the first implementation of a plan under it by three and
a half hours. It was in force for all of stage 2c-i.

## What was missing, and is recorded here

**1. No finding.** The project records architecture decisions as findings —
`2026-09-02-phase-c0-fixed-executor-architecture-decision.md` is the precedent.
This one lived only in the design, `status.md` and a commit message.

**2. No review.** No findings document mentions `39c9b75e` or revision 3. A
normative change to the verification regime of all of Phase C.0 entered without
the codex review CLAUDE.md requires for specs. Codex was unavailable that day,
but `review-claude.sh` had been added at 19:18 for exactly that case, and was
not run on this change.

**3. It superseded earlier findings without saying so.** These findings require
the soak and still read as binding to anyone who opens them alone. **This
document supersedes their soak requirements**; their other findings are
unaffected.

| Finding | Soak requirement it states |
| --- | --- |
| `2026-08-31-phase-c0-kernel-path-adversarial-review.md` | line 49 "run an eight-hour zero-poison soak"; line 97 "mandatory software-cursor soak remains merge-blocking" |
| `2026-09-01-phase-c0-code-baseline-adversarial-review.md` | line 73 "the complete eight-hour soak" |
| `2026-09-01-phase-c0-post-incorporation-adversarial-review.md` | lines 49, 66, 166 — the eight-hour returnability arm and the final eight-hour soak |
| `2026-09-01-phase-c0-coordinate-concurrency-adversarial-review.md` | line 47 "the eight-hour soak, which then validates the chosen design" |
| `2026-09-02-phase-c0-executor-ipc-cost-measurement.md` | line 236, the eight-hour soak among the rerun rows |
| `2026-09-02-phase-c0-fixed-executor-adversarial-review.md` | line 49, "the eight-hour soak, which cycles DPMS and VT" |

## Consequence the regime carries, and nobody has scheduled

The bounded delivery check is **merge evidence for C.0**, and no plan or stage
has taken it. It exercises real KMS transitions — VT release and acquire, DPMS,
direct and composed entry and exit — through the owner machinery, which exists
in full only once stages 3/4 supply the remaining production writers. It
therefore needs:

- **DRM master on the device under test.** The deterministic and hardware test
  fixtures deliberately open the card without master (`backend.rs` around line
  6003), so the kernel rejects page flips and the flip-accepted path — a buffer
  actually going on screen, a real completion event, a real out-fence — never
  runs on this box today. On this machine that means running from a VT that is
  the active session on the seat, such as tty2, with no other master on the
  card.
- **Scheduling with stages 3/4**, since its transitions go through machinery
  those stages complete.

An early, partial instance — the flip-accepted path of the stage 2c-i ledger,
run with master — is possible before then, but it is not the delivery check and
must not be reported as one.
