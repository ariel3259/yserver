# Phase C.0 — adversarial review of the fixed-executor rewrite (full document)

**Date:** 2026-09-02
**Subject:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
(Draft of 2026-09-02, fixed-executor dispositions incorporated)
**Reviewer:** independent Opus session, whole-document read
**Disposition:** All five applied 2026-09-02. A-4 was applied as a three-part
disposition after two rounds of correction on both sides; see the disposition
notes at the end of this file. Verdict: no blocking findings.

## Verdict

No blocking findings. The rewrite is clean: no orphaned `when selected` branch,
no surviving `InProcessQualified` path, no residue of `CursorCompositionKey`,
`CoordinateCompositionPredicate`, `CoordinatePolicyDefectCandidate` or
`FlipDrivenSoftware`. `EvidenceInsufficient` survives correctly reassigned from
architecture selection to evidence-row validity. Section 4.1 now carries both
halves of its argument — the opacity/stall risk that motivates isolation and the
measured cost that makes it cheap — with the measurement cited rather than
asserted.

Four findings follow. A-1 will stop an evidence campaign if it ships as written.
A-2 and A-3 are corrections to claims that lost their antecedent in the rewrite.
A-4 is a design trade worth reopening once. One minor at the end.

## Findings

### A-1. `ExecutorSchedulingSaturation` auto-invalidates every arm containing a modeset

Section 16.3 defines the three transport criteria. The two bounds are scoped
correctly:

> `ExecutorTransportP99Max = 50 us` and `ExecutorTransportP999Max = 100 us` bound
> dispatch-to-reply latency for every section 4.1 call class, **measured with
> that class's own helper ioctl duration excluded**.

The saturation criterion drops that qualifier:

> A dispatch-to-reply or input-to-dispatch p99 at or above one millisecond means
> the measurement host had no timeslice to give.

`modeset/install/recovery` is a section 4.1 call class. Section 6.3 gives it a
30-second watchdog precisely because link training, MST topology work and DC
dependency waits run for hundreds of milliseconds. Its raw dispatch-to-reply p99
is therefore three or more orders of magnitude above one millisecond, with a
perfectly idle CPU.

Read literally, any arm that contains a modeset — the hotplug matrix, VT cycling,
cold start, the topology rows, and the eight-hour soak, which cycles DPMS and VT
— trips `ExecutorSchedulingSaturation`, becomes `EvidenceInsufficient`, and
carries an instruction to "rerun on a host that is not CPU saturated." No rerun
fixes it, because the cause is the ioctl, not the scheduler.

**Requested disposition.** Apply the same exclusion the transport bounds use, or
scope the saturation check explicitly to the latency-critical classes
(coordinate, cursor-only, primary-only, gamma-only) where a millisecond of
dispatch-to-reply genuinely can only mean starvation. The first is the smaller
edit and keeps the criterion general.

### A-2. The NVIDIA legacy-HW arm asks a question the executor makes unanswerable

Section 16.3's four-arm table:

> **Legacy hardware cursor** — Does the historical 11.5 ms mean / 16.3 ms max
> X11-core block still reproduce through the helper's ordinary fallback?

Under the fixed executor the call runs in the helper process. There is no X11
core to block. The arm cannot reproduce a core block because the architecture it
runs on structurally prevents one — which is the whole point of section 4.1.

What the arm can still measure is the driver wait's magnitude, and that number is
worth having: it is the input to the `SynchronousAtomicMove` expectation stated
three paragraphs later, and it is the empirical half of section 2's
justification. But as written the arm promises an observation it cannot produce,
and a reviewer comparing it against section 4.1 will read a contradiction.

**Requested disposition.** Reword the question to the driver-wait magnitude, and
name what consumes it. One sentence noting that the same wait no longer reaches
the core, and that this is the executor's intended effect rather than a
measurement failure, converts the apparent contradiction into supporting
evidence.

### A-3. A global normative transport bound derived from one CPU

`ExecutorTransportP99Max = 50 us` / `ExecutorTransportP999Max = 100 us` are
normative for every cohort, and exceeding either fails the transport row, which
blocks merge.

Their derivation is a measurement on a Ryzen 7 7700 with the `powersave`
governor. The finding records that caveat and correctly calls the conditions
conservative rather than optimistic. The spec does not carry it. The RX 6800 XT
is the maintainer's board and its CPU, scheduler preemption model and mitigation
configuration are unknown; on a materially slower host, or one with different
preemption settings, 50 us at p99 could fail for reasons that have nothing to do
with yserver's IPC design.

The consequence is disproportionate in the familiar way: a number chosen with 5x
margin over one host's measurement becomes a merge blocker on a host nobody has
measured.

**Requested disposition.** Carry the derivation into section 16.3 in one
sentence — measured p99 of 5-9 us under load on a named CPU class with a stated
margin — so a reviewer can judge transferability instead of treating 50 us as an
axiom. If the maintainer's host turns out materially slower, the alternative is
to record the bound per cohort with a stated floor, the same way the cohort
kernel range works. Either way the number should be traceable to its basis from
inside the document.

### A-4. `ShutdownExecutorStalled` trades a real failure for a barrier the OS already provides

`COMMIT-7` requires the parent to retain quarantine and its teardown supervisor
until `waitpid`/`waitid` proves every helper terminated, and states that under a
wedged kernel call "orderly process exit may remain in
`ShutdownExecutorStalled` indefinitely; the spec promises prompt logical
shutdown, not bounded exit under a wedged kernel."

A helper stuck inside an uninterruptible NVKMS ioctl does not exit on `SIGKILL`
until the call returns. So on the exact hardware whose opacity motivated the
executor, yserver's process never exits. What actually happens next is that the
service manager's stop timeout expires and it `SIGKILL`s the process group —
an outcome strictly less orderly than a planned bounded exit, arrived at by
refusing to plan one.

The barrier being protected is that no new incarnation may open the device until
the old `IncarnationFdSet` is closed. Across a process restart the kernel
provides that: when the wedged helper eventually dies, its fds close, and the
next yserver start opens a new file description with no shared state carrying the
old lease count. The rule that guards it is an in-process invariant and is
vacuous across restarts.

**Requested disposition.** Give shutdown a bounded path. After the teardown
deadline, stop waiting: the helper is already reparented to init, which will reap
it; record the unreaped lease in whatever persists (a log line is enough, since
nothing in-process survives), and exit. Keep the indefinite retention rule for
`ExecutorStalled` during a live session, where the invariant it protects is real
and logical withdrawal keeps the server usable. The two cases have different
costs and should not share a policy.

## Minor

### m-1. Nothing forbids the executor from reading the DRM event fd

Section 10 says C.0 "replaces the baseline `receive_events()` drain as a whole;
it does not race a second reader." That is stated against the baseline's own
in-process parser.

The executor holds a registered alias of the same open file description, so both
processes share one kernel event queue on one `drm_file`. A single read in the
helper — a debug drain, a flush before termination, a diagnostic added later —
silently consumes completion events the owner will then never see, and the
failure presents as a missing out-fence or completion-deadline expiry, which
poisons the incarnation.

One line making it explicit that the executor never reads the DRM event fd, and
that event drain is owner-exclusive for the incarnation, costs nothing and closes
a failure mode that would be very hard to diagnose from its symptom.

## Suggested order

1. A-1 before any device time is scheduled; it is the only finding that stops a
   campaign.
2. A-2 and A-3 as text edits in section 16.3.
3. A-4 as a bounded-shutdown rule in `COMMIT-7`.
4. m-1 as one sentence in section 10.

With those applied I see no reason not to move Status to Approved. The remaining
open item — the RX 6800 XT's kernel — is merge evidence, not an approval gate,
and section 4.1 already defines the procedure for a cohort kernel outside the
verified range.

---

## Disposition

**A-1 — applied.** The exclusion was added to `ExecutorSchedulingSaturation`.
The same defect was present in `ExecutorTransportExcursionCeiling`, which this
review did not name: a lifecycle class whose ioctl legitimately runs for
hundreds of milliseconds would have produced a permanent excursion from its own
duration while the criterion's own text claimed excursions are never
attributable to the driver. Both now exclude the class's helper ioctl duration
on the same basis as the two bounds.

**A-2 — applied.** The arm's question is now the driver-wait magnitude, with the
containment stated as section 4.1's intended effect and the consumers named.

**A-3 — applied.** Section 16.3 now states the basis, the host class, the
governor and the margin, and requires a materially slower cohort host to record
its own basis by the same route the cohort kernel range uses.

**m-1 — applied.** Section 10 now states that the executor never reads the DRM
event fd and that drain is owner-exclusive for the incarnation, with the symptom
named.

**A-4 — applied as a three-part disposition, after two rounds of correction.**

The review's original premise, that the in-process invariant is vacuous across
restarts, was too strong: reap retires more than a lease count, because a wedged
helper still holds an open file description and its ioctl can still be accepted
and mutate KMS state after it returns. That correction was accepted.

It was then corrected in turn, and the second correction is the one that decided
the outcome. `SIGKILL` from a service manager's stop timeout does not kill the
wedged helper, but it does kill the parent, which is interruptible in `waitid`
and is the only thing holding the window shut. So "indefinite hang with the
window closed" was never one of the branches. Both branches end in the same
residual window; they differ only in whether the moment is chosen and recorded
or arbitrary and silent.

A third fact narrows where the guarantee can live. For a directly opened device
the kernel already closes the window: `drm_setmaster_ioctl()` returns `EBUSY`
while `dev->master` is set, and `drm_master_release()` runs from
`drm_file_free()` for a primary-node client, so a wedged helper holding the last
reference to that `drm_file` stops a new server from taking master at all. Under
seat management — the ordinary desktop case — that protection is absent: the
`drm_file` belongs to logind and yserver's fd, and by inheritance the helper's,
is a dup of it, so logind's `DROP_MASTER`/`SET_MASTER` for the next session act
on the very `drm_file` the wedged ioctl is using. Both functions were verified
identical across the same 7.1.9 to 7.2.2 range as the concurrency premise and
are now cited in section 19.

The applied disposition is therefore in three parts, all written into the spec:

1. The reap barrier remains the preferred shutdown path.
2. At the teardown deadline the parent exits, records the unreaped lease, helper
   identity and device, leaves the helper orphaned for `init` to reap, and never
   claims the lease was released.
3. The real guarantee is a device-scoped advisory lock — one `flock` per DRM
   device or its platform equivalent — held by the executor for as long as it
   lives. Every server start consults it before installing any state and waits or
   refuses while it is held. It survives `SIGKILL` and reparenting and does not
   depend on the parent still running.

`COMMIT-7` carries all three reasons so the rule is not relitigated. The
indefinite-retention rule for `ExecutorStalled` during a live session is
deliberately unchanged: there the invariant it protects is real and logical
withdrawal keeps the server usable.

Nine sites were edited: `COMMIT-7`, the `ShutdownExecutorStalled` outcome row,
both affected cells of the section 6.4 shutdown row, state-machine tests 68 and
79, the section 16.3 orderly-shutdown hardware row, the section 17 acceptance
bullet, and the section 19 references. A sweep confirms no surviving claim of
indefinite physical exit or of exit conditioned solely on reap.
