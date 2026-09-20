# Stage 2c-iii, plan Cii — accepted

**Plan:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md`,
revision 6, after three codex review rounds (1B 4M, 1B 1M, 1B 2M) and two
revisions made during implementation against measured F8 stops.
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`, run
**without sandbox** so it could run the `_vulkan` tests itself; the coordinator
verified each task outside it, ran the mutations and committed.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — the Present-carrying owner entry | `f9e38620` | no |
| 2 — one production eligibility predicate | `a9dfc41e` | once: the tests drove the pure combination helper, so both of the task's mutations survived at the gathering site |
| 3 — the layout hooks, enumerated first | `726abdf7` | no |
| 4 — the producer's fork and its module | `3d8badde` | no |
| 5 — members and leases from the producer | `d274141c` | no |
| 6 — the direct commit's Present carriage | `83509224` | once, on two F8 stops (plan revision 5) |
| 7 — composed damage untouched, cursor invariants | `af02a092` | once, on one F8 stop (plan revision 6) |
| evidence gaps from the parity run | `65bd21b4` | — |

## The three F8 stops, and what they changed

Each was raised by the implementer, verified by the coordinator in the code, and
answered by amending the plan — never by bending the code.

1. **The clock probe cannot supply a stamp.** `ProbeOutcome::Ready { reference }`
   carries a sequence reference, not a sample. Revision 4's "ask for a clock
   probe and admit when it resolves" was impossible; removed.
2. **A Present-carrying commit cannot retire without `Presented`**
   (`owner/record.rs:400`). The "accepted Present lacking validated
   presentation" of stage 2c §3 therefore resolves through the completion
   deadline into `CompletionUnknown`, and the `Skip` belongs to that terminal
   path — stamped with the reference CRTC's last validated sample when one is
   known, and unstamped when none is, exactly as the legacy never-submitted
   `Skip` already publishes (`backend.rs:2761`). Plan revision 5.
3. **Direct entry does not invalidate composed damage in the merged base.**
   Spec §5.5 and DMG-5 say it does "as the merged base already does"; it does
   not. The only device-wide invalidation on that path is on the **return**, at
   composed-unflip retirement (`backend.rs:3374`), which
   `invalidate_all_scanout_damage`'s own doc comment states. **Spec correction
   recorded**: DMG-5's entry half does not describe the base. Cii owes only that
   no direct milestone applies composed damage and that entry matches Legacy;
   the return-path invalidation goes to Ciii with the unflip. Plan revision 6.

## Mutations (coordinator, applied by line at `65bd21b4`)

S1–S31 all **caught**. Two needed their site corrected while applying, and both
corrections are recorded because they say something about the code:

- **S18** — the retirement wake has two call sites; at the one the named test
  drives (`backend.rs:20200`) the mutation fails the test.
- **S23** — an unchanged cursor generation is not dropped by the decider's
  readiness condition the map pointed at, but by `Admission::set_maintenance`
  (`owner/admission/intents.rs:213`), which removes a generation equal to the
  current one before any decision runs.

Two mutations survived the first parity pass and were closed as **evidence
gaps**, not production defects (`65bd21b4`): S20 needed the predecessor's
completion to be driven before a second idle could be observed, and S23 needed
the site above. **S30** stays **equivalent**, as Task 1 recorded: with the
fast-update refusal removed, the completion-context validation refuses the same
descriptions downstream — the mechanism that made Ci's R3 equivalent. **S32** is
obsolete: revision 5 removed the rule it targeted.

## Checks at `65bd21b4` (coordinator, outside codex, GPU idle)

fmt; clippy default, `tcp-transport`, `xdmcp`; `c0_conv_cii_` **26/26** with
`--include-ignored` in debug and release; `c0_conv_ci_` 38/38 and `c0_conv_cir_`
7/7 on the GPU; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1933/0/142;
`cargo check` for Linux glibc, Linux musl and FreeBSD.

## Hardware gate — 2026-09-20, ACCEPTED

Run with the GPU free (no GPU processes before or after):
`render_acceptance -- --ignored` **164/164**; `c0_2ci -- --ignored` **21/21**;
the library's other ignored tests **121/121**. **306/306 in total**, against the
287/287 of the Ci-refactor acceptance plus this plan's nineteen new `_vulkan`
tests.

**Plan Cii is accepted at fixture level.** The direct producer now has an
`Owner` half behind the same transport fork as the composed one: one production
eligibility predicate, fifteen enumerated layout-hook sites, the producer's own
module behind a single fork point, members and present-pin leases carried by
value into the commit, the commit's Present carriage with its two clock rules,
and the cursor invariants. `Legacy`, which production still uses, is unchanged.

## Carried to Ciii

- The composed-unflip return path's invalidation of composed buffers (F8 3).
- Unflip dispatch, multi-device conductor state, the copied composed route and
  route-selection exclusivity, with the tty2 hardware run of spec §6.4.
- The Ci F8 stops stay open: the Legacy dormancy bug, the missing restore
  `TerminalState`, device loss without an owner signal, and the copied-route
  fixture.
