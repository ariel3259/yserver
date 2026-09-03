# Phase C.0 — executor IPC cost measurement

**Date:** 2026-09-02
**Subject:** sections 4.1, 7.1 and 16.3 of
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Disposition:** Measurement complete. Twelve changes were applied on
2026-09-02, across sections 4.1, 7.1, 16.3, 17, 18 and 19; all twelve are
enumerated below with their antecedents. Items 11 and 12 came from the
adversarial review recorded in
`2026-09-02-phase-c0-fixed-executor-adversarial-review.md`

## Question

Section 16.3 gates `executor IPC p99` at one millisecond. That bound was
identified in review as the one number that could still invalidate the fixed
executor architecture of section 4.1, because the coordinate path pays an IPC
round trip that the shipping in-process legacy cursor path does not. If the
transport cost is a material fraction of the coordinate budget, the design
needs rework, and it is far cheaper to know before implementation.

This finding measures the transport term. It does not measure the ioctl or the
real input path.

## Method

Two harnesses model the owner/executor split described in `COMMIT-5`: a
single-threaded parent standing in for the X11 core, a forked child standing in
for `KmsIoExecutor`, and one `SOCK_SEQPACKET` socketpair carrying framed
64-byte requests and 32-byte typed replies. The child performs no ioctl; where
an arm names an ioctl cost, the child burns that time deliberately so the
transport term can be separated from it.

`ipcbench` issues one request class at a fixed 1000 Hz cadence and reports,
per iteration, the interval from the request becoming due to `sendmsg`
returning (`due->dispatch`), from dispatch to the typed reply (`IPC rtt`), and
the sum (`due->reply`).

`ipcbench2` adds the quantity the design actually constrains. Section 7.1
holds `CoordinateSubmitting` from before dispatch until the typed return or the
reap, and forbids any new host call while a coordinate call is unresolved. It
therefore runs two classes over one channel — coordinate at 1000 Hz, primary at
the refresh rate, the primary's reply carrying an out-fence by `SCM_RIGHTS` —
and reports channel occupancy plus how many primaries fell due inside a live
coordinate reservation.

Load conditions are stated per arm. `spin=N` runs N unrelated CPU-bound
processes for the whole arm; the box has 16 hardware threads.

## Environment

Ryzen 7 7700 (8C/16T), kernel `7.2.2-gentoo-dist`, governor `powersave`, with
the maintainer's ordinary desktop session running. This is the development box,
not a release cohort device. Governor and ambient load are both conservative:
a `performance` governor and a quiet box would improve these numbers.

## Results — transport in isolation

20,000 samples per arm at 1000 Hz. All figures microseconds.

| Arm | due->dispatch p99 | IPC rtt p50 | IPC rtt p99 | IPC rtt p99.9 | IPC rtt max |
| --- | --- | --- | --- | --- | --- |
| idle, core blocked in sleep | 6.3 | 3.3 | 8.4 | 10.6 | 16.7 |
| idle, core hot | 0.8 | 1.9 | 3.1 | 7.8 | 10.4 |
| core 50% busy | 11.3 | 2.1 | 8.1 | 11.3 | 45.0 |
| system load 8/16 | 11.9 | 2.6 | 8.9 | 18.8 | 51.5 |
| core 50% + load 8/16 | 11.7 | 2.7 | 4.9 | 9.0 | 12.8 |
| fd-passing + load 8/16 | 11.7 | 3.7 | 9.2 | 11.0 | 144.0 |
| helper ioctl 50 us + load 8/16 | 11.7 | 52.7 | 54.3 | 56.3 | 66.6 |
| **system load 16/16** | **1992.6** | **4.2** | **978.1** | **994.3** | **1990.6** |

## Results — coordinate reservation with competing primary traffic

20 seconds per arm, coordinate 1000 Hz, system load 8/16.

| Arm | occupancy | primaries due inside a live reservation | coord rtt p99 | coord due->dispatch p99 |
| --- | --- | --- | --- | --- |
| primary 60 Hz, no ioctl cost | 0.353% | 1 / 1201 | 8.8 | 8.5 |
| primary 240 Hz, no ioctl cost | 0.437% | 1 / 4801 | 8.5 | 13.2 |
| primary 240 Hz, primary ioctl 200 us | 5.260% | 1 / 4801 | 8.5 | 206.4 |
| both ioctls costed (50 us / 200 us) | 10.245% | 1 / 4801 | 58.1 | 206.4 |

## Findings

**M-1 — the IPC term is not a design risk.** Round-trip p99 is 5 to 9
microseconds under every load condition short of full CPU oversubscription:
two orders of magnitude under the section 16.3 bound. The arm with a 50 us
simulated ioctl confirms it from the other side, returning 52.7 us at p50, so
the transport contributes roughly 2.7 us additively and the ioctl dominates
entirely. Returning an out-fence by `SCM_RIGHTS` costs a further 0.3 us at p99.
The fixed executor architecture of section 4.1 survives measurement.

**M-2 — the written bound does not measure the transport.** The only arm that
violates one millisecond is 16 CPU-bound processes on 16 hardware threads,
where `due->dispatch` p50 reaches 992.6 us and round-trip p99 reaches 978.1 us.
That is the box having no timeslice to give, not a transport cost, and an
in-process core would be equally starved. A criterion that can only be
violated by scheduler starvation does not discriminate between the two
architectures, which was the sole reason the number existed. It must be split
into a transport criterion and a saturation criterion, and the saturation text
must say the result is not attributable to the executor.

**M-3 — the tail has no owner.** Round-trip p99.9 stays between 9 and 19 us,
but the maximum reaches 233.8 and 236.5 us in the two arms with zero ioctl
cost, so that excursion is transport and scheduling, not driver work. Section
6.3 defines `CoordinateFastReturnMax` on the helper-measured ioctl duration
"independently of IPC", so it cannot absorb this tail; nothing downstream owns
it. A tightened p99 without a tail bound would leave a quarter-millisecond
excursion unowned in a 1000 Hz path, where an event every few thousand samples
is perceptible even though it breaks no gate.

**M-4 — the reservation is not a throughput constraint.** With no ioctl cost,
channel occupancy is 0.353% at 60 Hz and 0.437% at 240 Hz, and exactly one
primary in 1201 and one in 4801 fell due inside a live coordinate reservation.
Primary `due->dispatch` p99 is 11.6 us, indistinguishable from the coordinate
class's own, so no induced block is measurable.

**M-5 — the blocking term is the ioctl, not the isolation.** Giving the primary
a 200 us ioctl raises occupancy to 5.260% and coordinate `due->dispatch` p99 to
206.4 us: the coordinate call waits behind the primary's ioctl while the
transport stays at 8.5 us. That pressure exists identically in process, and it
is exactly what section 7.1's coordinate-overlap rule exists to relieve.
Process isolation contributes single-digit microseconds to it. This is the
finding that closes the architecture question; see the conclusion below.

**M-6 — a prior concern is withdrawn.** An earlier review round argued that the
95% coordinate-updates-per-second retention gate of section 16.3 could fail
because the IPC round trip serialises the owner against an in-process baseline
that has none. At a 5 us round trip the serialisation ceiling derived from
round-trip latency is of order 10^5 per second against 1000 Hz of offered
input. The argument assumed the written one-millisecond bound was
representative of the real cost; it is not, by two orders of magnitude.

**M-7 — "submit" is undefined.** Section 16.3 gates `input-to-submit p99` at
one output period plus 2 ms while item 4 of the same section records
`total input-to-dispatch overhead` as a separate metric, implying the two
differ. If *submit* is the owner's dispatch to the executor, both the round
trip and the ioctl fall outside the gate; if it is the accepted ioctl return,
both fall inside. The difference between the readings is precisely the quantity
under debate. Magnitude makes this less urgent than it looked, but the term
still needs a definition.

**M-8 — the gate is mis-constructed for high-refresh cohorts.** With real
numbers the fixed floor is about 0.1% of the budget at 240 Hz. But the gate as
written permits 1 ms of IPC plus 1 ms of `CoordinateFastReturnMax` inside
`input-to-submit p99 <= one output period + 2 ms`, which is 10.7% of the budget
at 60 Hz and 32.4% at 240 Hz. A high-refresh cohort could fail by construction
rather than by measurement. Tightening the written bounds to measured reality
resolves this and M-2 together.

## Conclusion — what process isolation costs

Section 4.1 became a design decision rather than an empirical claim, which left
one quantitative question open: what does the isolation cost. The measurement
answers it. Per host call, single-digit microseconds. At 1000 Hz, 0.353% to
0.437% channel occupancy. Returning an out-fence by `SCM_RIGHTS`, a further
0.3 us at p99.

The dominant term in the coordinate path is the ioctl held by whatever call is
in flight, and **that term does not change between architectures**. An
in-process owner waits on the same ioctl, on the same single thread, with no
crash containment, no watchdog and no bounded reap. The costed-ioctl arms make
the shape visible directly: coordinate `due->dispatch` p99 of 206.4 us against
a transport term of 8.5 us in the same arm. Because `ipcbench2` serialises more
strictly than the design does, that 206.4 us is an upper bound on a
configuration section 7.1's coordinate-overlap rule exists to avoid, so the
production path sits below it.

A reviewer who doubts the executor will ask what the isolation buys and what it
costs. The measured answer is microseconds, against a dominant term that is
architecture-invariant. This does not reopen the section 4.1 decision — it was
never resting on a returnability claim — but it removes the one quantitative
objection that could have been raised against it.

## Spec changes applied

Items 1 to 5 were proposed by this finding and applied after approval. Items 6
to 10 carry no independent justification: each exists only because an earlier
item would otherwise be incomputable, unattributable or orphaned in the
document. They are recorded here so that no applied change lacks a written
antecedent.

1. **Section 16.3 — three named transport criteria** replace
   `executor IPC p99 must not exceed 1 ms`. `ExecutorTransportP99Max = 50 us`
   and `ExecutorTransportP999Max = 100 us` bound dispatch-to-reply with the
   class's own helper ioctl duration excluded, which preserves the disjointness
   section 6.3 established. `ExecutorSchedulingSaturation` retains the
   one-millisecond figure but as an evidence-validity check whose text states
   the result is not attributable to the executor. *Antecedent: M-1, M-2.*
2. **Section 16.3 — `ExecutorTransportExcursionCeiling = 500 us`**, a
   characterization ceiling whose consequence is proportional to frequency
   (failing above 0.01% of an arm's samples) rather than zero-tolerance, and
   whose text attributes the excursion to transport and scheduling rather than
   to the driver. *Antecedent: M-3.*
3. **Section 16.3 — *submit* defined** as the instant the owner installs the
   `Submitting` or `CoordinateSubmitting` record and dispatches, containing
   neither the round trip nor the ioctl. *Antecedent: M-7, M-8.*
4. **Sections 4.1 and 7.1 — kernel audit expressed as a range** over five named
   functions, with the commit degraded to reading provenance and a rule that a
   cohort kernel outside the range requires the same five-function comparison.
5. **Section 7.1 — DCN native-cursor exemption list** is 4.0.1, 4.2.0 and, from
   7.2, 4.2.1, with the disabled-CRTC early return recorded. Navi 21 conclusion
   unchanged.
6. **Section 16.3, table item 4 — record p99.9 and excursion counts.**
   *Antecedent: items 1 and 2.* The table recorded `p50/p99/max` only, so
   neither `ExecutorTransportP999Max` nor the 0.01% excursion-frequency rule
   could be evaluated from the recorded evidence. A criterion that cannot be
   computed from the evidence table is not a criterion.
7. **Section 19 — references record the range.** *Antecedent: item 4.* Section
   19 still listed the local source at that commit as a source of record, which
   contradicted the degraded status the same commit now has in section 4.1. The
   entry also warns that a stripped distribution kernel tree carries no `.c` or
   `.h` file, because that trap produced a false negative during this work.
8. **Section 4.1 — the measured cost of isolation.** *Antecedent: the
   conclusion of this finding.* Section 4.1 justified the executor only from the
   risk side. The quantitative answer now sits in the document a reviewer
   actually reads, with a citation here for the arms and their limits.
9. **Sections 16.3 and 18 — row vocabulary and failure mapping.**
   *Antecedent: items 1 and 2.* Those criteria were written as failing "the
   transport row", a term this document does not define; its row vocabulary is
   coordinate-policy, performance and soak. Section 16.3 now fails the affected
   performance row, and section 18 maps each transport criterion to its row and
   repair scope and states that `ExecutorSchedulingSaturation` is not a failure
   and repairs nothing.
10. **Section 17 — acceptance bullet names the new thresholds.**
    *Antecedent: items 1 and 2.* The performance bullet enumerated what the
    table must pass and did not include the transport criteria, so section 17
    and section 16.3 disagreed about what the table is gated against.

11. **Section 16.3 — the saturation check excludes the class's own ioctl.**
    *Antecedent: adversarial review A-1, and item 1 of this list.* The two
    transport bounds carried that exclusion; the saturation criterion did not.
    `modeset/install/recovery` is a section 4.1 call class whose watchdog is 30
    seconds precisely because link training and DC dependency waits run for
    hundreds of milliseconds, so its raw dispatch-to-reply p99 exceeds one
    millisecond on a perfectly idle host. As written, every arm containing a
    modeset — hotplug, VT cycling, cold start, topology and the eight-hour soak
    — would have become `EvidenceInsufficient` with an instruction to rerun on
    a less loaded host, which no rerun could satisfy.
12. **Section 16.3 — the transport bounds carry their derivation.**
    *Antecedent: adversarial review A-3, and items 1 and 2 of this list.* 50 us
    and 100 us are normative for every cohort and block merge, but were derived
    from one host measured in this finding. The spec now states the basis and
    its margin, and requires a materially slower cohort host to record its own
    basis by the same route the cohort kernel range uses instead of inheriting a
    number measured elsewhere.

### Consistency pass

After all ten edits, no reference to a one-millisecond IPC bound survives as a
transport criterion. The only remaining millisecond values in the document are
`CoordinateFastReturnMax` in section 7.1, which is the helper-measured ioctl
duration and is unchanged, and the `ExecutorSchedulingSaturation` threshold in
section 16.3, which is an evidence-validity check and says so. Each of the four
new identifiers is defined once and consumed in section 16.3, section 17 or
section 18.

## Audited kernel range

Section 16.3 requires exact kernel, module and source identity for physical
evidence, so this measurement's provenance is recorded here.

The concurrency premise of section 4.1 is pinned to Linux
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`. That commit is the tip of the local
Asahi checkout at `~/Projects/linux`, tag `asahi-7.1.9-1`; its subject is
`mfd: macsmc: Add second gpio subdevice for 'gp00' keys`, which is not a
mainline DRM reference point. A reviewer resolving that SHA against
`torvalds/linux` gets a 200 because GitHub shares object storage across a fork
network, not because the commit is in mainline.

`/usr/src/linux` on this box is `linux-7.2.2-gentoo-dist` and is a stripped
dist tree: zero `.c` and zero `.h` files. It cannot serve as a source
reference, and grepping it yields false negatives.

Five functions carry the premise. All five were compared between the audited
tree and vanilla 7.2, extracted from `/var/cache/distfiles/linux-7.2.tar.xz`:

| Function | Result |
| --- | --- |
| `drm_atomic_helper_async_check()` | identical modulo type rename |
| `drm_atomic_helper_setup_commit()` | identical modulo type rename, same line 2511 |
| `drm_atomic_add_affected_planes()` | identical modulo type rename |
| `amdgpu_dm_atomic_check()` modeset/color-mgmt/VRR/`dsc_force_changed` guard | byte-identical |
| `dm_crtc_get_cursor_mode()` | adds `IP_VERSION(4, 2, 1)` to the native exemption list and an early native return when `!dm_crtc_state->base.enable` |

7.2 renames `struct drm_atomic_state` to `struct drm_atomic_commit` across the
atomic helpers. This is API churn with no bearing on any C.0 conclusion.
Neither `dm_crtc_get_cursor_mode()` change affects Navi 21 with an enabled
CRTC.

The 7.2 to 7.2.2 delta is closed: `patch-7.2.2.xz` touches 91 files and none
under `drivers/gpu` or matching `drm`. The audited facts therefore hold across
**7.1.9 through 7.2.2 for those five functions**, which covers both the audited
tree and this development box.

One gap remains and it is not resolvable here: the release cohort for `CAP-4`
is the maintainer's RX 6800 XT, so the cohort kernel is whatever that box runs.
If it falls inside the verified range there is nothing further to do; if it
falls outside, the comparison procedure above is the one to repeat.

## What this does not establish

- It does not measure the real input path or a real ioctl. Only the transport
  term is bounded; `total input-to-dispatch overhead` still requires
  instrumented yserver on a VT.
- It ran on the development box's CPU, not a cohort device's. Transport cost
  depends on CPU and kernel rather than GPU, so it should transfer, but the
  cohort box's CPU is unknown. The `powersave` governor and the live desktop
  session make the measurement conservative rather than optimistic.
- The `ipcbench2` harness serialises strictly: one request in flight across
  both classes. The design permits a coordinate call to overlap one accepted
  overlap-safe primary, so the 206.4 us coordinate `due->dispatch` p99 in the
  costed-ioctl arms is an upper bound on a configuration section 7.1 exists to
  avoid, not a prediction of production behaviour.
- The `achieved updates/s` field of `ipcbench` is pinned by the harness's own
  cadence and is not evidence of a throughput ceiling. The ceiling quoted in
  M-6 is derived from round-trip latency, not measured directly.
- The stub helper performs no ioctl, so real syscall entry and exit costs are
  outside every arm except through the deliberately injected ioctl times.

## Reproduction

Sources and raw output are committed beside this finding in
`docs/superpowers/findings/2026-09-02-executor-ipc-cost/`: `ipcbench.c`,
`ipcbench2.c` and the unedited `results.txt` / `results2.txt` the tables above
were read from. Both build with `gcc -O2 -Wall -Wextra` and take no arguments
beyond the ones each arm names in its own header line. They are characterisation
harnesses, not merge candidates, and produce no production evidence row.
