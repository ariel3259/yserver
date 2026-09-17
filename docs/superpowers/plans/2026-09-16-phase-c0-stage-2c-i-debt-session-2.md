# Stage 2c-i debt, session 2 — mechanism changes Implementation Plan

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. Execute tasks in order, one at a time; tick steps (`- [ ]` → `- [x]`) only with the evidence each names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. Steps marked **[H]** need GPU and DRM access, which this sandbox does not have: at an [H] step, stop and hand off. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. At each "hand off for commit" step, stop with the tree dirty; the coordinating session verifies and commits with the message given.

**Revision 5 (2026-09-17)** — the extractor in *How to apply a block* contained backticks; run inside `bash -lc "..."` they were read as command substitution, the pattern came back empty, and codex stopped under F8 at Task 4 Step 1. It is now written to a file first and builds its fence with `chr(96)`, so it carries no backtick at all. Tasks 1-3 were committed before this and are unaffected.

**Revision 4 (2026-09-17)** — Task 2's terminal-close test referenced `permit_for`, which Task 4 introduces, and a permit needs the evidence API Task 4 reshapes; codex stopped at it under F8 while executing Task 2. The test is split in two, one half per task, and **every task has now been applied in order with `patch`, compiled and tested at each step** — 171, 172, 173, 180 — which is the check whose absence let a cross-task reference through.

**Revision 3 (2026-09-17)** — incorporates codex round 2 (`…-session-2-plan-review-round2.md`: 2 blocking, 1 major; round 1's B-1, B-2 and B-3 audited APPLIED) on top of revision 2's answer to round 1. See *Corrections from the reviews*.

**Goal:** Land the four mechanism items of spec section 4 — husk accounting bound to identity, propagated GPU unwind errors with the transport close they owe, the reset-boundary test, and handover evidence — and prove family A and `consume_owner_write`'s non-Owner refusal on that evidence, so the census over the resource service reports zero survivors.

**Architecture:** Every code change in this plan was prototyped on `12a32854`, and every task re-applied in order on `a87601c2` with a compile and test run after each by the coordinating session, and the prototype passed the full gate, hardware included (see *Provenance*). The production edits are unified diffs, applied mechanically with `patch`; the tests are verbatim blocks appended to `resources/guard_tests.rs`, plus one new module, `resources/reset_boundary_tests.rs`. Guards get `/// census:` tags so `tools/guard-census.py` proves each by its own oracle, as in session 1.

**Tech Stack:** Rust (`cargo test`), `patch`, Python 3 (`tools/guard-census.py`, committed in session 1).

**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (revision 3 + part 3, with the session-1 amendments at `b20f83aa`). Sections 4, 5 and 8 govern this plan. Part 3 (section 9) gets its own plan.

## Global Constraints

- **R8:** nothing built here is production-active. The production-route edits are: a converted scanout bo hands its own device alias to the registration that counts it, and `submit_shared_scanout_frame` propagates its unwind error instead of discarding it. Both are inert in production, where no bo is managed and no gate is installed.
- **F3:** do not mock `ResourceService`; tests drive the real service.
- **F8:** if a step's expected result does not happen, stop and report the exact step, command and output. Never adapt a diff or a test to make it pass.
- **R12:** hardware tests use a `_vulkan`/`_drm` suffix and `#[ignore]`, and panic on a missing device. None of this session's new tests need hardware, but Task 1 changes code the hardware tests exercise, so Task 6's hardware run is not optional.
- Every new test name starts with `c0_2ci_`.
- Every guard assertion's message contains `[census:<MARKER>]`, and the test carries the matching `/// census:` tag (format: session 1 plan, Task 1).
- Gate before each hand-off: `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`; `cargo test -p yserver --lib c0_2ci`.
- The implementer does not run `git commit`, `git add`, `git checkout`, `git stash`, `git apply` or `rm -f`. The coordinating session commits every task after verifying it, using the message in the task, whose trailer records provenance: `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)`. Never a session URL in a commit message.
- Nobody pushes, squashes, rebases or amends.

## Corrections from the reviews

### Round 2 (2 blocking, 1 major)

| Finding | Disposition |
| --- | --- |
| **B-1** — a close arriving through the handle was not terminal: five transitions read the raw `self.state`, so after a service-driven close a gate still quiesced, issued a permit, published Owner and minted grants | **Fixed.** `begin_quiescing`, `authorize_owner_write`, `consume_owner_write`, `issue_handover_permit` and `publish_owner` now consult the effective `state()`, which `state()`'s own doc explains. Proven in two halves, because reaching Owner needs Task 4's evidence: `c0_2ci_service_driven_close_stops_the_gate_quiescing` (Task 2) and `c0_2ci_service_driven_close_is_terminal_for_owner_transitions` (Task 5). The three census tags whose condition text changed were updated with it. |
| **B-2** — flattening the cause into `PresentError::Io` cost a `ERROR_DEVICE_LOST` its identity, so the caller stopped latching `renderer_failed` | **Fixed.** A new `PresentError::ManagedUnwind { cause: Box<PresentError>, unwind: String }` keeps the cause structural, and `present_error_is_device_lost` recurses through it. Proven by `c0_2ci_failed_unwind_keeps_a_device_loss_recognisable`, which fails when the recursion is removed. The consuming handler itself (the scene tick's `renderer_failed` latch) is hardware-only; the test covers the classifier it calls. |
| **M-1** — carried forward: a plan cannot satisfy its authoritative criterion by amending it | **Taken, in the honest direction.** The spec amendment no longer rewrites 4.2's acceptance criterion: it records the real-path half as **not met**, open like 4.3's F8 stop, and Task 6 carries both into the acceptance record. The mechanism change still lands; what is not claimed is the evidence. |

### Round 1 (3 blocking, 2 major)

| Finding | Disposition |
| --- | --- |
| **B-1** — the husk registration was discharged while the bo's `Rc<drm::Device>` alias stayed alive | **Fixed.** The alias itself now moves into the registration: `ScanoutBo::drm` becomes `Option`, `register_pool_husk` takes the alias by value, and `unregister_pool_husk` drops it as it uncounts it. The count cannot reach zero while the husk's alias lives. Proven by `c0_2ci_husk_registration_owns_the_alias_it_counts`, which watches `Rc::strong_count`. |
| **B-2** — `ResourceService` could not identify the transport it closed | **Fixed.** `TransportGateHandle` carries its gate's device, incarnation and instance identity; `set_transport_gate` returns `Result` and refuses a handle for another transport (`WrongIncarnation`) or a second, different gate (`InvalidState`), while re-installing the same gate is idempotent. Two new census guards, both proven by oracle. |
| **B-3** — the reset correction modelled the generation replacement instead of driving it | **Recorded as the F8 stop the spec asks for** (user's decision). The test's module doc states what it does not prove and why, and it is renamed to claim only what it drives: the forced teardown, and the numeric reuse the replacement would produce. The crossing itself stays open for whoever owns the boundary next. |
| **M-1** — 4.2's real-path criterion replaced by inspection | **Accepted and written down** (user's decision). Both failure arms of the real path return `managed_submit_failure`, so no result can be discarded there; that a call site still calls it is checked by reading, at review, and the spec amendment says the criterion is met that far and no further. |
| **M-2** — handover evidence still self-asserted | **Not taken** (user's decision). Spec 4.4 asks for explicit evidence for every `WriterClass`, which the plan supplies, with an exhaustive match that fails to compile when a class is added. Minting witnesses from fixtures that install or disable each writer, and reservations from a live `RecipientSlot`, is production-issuer work the spec defers to stages 3/4 (section 6). |

## How to apply a block

Every diff and every Rust block below is exact, and is applied **mechanically, never retyped**. Write the extractor to a file once, then use it for every block: it builds its fence with `chr(96)` and contains no backtick itself, so no shell quoting -- including `bash -lc "..."`, which cost an earlier run its pattern to command substitution -- can mangle it. Extract a task's `n`th fenced block of a language with:

```bash
cat > /tmp/s2-extract.py <<'XPY'
import re, sys
plan, task, lang, n = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
fence = chr(96) * 3          # built, never written: no backtick in this file,
text = open(plan).read()     # so no shell quoting can eat the pattern
start = text.index("### " + task + ":")
end = text.find("\n### Task ", start + 1)
body = text[start:] if end == -1 else text[start:end]
blocks = re.findall("^" + fence + lang + "\n(.*?)^" + fence + "$", body, re.S | re.M)
if len(blocks) < n:
    sys.exit("no " + lang + " block " + str(n) + " in " + task)
sys.stdout.write(blocks[n - 1])
XPY
python3 /tmp/s2-extract.py docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md "Task N" diff 1 > /tmp/s2-taskN.patch
```

Then apply a diff with `patch -p1 --no-backup-if-mismatch < /tmp/s2-taskN.patch`, and append a Rust block with `cat /tmp/s2-taskN.rs >> <file>`. `patch` must report no rejected hunk and no fuzz; an offset is fine. Anything else is an F8 stop.

## Execution split

- **The implementer** applies each task, runs the non-hardware gate, and proves each task's tagged guards with `tools/guard-census.py --deterministic-only --require-oracle`.
- **The coordinating session** runs the [H] steps of Task 6: the full acceptance census (hardware included) and the hardware gate. It also re-runs, per task, the named mutations that are not census sites (they are listed in each task's last step under *Reviewer mutations*).

## Plan corrections to the spec (applied in Task 1)

Found while prototyping; Task 1 records them in the spec so plan and spec agree:

1. **4.2 — where the close comes from, and what the test drives.** `submit_shared_scanout_frame` receives no transport gate, only the `ResourceService`. The gate handle is therefore installed on the service, the way `CommitResourceConsumer` already holds one, and the handle names its gate so the service can refuse a foreign one. The real path cannot be made to fail its unwind without Vulkan, DRM and fault injection, so the unwind moves into `scene::managed_submit_failure`, which both failure arms of the real path now return as their `Err` value — there is no result left for the call site to discard. The named tests drive that function. What they cannot see is a call site that stops calling it; that residue is checked by reading the two `return Err(managed_submit_failure(...))` arms at review.
2. **4.2 — when the transport closes.** Following the 2c-i design's section 4 ("Unknown submission retains its reservation and closes the affected transport"): always, when the submission may have reached the GPU, even if the freeze succeeds; and, when it provably did not, only if cancelling its obligations fails.
3. **4.3 — the generation-replacement half is an F8 stop.** `force_destroy_all_clients` is `pub`, but `reset_generation` is `pub(crate)` in `yserver-core` and needs a live poller, setup registry and input inventory. Spec 4.3 says an undrivable half is an F8 stop to report, not a reason to add wiring, so that is what this is: the test drives the forced teardown through the real entry point and then reuses the numeric XIDs over the same backend, proving that proofs are keyed by allocation and not by XID. It does not prove the reset's invariant 6, and its module doc says so.
4. **4.4 — the outstanding-grant guards.** `begin_quiescing` refuses while a grant is outstanding, so no public transition reaches `Quiescing` with one. The tests for the two outstanding-grant guards build that state with the existing `set_outstanding_owner_writes_for_tests`.
5. **4.2 — the real-path criterion stays open.** Round-2 M-1: the plan does not amend 4.2's acceptance criterion to accept inspection. The mechanism lands and is proven at `managed_submit_failure`; the criterion's real-path half is recorded as unmet, beside 4.3's F8 stop, and Task 6 says so in the acceptance record.
6. **Acceptance census.** The full enumeration grows to **71** sites: session 1's 68, `issue_handover_permit`'s new reservation guard, and the two new `set_transport_gate` guards. Expected: `CAUGHT_BY_ORACLE` 39, `CAUGHT` 32, `SURVIVES` **0**. The three new guards in `drm_cleanup.rs` sit outside the census's default files and are proven with `--files drm_cleanup.rs`.

## File Structure

- Modify: `crates/yserver/src/kms/render/resources/drm_cleanup.rs` — `PoolHuskRegistration`, which owns the husk's device alias; the registry's accounting flag; the token-consuming unregister (Task 1).
- Modify: `crates/yserver/src/kms/vk/scanout.rs` — `ScanoutBo::drm` becomes `Option`; the bo holds its registration; `detach_managed_entries` consumes it (Task 1).
- Modify: `crates/yserver/src/kms/render/platform.rs` — `register_managed_scanout_bo` moves the alias into the registration (Task 1); one test's handover evidence (Task 4).
- Modify: `crates/yserver/src/kms/render/resources/mod.rs` — re-export (Task 1); the service's transport gate and its identity guards (Task 2); `mod reset_boundary_tests` (Task 3).
- Modify: `crates/yserver/src/kms/render/resources/gpu.rs` — `abandon_unsubmitted_batch` (Task 2).
- Modify: `crates/yserver/src/kms/render/scene.rs` — `managed_submit_failure` and its two call sites (Task 2).
- Create: `crates/yserver/src/kms/render/resources/reset_boundary_tests.rs` (Task 3).
- Modify: `crates/yserver/src/kms/render/resources/transport.rs` — the identified gate handle (Task 2); identity-bound `RecipientReservation`, test coverage evidence, the reservation guard (Task 4).
- Modify: `crates/yserver/src/kms/render/resources/handoff.rs`, `resources/tests.rs` — callers of the reshaped evidence (Task 4).
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` — appended sections (Tasks 1, 2, 4, 5).
- Modify: `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (Task 1, corrections; Task 6, status).
- Create ([H], coordinator): `docs/superpowers/findings/2026-09-17-stage-2c-i-debt-census-session-2.md` (Task 6).

---

### Task 1: Husk accounting bound to identity (spec 4.1)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/drm_cleanup.rs`, `crates/yserver/src/kms/render/resources/mod.rs`, `crates/yserver/src/kms/vk/scanout.rs`, `crates/yserver/src/kms/render/platform.rs`
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)
- Modify: `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`

**Interfaces:**
- Produces: `DrmCleanupRegistry::register_pool_husk(&mut self, alias: Rc<crate::drm::Device>) -> PoolHuskRegistration`; `DrmCleanupRegistry::unregister_pool_husk(&mut self, PoolHuskRegistration) -> Result<(), ResourceError>` (`WrongIncarnation` for another device or incarnation, `InvalidProof` for another registry's registration; both set the accounting flag); `try_mint_file_family_closed` refuses with `"pool husk accounting failed"` once the flag is set, checked before the alias count.
- Produces: `ScanoutBo::{take_husk_alias, set_husk_registration, take_husk_registration}`; `ScanoutBo::take_physical_backing` now returns `Option<ScanoutBoBacking>` (`None` once the alias has moved); `detach_managed_entries`' signature is unchanged.
- Invariants: the registration owns the alias it counts, so consuming it ends both together, and no registry can certify zero aliases while a husk's alias lives; any registration other than one this registry minted for this device and incarnation is refused and closes both registries' barriers; a registration dropped undischarged closes its registry's barrier.

- [x] **Step 1: Amend the spec**

In `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`, insert this subsection immediately before the heading `## 5. Evidence and review`:

```markdown
### 4.5. Plan corrections (session 2, 2026-09-17)

From prototyping session 2 and from codex round 1 on its plan
(`…-session-2-plan-review-round1.md`). The plan
(`docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md`)
carries the detail.

1. **4.1.** The husk registration **owns** the alias it accounts for: the
   scanout bo's own `Rc<drm::Device>` moves into it at conversion, and
   consuming the registration drops it. Round 1's B-1 showed that keeping the
   alias on the bo let the inventory reach zero while the alias was still
   alive.
2. **4.2.** The transport gate handle is installed on the `ResourceService`,
   the only thing `submit_shared_scanout_frame` receives, and the handle
   names its gate's device, incarnation and instance so the service refuses a
   foreign gate or a silent replacement (round-1 B-2). A close arriving
   through that handle is terminal: every transition consults the effective
   state, never the raw field (round-2 B-1). The unwind moves into
   `scene::managed_submit_failure`, which both failure arms of the real path
   return as their `Err` value, and which keeps its cause structurally in
   `PresentError::ManagedUnwind`, so a device loss survives a failed unwind
   and still latches the fatal renderer state (round-2 B-2).
3. **4.2 — acceptance, unmet half.** The named tests drive
   `managed_submit_failure`, not the real `scene.rs` path: failing that path's
   unwind needs Vulkan, DRM and fault injection. **4.2's requirement of a
   named test driving the real path is therefore NOT met**, and this
   amendment does not weaken it — it records it as open, next to 4.3's F8
   stop. That a call site still calls the helper is checked by reading, at
   review (round-1 M-1, carried forward as round-2 M-1).
4. **4.2.** The transport closes whenever the submission may have reached
   the GPU, even when the freeze succeeds (2c-i design section 4), and, when
   it provably did not, only if cancelling its obligations fails.
5. **4.3 — F8 stop.** `reset_generation` is `pub(crate)` in `yserver-core`
   and needs a live poller, setup registry and input inventory, so the
   generation-replacement half of 4.3 is **not driven**, and this is the F8
   stop 4.3 itself calls for rather than a reason to add wiring (round-1
   B-3). The test drives the forced teardown through
   `force_destroy_all_clients` and then reuses the numeric XIDs over the same
   backend: it proves proofs are keyed by allocation and not by XID, and it
   does not prove the reset's invariant 6. The crossing stays open for
   whoever owns the boundary next.
6. **4.4.** No public transition reaches `Quiescing` with a grant
   outstanding; the two outstanding-grant tests build that state with
   `set_outstanding_owner_writes_for_tests`. Coverage evidence stays
   test-side and per-class as 4.4 specifies: minting it from fixtures that
   install or disable each writer, and reservations from a live
   `RecipientSlot`, is production-issuer work section 6 defers to stages 3/4
   (round-1 M-2, not taken).
7. **5.1.** The full enumeration is 71 sites (the reservation guard and the
   two `set_transport_gate` guards are new). Session 2 is accepted with
   `CAUGHT_BY_ORACLE` 39, `CAUGHT` 32 and zero survivors, and the three new
   `drm_cleanup.rs` guards proven by oracle with `--files drm_cleanup.rs` --
   with 4.2's real-path half and 4.3's crossing recorded as open.
```

- [x] **Step 2: Apply the production diff**

Extract this task's `diff` block 1 to `/tmp/s2-task1.patch` and apply it (see *How to apply a block*).

```diff
diff --git a/crates/yserver/src/kms/render/platform.rs b/crates/yserver/src/kms/render/platform.rs
--- a/crates/yserver/src/kms/render/platform.rs
+++ b/crates/yserver/src/kms/render/platform.rs
@@ -5785,12 +5785,17 @@ impl PlatformBackend {
             None => None,
         };
 
+        // Spec 4.1 (stage 2c-i debt): `None` means this bo's device alias
+        // already moved into a registration -- it was converted once
+        // already, and converting it again would leave that registration
+        // accounting for nothing.
         let display_backing = scanout
             .display_pool_mut()
             .bos
             .get_mut(bo_idx)
             .ok_or(ResourceError::InvalidState)?
-            .take_physical_backing();
+            .take_physical_backing()
+            .ok_or(ResourceError::InvalidState)?;
 
         // fb_handle/gem_handle presence was validated above, so the only
         // error `from_scanout_bo_backing` can return cannot occur here.
@@ -5872,19 +5877,25 @@ impl PlatformBackend {
             .get_mut(output_idx)
             .and_then(Option::as_mut)
             .ok_or(ResourceError::InvalidState)?;
-        scanout
+        let display_bo = scanout
             .display_pool_mut()
             .bos
             .get_mut(bo_idx)
-            .ok_or(ResourceError::InvalidState)?
-            .set_managed(display_lease);
+            .ok_or(ResourceError::InvalidState)?;
+        display_bo.set_managed(display_lease);
         // F8-M1: the display husk keeps its own `Rc<drm::Device>` clone
         // (`take_physical_backing` above only moved the file-owned handles
         // and shared Vulkan backing out, not `self.drm`); count that alias
         // now that the conversion has committed, so the fd-family barrier's
         // inventory sees it. `detach_managed_entries` is the real
-        // unregister site (F2-m1).
-        registry.register_pool_husk();
+        // unregister site (F2-m1). Spec 4.1 (stage 2c-i debt): the alias
+        // itself moves into the registration, so consuming the registration
+        // is what drops it -- the count cannot reach zero while the husk's
+        // alias lives. The bo holds the registration until detach.
+        let alias = display_bo
+            .take_husk_alias()
+            .ok_or(ResourceError::InvalidState)?;
+        display_bo.set_husk_registration(registry.register_pool_husk(alias));
         if let Some(renderer_lease) = renderer_lease {
             scanout
                 .copied_mut()
diff --git a/crates/yserver/src/kms/render/resources/drm_cleanup.rs b/crates/yserver/src/kms/render/resources/drm_cleanup.rs
--- a/crates/yserver/src/kms/render/resources/drm_cleanup.rs
+++ b/crates/yserver/src/kms/render/resources/drm_cleanup.rs
@@ -1,4 +1,4 @@
-use std::{collections::BTreeSet, io, num::NonZeroU32, rc::Rc};
+use std::{cell::Cell, collections::BTreeSet, io, num::NonZeroU32, rc::Rc};
 
 use drm::{
     buffer::Handle as DrmBufferHandle,
@@ -81,6 +81,42 @@ pub(crate) struct FileFamilyClosed {
     _private: (),
 }
 
+/// Spec 4.1 (stage 2c-i debt): one pool husk's `Rc<drm::Device>` alias,
+/// held by the registry entry that counts it. Minted only by
+/// `DrmCleanupRegistry::register_pool_husk`, which takes the alias by value;
+/// consumed by value by `unregister_pool_husk`, which validates it against
+/// the registry's own identity and drops the alias as it uncounts it -- so
+/// the inventory can never reach zero while the alias is still alive
+/// (round-1 B-1). Dropping the registration undischarged fails closed: its
+/// registry can never mint `FileFamilyClosed` again (the lost-role-token
+/// rule).
+pub(crate) struct PoolHuskRegistration {
+    device_key: DrmDeviceKey,
+    incarnation: IncarnationId,
+    accounting: Rc<Cell<bool>>,
+    alias: Option<Rc<crate::drm::Device>>,
+    discharged: bool,
+}
+
+impl std::fmt::Debug for PoolHuskRegistration {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        f.debug_struct("PoolHuskRegistration")
+            .field("device_key", &self.device_key)
+            .field("incarnation", &self.incarnation)
+            .field("alias_held", &self.alias.is_some())
+            .field("discharged", &self.discharged)
+            .finish()
+    }
+}
+
+impl Drop for PoolHuskRegistration {
+    fn drop(&mut self) {
+        if !self.discharged {
+            self.accounting.set(true);
+        }
+    }
+}
+
 #[allow(dead_code)]
 pub(crate) trait CleanupIo {
     fn remove_fb(&mut self, fb: u32) -> io::Result<()>;
@@ -155,6 +191,12 @@ pub(crate) struct DrmCleanupRegistry {
     payload_alias_keys: BTreeSet<AllocationKey>,
     family_inventory: FamilyInventory,
     returned_descriptors: Vec<std::os::fd::OwnedFd>,
+    /// Spec 4.1: set once pool-husk accounting can no longer be trusted --
+    /// a registration dropped undischarged, or a foreign or unknown one
+    /// presented. Shared with every `PoolHuskRegistration` this registry
+    /// mints, whose pointer identity is also what proves a registration was
+    /// minted here. Never cleared.
+    husk_accounting_failed: Rc<Cell<bool>>,
 }
 
 impl std::fmt::Debug for DrmCleanupRegistry {
@@ -188,6 +230,7 @@ impl DrmCleanupRegistry {
             payload_alias_keys: BTreeSet::new(),
             family_inventory: FamilyInventory::default(),
             returned_descriptors: Vec::new(),
+            husk_accounting_failed: Rc::new(Cell::new(false)),
         }
     }
 
@@ -206,6 +249,7 @@ impl DrmCleanupRegistry {
             payload_alias_keys: BTreeSet::new(),
             family_inventory: FamilyInventory::default(),
             returned_descriptors: Vec::new(),
+            husk_accounting_failed: Rc::new(Cell::new(false)),
         }
     }
 
@@ -225,6 +269,7 @@ impl DrmCleanupRegistry {
             payload_alias_keys: BTreeSet::new(),
             family_inventory: FamilyInventory::default(),
             returned_descriptors: Vec::new(),
+            husk_accounting_failed: Rc::new(Cell::new(false)),
         }
     }
 
@@ -373,13 +418,52 @@ impl DrmCleanupRegistry {
             .saturating_sub(count);
     }
 
-    pub(crate) fn register_pool_husk(&mut self) {
+    /// Takes the husk's device alias by value (round-1 B-1): the
+    /// registration owns it from here, and only consuming the registration
+    /// releases it.
+    pub(crate) fn register_pool_husk(
+        &mut self,
+        alias: Rc<crate::drm::Device>,
+    ) -> PoolHuskRegistration {
         self.family_inventory.non_payload_aliases += 1;
+        PoolHuskRegistration {
+            device_key: self.device_key,
+            incarnation: self.incarnation,
+            accounting: Rc::clone(&self.husk_accounting_failed),
+            alias: Some(alias),
+            discharged: false,
+        }
     }
 
-    pub(crate) fn unregister_pool_husk(&mut self) {
-        self.family_inventory.non_payload_aliases =
-            self.family_inventory.non_payload_aliases.saturating_sub(1);
+    /// Spec 4.1: uncounts the husk `registration` proves this registry
+    /// counted. A registration for another device or incarnation, or one
+    /// another registry minted, is refused and fails closed on both sides:
+    /// this registry refuses to mint from now on, and so does the one that
+    /// minted it, when the refused registration drops undischarged.
+    pub(crate) fn unregister_pool_husk(
+        &mut self,
+        mut registration: PoolHuskRegistration,
+    ) -> Result<(), ResourceError> {
+        if registration.device_key != self.device_key
+            || registration.incarnation != self.incarnation
+        {
+            self.husk_accounting_failed.set(true);
+            return Err(ResourceError::WrongIncarnation);
+        }
+        if !Rc::ptr_eq(&registration.accounting, &self.husk_accounting_failed) {
+            self.husk_accounting_failed.set(true);
+            return Err(ResourceError::InvalidProof);
+        }
+        let Some(remaining) = self.family_inventory.non_payload_aliases.checked_sub(1) else {
+            self.husk_accounting_failed.set(true);
+            return Err(ResourceError::InvalidState);
+        };
+        self.family_inventory.non_payload_aliases = remaining;
+        registration.discharged = true;
+        // The alias this registration accounted for ends here, with the
+        // count that named it (round-1 B-1).
+        drop(registration.alias.take());
+        Ok(())
     }
 
     /// Becomes mintable when every submitter is detached, the helper is
@@ -418,6 +502,12 @@ impl DrmCleanupRegistry {
         if !self.family_inventory.control_closed {
             return Err(io::Error::other("control fd is not closed"));
         }
+        // Spec 4.1: checked before the alias count, so a refusal says which
+        // of the two it is -- a dropped registration also leaves its alias
+        // counted.
+        if self.husk_accounting_failed.get() {
+            return Err(io::Error::other("pool husk accounting failed"));
+        }
         if self.family_inventory.non_payload_aliases > 0 {
             return Err(io::Error::other("non-payload aliases still active"));
         }
diff --git a/crates/yserver/src/kms/render/resources/mod.rs b/crates/yserver/src/kms/render/resources/mod.rs
--- a/crates/yserver/src/kms/render/resources/mod.rs
+++ b/crates/yserver/src/kms/render/resources/mod.rs
@@ -43,7 +45,7 @@ pub(crate) use completion::{ResourceConsumer, ResourceWaiter, WaiterRegistry};
 #[allow(unused_imports)]
 pub(crate) use drm_cleanup::{
     CleanupIo, DeviceCleanupIo, DirectFramebufferAllocation, DrmCleanupRegistry, DrmCleanupRight,
-    FamilyInventory, FileFamilyClosed, GemOwner, RightState,
+    FamilyInventory, FileFamilyClosed, GemOwner, PoolHuskRegistration, RightState,
 };
 #[allow(unused_imports)]
 use gpu::ValidatedGpuBatch;
diff --git a/crates/yserver/src/kms/vk/scanout.rs b/crates/yserver/src/kms/vk/scanout.rs
--- a/crates/yserver/src/kms/vk/scanout.rs
+++ b/crates/yserver/src/kms/vk/scanout.rs
@@ -524,9 +524,15 @@ pub struct ScanoutBo {
     /// sized for the bo (XRGB8888 → 4 bytes × width × height), and
     /// the device memory backing it.
     pub vk_transfer: TransferResources,
-    /// Shared DRM device handle (for un-registering the framebuffer
-    /// + closing the GEM handle in Drop).
-    drm: Rc<crate::drm::Device>,
+    /// Shared DRM device handle, for un-registering the framebuffer and
+    /// closing the GEM handle in Drop.
+    ///
+    /// Spec 4.1 (stage 2c-i debt): `None` once managed conversion moved this
+    /// alias into the `PoolHuskRegistration` that accounts for it, which is
+    /// what makes the registry's alias count true -- consuming the
+    /// registration drops this very `Rc`. A husk has no framebuffer or GEM
+    /// handle left, so nothing below needs the device again.
+    drm: Option<Rc<crate::drm::Device>>,
     /// Held to keep image+memory destructors anchored to a live
     /// device. Cloned per bo from the pool's Arc so individual bos
     /// can be moved/dropped independently.
@@ -562,6 +568,11 @@ pub struct ScanoutBo {
     /// the pool slot leaves the entry with zero live uses, dirty, and
     /// destroyable on the very next `service_ready` tick.
     managed: Option<crate::kms::render::resources::AllocationLease>,
+    /// Spec 4.1 (stage 2c-i debt): the registration proving this bo's husk
+    /// alias was counted, held beside the lease it accompanies and consumed
+    /// by `ScanoutPool::detach_managed_entries`. Dropped undischarged (the
+    /// bo dropped, or detached with no registry) it fails closed.
+    husk_registration: Option<crate::kms::render::resources::PoolHuskRegistration>,
 }
 
 /// Per-bo transfer-side resources (command pool/buffer + staging
@@ -638,6 +649,27 @@ pub struct ScanoutBoPool {
     gbm_device: Option<Rc<GbmDevice>>,
 }
 
+/// Spec 4.1: consumes `bo`'s husk registration through `registry`, or drops
+/// it undischarged -- failing closed -- when there is none. A refused
+/// registration has already failed closed inside the registry; the log is
+/// the only other channel left (F6).
+fn discharge_husk_registration(
+    bo: &mut ScanoutBo,
+    registry: Option<&mut crate::kms::render::resources::DrmCleanupRegistry>,
+) {
+    let Some(registration) = bo.take_husk_registration() else {
+        return;
+    };
+    match registry {
+        Some(registry) => {
+            if let Err(err) = registry.unregister_pool_husk(registration) {
+                log::error!("scanout: pool husk registration refused on detach: {err:?}");
+            }
+        }
+        None => drop(registration),
+    }
+}
+
 #[cfg(test)]
 impl ScanoutBoPool {
     pub(crate) fn for_tests() -> Self {
@@ -766,17 +798,18 @@ impl OutputScanout {
 
     /// F8-M1: `registry` accounts for the husk's `Rc<drm::Device>` clone
     /// left behind by `take_physical_backing` (F2-m1) -- every `ScanoutBo`
-    /// whose managed lease this call actually drops (`take_managed()`
-    /// returned `Some`) had that clone registered once at conversion time
-    /// (`PlatformBackend::register_managed_scanout_bo`), and this is the
-    /// real, non-test site that unregisters it: `take_managed()` alone
-    /// releases the pool's retain reservation and makes the entry
-    /// destroyable, but the husk's own `self.drm` field is untouched by it
-    /// and outlives the managed key -- the fd-family barrier's inventory
-    /// must stop counting it here or it can never mint (R5). `None` is the
-    /// production shape (`reset_scanout_bos_for_suspend` calls this with no
-    /// registry in scope, and no production bo is ever managed to begin
-    /// with -- R8), and a no-op there is correct.
+    /// converted by `PlatformBackend::register_managed_scanout_bo` holds the
+    /// `PoolHuskRegistration` that counted that clone, and this is the real,
+    /// non-test site that consumes it: `take_managed()` alone releases the
+    /// pool's retain reservation and makes the entry destroyable, but the
+    /// husk's own `self.drm` field is untouched by it and outlives the
+    /// managed key -- the fd-family barrier's inventory must stop counting
+    /// it here or it can never mint (R5). Spec 4.1 (stage 2c-i debt): with
+    /// `None`, a bo that holds a registration drops it undischarged, which
+    /// closes the barrier for good instead of silently skipping the
+    /// accounting. `None` remains the production shape
+    /// (`drain_scanout_pool_at` has no registry in scope), and there no bo
+    /// is ever managed (R8), so no registration exists to drop.
     pub(crate) fn detach_managed_entries(
         &mut self,
         mut registry: Option<&mut crate::kms::render::resources::DrmCleanupRegistry>,
@@ -787,20 +820,14 @@ impl OutputScanout {
         match self {
             Self::Shared(pool) => {
                 for bo in &mut pool.bos {
-                    if bo.take_managed().is_some()
-                        && let Some(registry) = registry.as_deref_mut()
-                    {
-                        registry.unregister_pool_husk();
-                    }
+                    bo.take_managed();
+                    discharge_husk_registration(bo, registry.as_deref_mut());
                 }
             }
             Self::Copied(pool) => {
                 for bo in &mut pool.destinations.bos {
-                    if bo.take_managed().is_some()
-                        && let Some(registry) = registry.as_deref_mut()
-                    {
-                        registry.unregister_pool_husk();
-                    }
+                    bo.take_managed();
+                    discharge_husk_registration(bo, registry.as_deref_mut());
                 }
                 for src in &mut pool.sources {
                     // F8-M1 (resolved open question): no `unregister_pool_husk()`
@@ -3232,18 +3259,21 @@ impl ScanoutBo {
     /// pool-slot state (phase, width/height, `managed_key`) is untouched —
     /// building a `ScanoutAllocation` over the same handles while this bo
     /// still owns them is exactly the two-closers shape R3 forbids.
-    pub(crate) fn take_physical_backing(&mut self) -> ScanoutBoBacking {
-        ScanoutBoBacking {
+    /// `None` once this bo's device alias has moved into a
+    /// `PoolHuskRegistration` (spec 4.1): a bo cannot be converted twice.
+    pub(crate) fn take_physical_backing(&mut self) -> Option<ScanoutBoBacking> {
+        let drm = Rc::clone(self.drm.as_ref()?);
+        Some(ScanoutBoBacking {
             fb_handle: self.fb_handle.take(),
             gem_handle: self.gem_handle.take(),
             gbm_bo: self.gbm_bo.take(),
-            drm: Rc::clone(&self.drm),
+            drm,
             image: std::mem::replace(&mut self.vk_image, vk::Image::null()),
             memory: std::mem::replace(&mut self.vk_memory, vk::DeviceMemory::null()),
             view: std::mem::replace(&mut self.vk_image_view, vk::ImageView::null()),
             transfer: std::mem::replace(&mut self.vk_transfer, TransferResources::empty()),
             vk: Arc::clone(&self.vk),
-        }
+        })
     }
 }
 
@@ -3271,11 +3301,12 @@ impl ScanoutBo {
             fb_handle: None,
             gem_handle: None,
             vk_transfer: TransferResources::empty(),
-            drm,
+            drm: Some(drm),
             vk,
             disarmed: false,
             gbm_bo: None,
             managed: None,
+            husk_registration: None,
         }
     }
 }
@@ -3564,11 +3595,12 @@ impl ScanoutBo {
             fb_handle: framebuffer,
             gem_handle: gem,
             vk_transfer: transfer.expect("completed allocation has transfer resources"),
-            drm,
+            drm: Some(drm),
             vk,
             disarmed: false,
             gbm_bo,
             managed: None,
+            husk_registration: None,
         })
     }
 
@@ -3586,6 +3618,27 @@ impl ScanoutBo {
         self.managed = Some(lease);
     }
 
+    /// Spec 4.1: takes this bo's own device alias, so it can be moved into
+    /// the registration that accounts for it. `None` once taken.
+    pub(crate) fn take_husk_alias(&mut self) -> Option<Rc<crate::drm::Device>> {
+        self.drm.take()
+    }
+
+    /// Spec 4.1: keeps the registration for this bo's husk alias until
+    /// `detach_managed_entries` consumes it.
+    pub(crate) fn set_husk_registration(
+        &mut self,
+        registration: crate::kms::render::resources::PoolHuskRegistration,
+    ) {
+        self.husk_registration = Some(registration);
+    }
+
+    pub(crate) fn take_husk_registration(
+        &mut self,
+    ) -> Option<crate::kms::render::resources::PoolHuskRegistration> {
+        self.husk_registration.take()
+    }
+
     /// Ends this slot's managed reservation (F2-B1): `detach_managed_entries`
     /// and pool drain/replacement call this, which is what makes the entry
     /// destroyable on the next tick -- not a mere key clear.
@@ -3781,11 +3834,16 @@ impl ScanoutBo {
     /// succeeds, so a caller can retain the complete object graph when cleanup
     /// fails instead of letting ordinary Drop free still-referenced backing.
     fn release_disposable_drm_resources(&mut self) -> io::Result<()> {
+        // A converted husk has neither handle left and no device alias
+        // (spec 4.1), so there is nothing to release.
+        let Some(drm) = self.drm.as_ref() else {
+            return Ok(());
+        };
         release_drm_handles_strict(
             &mut self.fb_handle,
             &mut self.gem_handle,
             |framebuffer| {
-                self.drm.destroy_framebuffer(framebuffer).map_err(|error| {
+                drm.destroy_framebuffer(framebuffer).map_err(|error| {
                     scanout_io_context(
                         format!("destroy disposable framebuffer {framebuffer:?}"),
                         error,
@@ -3793,7 +3851,7 @@ impl ScanoutBo {
                 })
             },
             |gem| {
-                self.drm.close_buffer(gem).map_err(|error| {
+                drm.close_buffer(gem).map_err(|error| {
                     scanout_io_context(format!("close disposable GEM handle {gem:?}"), error)
                 })
             },
@@ -3850,15 +3908,17 @@ impl Drop for ScanoutBo {
         // DRM-side teardown next: framebuffer references the GEM
         // handle; both must be released before we free the underlying
         // memory the dma-buf was exported from.
-        if let Some(fb) = self.fb_handle.take()
-            && let Err(e) = self.drm.destroy_framebuffer(fb)
-        {
-            log::warn!("drm destroy_framebuffer failed: {e}");
-        }
-        if let Some(h) = self.gem_handle.take()
-            && let Err(e) = self.drm.close_buffer(h)
-        {
-            log::warn!("drm close_buffer (gem) failed: {e}");
+        if let Some(drm) = self.drm.as_ref() {
+            if let Some(fb) = self.fb_handle.take()
+                && let Err(e) = drm.destroy_framebuffer(fb)
+            {
+                log::warn!("drm destroy_framebuffer failed: {e}");
+            }
+            if let Some(h) = self.gem_handle.take()
+                && let Err(e) = drm.close_buffer(h)
+            {
+                log::warn!("drm close_buffer (gem) failed: {e}");
+            }
         }
 
         unsafe {
```

Expected: `patch` reports four files patched, no rejects, no fuzz.

- [x] **Step 3: Append the tests**

Extract this task's `rust` block 1 to `/tmp/s2-task1.rs` and append it to `crates/yserver/src/kms/render/resources/guard_tests.rs`.

```rust

// ---------------------------------------------------------------------------
// Session 2, spec 4.1: pool-husk accounting bound to identity.
// ---------------------------------------------------------------------------

/// A registry on `incarnation` whose every other `FileFamilyClosed`
/// precondition already holds, so a mint refusal can only come from husk
/// accounting.
fn husk_registry(incarnation: IncarnationId) -> DrmCleanupRegistry {
    let mut registry = DrmCleanupRegistry::new_with_io(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        incarnation,
        Box::new(super::tests::MockCleanupIo::new(Rc::new(
            std::cell::RefCell::new(Vec::new()),
        ))),
    );
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();
    registry
}

/// A stub device alias for a husk registration: `Device::for_tests` opens
/// no DRM node, and what the test needs is only the `Rc` whose lifetime the
/// registration now owns.
fn husk_alias() -> Rc<crate::drm::Device> {
    Rc::new(crate::drm::Device::for_tests().expect("stub drm device"))
}

fn mint_refusal(registry: &mut DrmCleanupRegistry) -> Option<String> {
    registry
        .try_mint_file_family_closed(|_, _| Ok(()))
        .err()
        .map(|err| err.to_string())
}

/// Round-1 B-1: the alias and the count that names it end together, so the
/// inventory cannot reach zero while the husk's `Rc<drm::Device>` is alive.
#[test]
fn c0_2ci_husk_registration_owns_the_alias_it_counts() {
    let mut registry = husk_registry(IncarnationId::first());
    let alias = husk_alias();
    let watch = Rc::clone(&alias);
    let registration = registry.register_pool_husk(alias);
    assert_eq!(
        Rc::strong_count(&watch),
        2,
        "the registration must own the husk's alias"
    );
    assert_eq!(
        mint_refusal(&mut registry).as_deref(),
        Some("non-payload aliases still active"),
        "the family cannot close while the husk alias is counted"
    );
    registry.unregister_pool_husk(registration).unwrap();
    assert_eq!(
        Rc::strong_count(&watch),
        1,
        "consuming the registration must drop the alias it counted"
    );
    assert_eq!(mint_refusal(&mut registry), None);
}

/// census: S2-husk-mint-poisoned drm_cleanup.rs try_mint_file_family_closed `self.husk_accounting_failed.get()`
#[test]
fn c0_2ci_guard_dropped_husk_registration_closes_the_family_barrier() {
    let mut registry = husk_registry(IncarnationId::first());
    drop(registry.register_pool_husk(husk_alias()));
    assert_eq!(
        mint_refusal(&mut registry).as_deref(),
        Some("pool husk accounting failed"),
        "a husk registration dropped undischarged must fail closed [census:S2-husk-mint-poisoned]"
    );
}

/// census: S2-husk-foreign drm_cleanup.rs unregister_pool_husk `registration.device_key != self.device_key || registration.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_foreign_husk_registration_is_refused_and_fails_closed() {
    let mut minted_by = husk_registry(IncarnationId::first());
    let mut presented_to = husk_registry(IncarnationId::first().next());
    let own = presented_to.register_pool_husk(husk_alias());
    let foreign = minted_by.register_pool_husk(husk_alias());
    assert_eq!(
        presented_to.unregister_pool_husk(foreign),
        Err(ResourceError::WrongIncarnation),
        "a registration from another incarnation must be refused as such [census:S2-husk-foreign]"
    );
    // The refused registration must not have consumed `own`'s count...
    presented_to.unregister_pool_husk(own).unwrap();
    // ...and both registries now refuse to certify the family closed.
    assert_eq!(
        mint_refusal(&mut presented_to).as_deref(),
        Some("pool husk accounting failed")
    );
    assert_eq!(
        mint_refusal(&mut minted_by).as_deref(),
        Some("pool husk accounting failed")
    );
}

/// census: S2-husk-unknown drm_cleanup.rs unregister_pool_husk `!Rc::ptr_eq(&registration.accounting, &self.husk_accounting_failed)`
#[test]
fn c0_2ci_guard_unknown_husk_registration_cannot_consume_another_husks_count() {
    // Same device and incarnation, different registry: identity alone
    // cannot tell them apart, so only the registry's own correlation can.
    let mut minted_by = husk_registry(IncarnationId::first());
    let mut presented_to = husk_registry(IncarnationId::first());
    let own = presented_to.register_pool_husk(husk_alias());
    let unknown = minted_by.register_pool_husk(husk_alias());
    assert_eq!(
        presented_to.unregister_pool_husk(unknown),
        Err(ResourceError::InvalidProof),
        "a registration another registry minted must be refused [census:S2-husk-unknown]"
    );
    presented_to.unregister_pool_husk(own).unwrap();
    assert_eq!(
        mint_refusal(&mut presented_to).as_deref(),
        Some("pool husk accounting failed")
    );
    assert_eq!(
        mint_refusal(&mut minted_by).as_deref(),
        Some("pool husk accounting failed")
    );
}
```

- [x] **Step 4: Run the tests**

Run: `cargo +nightly fmt && cargo test -p yserver --lib husk`
Expected: `4 passed`, plus the `_vulkan` husk test ignored. A failure is an F8 stop.

- [x] **Step 5: Run the oracle**

Run: `python3 tools/guard-census.py --files drm_cleanup.rs --deterministic-only --require-oracle`
Expected: `S2-husk-foreign`, `S2-husk-unknown` and `S2-husk-mint-poisoned` each `CAUGHT_BY_ORACLE`; summary `CAUGHT_BY_ORACLE: 3`; exit 0.

- [x] **Step 6: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; `c0_2ci` 163 passed, 18 ignored.

*Reviewer mutations* (coordinator, each must fail a named test): delete `self.accounting.set(true);` from `PoolHuskRegistration::drop` (fails `c0_2ci_guard_dropped_husk_registration_closes_the_family_barrier`); replace `drop(registration.alias.take());` in `unregister_pool_husk` with `std::mem::forget(registration.alias.take());` (fails `c0_2ci_husk_registration_owns_the_alias_it_counts` at the `Rc::strong_count` assertion). Also read `discharge_husk_registration`'s `None => drop(registration),` arm: no deterministic test reaches it, since `detach_managed_entries` needs a Vulkan scanout pool and every existing detach test passes a registry; what it relies on — a dropped registration fails closed — is proven by the dropped-registration test.

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/drm_cleanup.rs crates/yserver/src/kms/render/resources/mod.rs crates/yserver/src/kms/vk/scanout.rs crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/resources/guard_tests.rs docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md
git commit -m "fix(kms): give the pool-husk registration the alias it accounts for

Stage 2c-i debt, spec 4.1. register_pool_husk was an unkeyed += 1 and
unregister_pool_husk a saturating -= 1, so accounting could be skipped
and a spurious unregister could consume another husk's count.
Registering now takes the husk's own Rc<drm::Device> by value and
yields a PoolHuskRegistration the scanout bo holds; unregistering
consumes it, validates it against the registry's identity and drops the
alias with the count that named it, so the inventory cannot reach zero
while the husk's alias lives (review round 1, B-1). A foreign, unknown
or dropped registration fails closed: the registry refuses to mint
FileFamilyClosed.

Also records the session-2 plan corrections in the spec (4.5).

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 2: Swallowed GPU errors, and the transport close they owe (spec 4.2)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/mod.rs`, `crates/yserver/src/kms/render/resources/transport.rs`, `crates/yserver/src/kms/render/resources/gpu.rs`, `crates/yserver/src/kms/render/scene.rs`, `crates/yserver/src/kms/vk/compositor.rs`
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `gpu::{cancel_pre_submit_batch, freeze_uncertain_batch}`.
- Produces: `TransportGateHandle::{device, incarnation, same_gate}`, filled by `TransportGate::handle`; every `TransportGate` transition reading the effective `state()`; `PresentError::ManagedUnwind { cause, unwind }` and a recursive `scene::present_error_is_device_lost`; `ResourceService::set_transport_gate(TransportGateHandle) -> Result<(), ResourceError>` and `ResourceService::close_transport_gate(&self)`; `gpu::abandon_unsubmitted_batch(&mut ResourceService, &[(AllocationKey, ObligationId)], gpu_submitted: bool) -> Result<(), ResourceError>`; `scene::managed_submit_failure(&mut ResourceService, &[(AllocationKey, ObligationId)], gpu_submitted: bool, cause: PresentError) -> PresentError`.
- Invariants: a service closes only its own transport, and never silently swaps it; a handle-driven close is terminal — no quiescing, permit, publication or grant after it; an unwind failure is part of the returned error, never discarded, and never costs the cause its identity; an uncertain submission closes the transport; a pre-submit failure closes it only when its cancel fails.

- [ ] **Step 1: Apply the production diff**

Extract this task's `diff` block 1 to `/tmp/s2-task2.patch` and apply it.

```diff
diff --git a/crates/yserver/src/kms/render/resources/gpu.rs b/crates/yserver/src/kms/render/resources/gpu.rs
--- a/crates/yserver/src/kms/render/resources/gpu.rs
+++ b/crates/yserver/src/kms/render/resources/gpu.rs
@@ -385,6 +385,29 @@ pub(crate) fn cancel_pre_submit_batch(
     }
 }
 
+/// Spec 4.2 (stage 2c-i debt): unwinds a managed batch whose submission
+/// failed, for the caller to propagate. A submission that may have reached
+/// the GPU freezes its entries and closes the transport (2c-i design section
+/// 4: "Unknown submission retains its reservation and closes the affected
+/// transport"); one that provably did not cancels them, and closes the
+/// transport only if that cancel fails -- the ledger no longer matches what
+/// the batch registered.
+pub(crate) fn abandon_unsubmitted_batch(
+    service: &mut ResourceService,
+    entries: &[(AllocationKey, ObligationId)],
+    gpu_submitted: bool,
+) -> Result<(), ResourceError> {
+    if gpu_submitted {
+        service.close_transport_gate();
+        return freeze_uncertain_batch(service, entries);
+    }
+    let result = cancel_pre_submit_batch(service, entries);
+    if result.is_err() {
+        service.close_transport_gate();
+    }
+    result
+}
+
 /// Task 5.3: a submission whose outcome is uncertain (dispatch may or may
 /// not have reached the GPU) freezes every obligation it was about to
 /// register, rather than cancelling them -- the allocations must not be
diff --git a/crates/yserver/src/kms/render/resources/mod.rs b/crates/yserver/src/kms/render/resources/mod.rs
--- a/crates/yserver/src/kms/render/resources/mod.rs
+++ b/crates/yserver/src/kms/render/resources/mod.rs
@@ -153,6 +155,10 @@ pub(crate) struct ResourceService {
     serviced_elapsed: std::time::Duration,
     last_serviced: Option<Instant>,
     max_serviced_duration: std::time::Duration,
+    /// Spec 4.2 (stage 2c-i debt): the transport an uncertain or unwindable
+    /// GPU submission must close (2c-i design section 4). `None` in
+    /// production, where no gate is installed (R8).
+    transport_gate: Option<TransportGateHandle>,
 }
 
 #[allow(dead_code)]
@@ -174,6 +180,35 @@ impl ResourceService {
             serviced_elapsed: std::time::Duration::ZERO,
             last_serviced: None,
             max_serviced_duration: std::time::Duration::from_secs(5),
+            transport_gate: None,
+        }
+    }
+
+    /// Round-1 B-2: the gate this service closes must be this service's own
+    /// transport. A handle for another device or incarnation is refused, and
+    /// so is a second, different gate -- replacing one would leave the
+    /// transport the service's outstanding work belongs to open. Re-installing
+    /// the same gate is idempotent.
+    pub(crate) fn set_transport_gate(
+        &mut self,
+        gate: TransportGateHandle,
+    ) -> Result<(), ResourceError> {
+        if gate.device() != self.device || gate.incarnation() != self.incarnation {
+            return Err(ResourceError::WrongIncarnation);
+        }
+        if let Some(installed) = &self.transport_gate
+            && !installed.same_gate(&gate)
+        {
+            return Err(ResourceError::InvalidState);
+        }
+        self.transport_gate = Some(gate);
+        Ok(())
+    }
+
+    /// Closes the installed transport gate, if any.
+    pub(crate) fn close_transport_gate(&self) {
+        if let Some(gate) = &self.transport_gate {
+            gate.close_gate();
         }
     }
 
diff --git a/crates/yserver/src/kms/render/resources/transport.rs b/crates/yserver/src/kms/render/resources/transport.rs
--- a/crates/yserver/src/kms/render/resources/transport.rs
+++ b/crates/yserver/src/kms/render/resources/transport.rs
@@ -208,8 +262,14 @@ impl DirectOwnershipState for FakeDirectOwnershipState {
     }
 }
 
+/// Spec 4.2 (stage 2c-i debt), round-1 B-2: a handle names the gate it
+/// closes. `device`/`incarnation` are what a holder validates against its
+/// own identity before installing one; `forced_closed` doubles as the gate's
+/// instance identity, since it is the very cell that gate reads.
 #[derive(Clone, Debug)]
 pub(crate) struct TransportGateHandle {
+    device: DrmDeviceKey,
+    incarnation: IncarnationId,
     forced_closed: Rc<Cell<bool>>,
 }
 
@@ -221,6 +281,20 @@ impl TransportGateHandle {
     pub(crate) fn is_closed(&self) -> bool {
         self.forced_closed.get()
     }
+
+    pub(crate) fn device(&self) -> DrmDeviceKey {
+        self.device
+    }
+
+    pub(crate) fn incarnation(&self) -> IncarnationId {
+        self.incarnation
+    }
+
+    /// True when both handles name the same gate instance, not merely the
+    /// same device and incarnation.
+    pub(crate) fn same_gate(&self, other: &Self) -> bool {
+        Rc::ptr_eq(&self.forced_closed, &other.forced_closed)
+    }
 }
 
 #[derive(Debug)]
@@ -277,7 +351,7 @@ impl TransportGate {
     /// and not retired -- read live from the real state given at
     /// construction (M-13), never from a setter on this gate.
     pub(crate) fn begin_quiescing(&mut self) -> Result<(), ResourceError> {
-        if self.state == TransportState::Closed {
+        if self.state() == TransportState::Closed {
             return Err(ResourceError::Detached);
         }
         if self.ownership.direct_ownership_busy()
@@ -315,10 +389,18 @@ impl TransportGate {
 
     pub(crate) fn handle(&self) -> TransportGateHandle {
         TransportGateHandle {
+            device: self.device,
+            incarnation: self.incarnation,
             forced_closed: Rc::clone(&self.forced_closed),
         }
     }
 
+    /// The effective state. Round-2 B-1: every transition below consults
+    /// this, never the raw `self.state`, because a close can arrive through a
+    /// `TransportGateHandle` -- which owns no `&mut TransportGate` and can
+    /// only set the shared flag. A handle-driven close must be as terminal as
+    /// `close()` itself: no quiescing, no permit, no publication, no grant
+    /// after it.
     pub(crate) fn state(&self) -> TransportState {
         if self.forced_closed.get() {
             TransportState::Closed
@@ -368,7 +450,7 @@ impl TransportGate {
         &mut self,
         class: WriterClass,
     ) -> Result<OwnerWriteGrant, ResourceError> {
-        if self.state != TransportState::Owner {
+        if self.state() != TransportState::Owner {
             return Err(ResourceError::Detached);
         }
         if self.closed_admission.get() {
@@ -399,7 +481,7 @@ impl TransportGate {
             self.force_close();
             return Err((ResourceError::WrongIncarnation, grant));
         }
-        if self.state != TransportState::Owner {
+        if self.state() != TransportState::Owner {
             return Err((ResourceError::Detached, grant));
         }
         if !self.issued_serials.remove(&grant.serial) {
@@ -488,7 +573,7 @@ impl TransportGate {
         if permit.device != self.device || permit.incarnation != self.incarnation {
             return Err(ResourceError::WrongIncarnation);
         }
-        if self.state != TransportState::Quiescing {
+        if self.state() != TransportState::Quiescing {
             return Err(ResourceError::Busy);
         }
         if self.outstanding_owner_writes != 0 {
diff --git a/crates/yserver/src/kms/render/scene.rs b/crates/yserver/src/kms/render/scene.rs
--- a/crates/yserver/src/kms/render/scene.rs
+++ b/crates/yserver/src/kms/render/scene.rs
@@ -137,8 +137,15 @@ fn kms_retirement_matches(
     pending_bo_idx == presented_bo_idx && stage.is_kms_flip_pending()
 }
 
-fn present_error_is_device_lost(error: &PresentError) -> bool {
-    matches!(error, PresentError::Vk(vk::Result::ERROR_DEVICE_LOST))
+pub(crate) fn present_error_is_device_lost(error: &PresentError) -> bool {
+    match error {
+        PresentError::Vk(vk::Result::ERROR_DEVICE_LOST) => true,
+        // Round-2 B-2: a failed unwind wraps its cause; the classification
+        // has to reach through it or a lost device stops being recognised
+        // exactly when the frame also failed to unwind.
+        PresentError::ManagedUnwind { cause, .. } => present_error_is_device_lost(cause),
+        _ => false,
+    }
 }
 
 enum CopiedRenderSubmitError {
@@ -7967,26 +7974,22 @@ fn submit_shared_scanout_frame(
     let (submitted, fb_handle) = match render_res {
         Ok(Ok((sub, fb))) => (sub, fb),
         Ok(Err(render_err)) => {
-            if *gpu_submitted {
-                let _ =
-                    crate::kms::render::resources::gpu::freeze_uncertain_batch(service, &entries);
-            } else {
-                let _ =
-                    crate::kms::render::resources::gpu::cancel_pre_submit_batch(service, &entries);
-            }
-            return Err(render_err);
+            return Err(managed_submit_failure(
+                service,
+                &entries,
+                *gpu_submitted,
+                render_err,
+            ));
         }
         Err(res_err) => {
-            if *gpu_submitted {
-                let _ =
-                    crate::kms::render::resources::gpu::freeze_uncertain_batch(service, &entries);
-            } else {
-                let _ =
-                    crate::kms::render::resources::gpu::cancel_pre_submit_batch(service, &entries);
-            }
-            return Err(PresentError::Io(std::io::Error::other(format!(
-                "with_scanout_write: {res_err:?}"
-            ))));
+            return Err(managed_submit_failure(
+                service,
+                &entries,
+                *gpu_submitted,
+                PresentError::Io(std::io::Error::other(format!(
+                    "with_scanout_write: {res_err:?}"
+                ))),
+            ));
         }
     };
 
@@ -8037,6 +8040,29 @@ fn submit_shared_scanout_frame(
     }
 }
 
+/// Spec 4.2 (stage 2c-i debt): the error a failed managed submission reports.
+/// Unwinding its batch is not best-effort: a failure there is part of the
+/// result, never discarded, and `abandon_unsubmitted_batch` has already
+/// closed the transport for it.
+pub(crate) fn managed_submit_failure(
+    service: &mut ResourceService,
+    entries: &[(AllocationKey, crate::kms::render::resources::ObligationId)],
+    gpu_submitted: bool,
+    cause: PresentError,
+) -> PresentError {
+    match crate::kms::render::resources::gpu::abandon_unsubmitted_batch(
+        service,
+        entries,
+        gpu_submitted,
+    ) {
+        Ok(()) => cause,
+        Err(unwind) => PresentError::ManagedUnwind {
+            cause: Box::new(cause),
+            unwind: format!("{unwind:?}"),
+        },
+    }
+}
+
 /// Render into A's exportable source. The paired destination phase reserves
 /// the same BO index until readiness advances the frame to B's copy + KMS
 /// submission on the main-loop boundary.
diff --git a/crates/yserver/src/kms/vk/compositor.rs b/crates/yserver/src/kms/vk/compositor.rs
--- a/crates/yserver/src/kms/vk/compositor.rs
+++ b/crates/yserver/src/kms/vk/compositor.rs
@@ -39,6 +39,17 @@ pub enum PresentError {
     NoFb,
     #[error("scanout bo state machine wrong phase: {0:?}")]
     WrongPhase(BoPhase),
+    /// Round-2 B-2 (stage 2c-i debt): a managed submission that failed AND
+    /// whose batch could not be unwound. `cause` is kept structurally, not
+    /// flattened into text, because callers classify it -- a device loss
+    /// buried in a formatted string stops latching the fatal renderer state.
+    /// `unwind` is the ledger error, rendered, since this layer sits below
+    /// `resources`.
+    #[error("{cause}; unwinding the managed batch also failed: {unwind}")]
+    ManagedUnwind {
+        cause: Box<PresentError>,
+        unwind: String,
+    },
 }
 
 impl From<vk::Result> for PresentError {
```

Expected: five files patched, no rejects, no fuzz.

- [ ] **Step 2: Append the tests**

Extract this task's `rust` block 1 to `/tmp/s2-task2.rs` and append it to `guard_tests.rs`.

```rust

// ---------------------------------------------------------------------------
// Session 2, spec 4.2: a failed managed submission's unwind is propagated,
// and the transport closes when the submission is uncertain or unwinding
// fails.
// ---------------------------------------------------------------------------

use crate::kms::{render::scene::managed_submit_failure, vk::compositor::PresentError};

/// A service with one spy entry holding a pending GPU obligation, and a
/// legacy transport gate installed on it.
fn submission_fixture() -> (
    ResourceService,
    AllocationLease,
    ObligationId,
    TransportGate,
) {
    let (mut service, held, _drops) = spy_service();
    let obligation = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let gate = TransportGate::for_tests(service.device(), IncarnationId::first());
    service.set_transport_gate(gate.handle()).unwrap();
    (service, held, obligation, gate)
}

/// census: S2-gate-install-identity mod.rs set_transport_gate `gate.device() != self.device || gate.incarnation() != self.incarnation`
#[test]
fn c0_2ci_guard_service_refuses_a_transport_gate_for_another_transport() {
    let (mut service, _held, _obligation, _gate) = submission_fixture();
    let other_device = TransportGate::for_tests(
        DrmDeviceKey {
            major: 226,
            minor: 1,
        },
        IncarnationId::first(),
    );
    let other_incarnation =
        TransportGate::for_tests(service.device(), IncarnationId::first().next());
    for foreign in [&other_device, &other_incarnation] {
        assert_eq!(
            service.set_transport_gate(foreign.handle()),
            Err(ResourceError::WrongIncarnation),
            "a service must refuse a gate for another transport [census:S2-gate-install-identity]"
        );
    }
    service.close_transport_gate();
    assert_eq!(other_device.state(), TransportState::Legacy);
    assert_eq!(other_incarnation.state(), TransportState::Legacy);
}

/// census: S2-gate-install-replacement mod.rs set_transport_gate `let Some(installed) = &self.transport_gate && !installed.same_gate(&gate)`
#[test]
fn c0_2ci_guard_service_refuses_a_second_transport_gate() {
    let (mut service, _held, _obligation, gate) = submission_fixture();
    // Same device and incarnation, a different gate instance.
    let replacement = TransportGate::for_tests(service.device(), IncarnationId::first());
    assert_eq!(
        service.set_transport_gate(replacement.handle()),
        Err(ResourceError::InvalidState),
        "a service must refuse to swap the transport it closes [census:S2-gate-install-replacement]"
    );
    // Re-installing the gate already there is idempotent.
    service.set_transport_gate(gate.handle()).unwrap();
    service.close_transport_gate();
    assert_eq!(gate.state(), TransportState::Closed);
    assert_eq!(replacement.state(), TransportState::Legacy);
}

/// Round-2 B-2: a failed unwind must not cost the cause its identity. The
/// caller latches the fatal renderer state on a device loss, and it
/// classifies the error it is handed.
#[test]
fn c0_2ci_failed_unwind_keeps_a_device_loss_recognisable() {
    let (mut service, held, obligation, _gate) = submission_fixture();
    service.cancel(held.key(), obligation).unwrap();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::Vk(ash::vk::Result::ERROR_DEVICE_LOST),
    );
    assert!(
        crate::kms::render::scene::present_error_is_device_lost(&err),
        "a device loss must survive a failed unwind, got {err}"
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "and the unwind failure must still be reported, got {err}"
    );
}

/// census: S2-cancel-pre-submit-error gpu.rs cancel_pre_submit_batch `Some(err) =>`
#[test]
fn c0_2ci_guard_failed_pre_submit_cancel_is_reported_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    // The obligation is already gone, so the unwind's cancel must fail.
    service.cancel(held.key(), obligation).unwrap();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::NoFb,
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "a failed pre-submit cancel must reach the caller, got {err} [census:S2-cancel-pre-submit-error]"
    );
    assert_eq!(gate.state(), TransportState::Closed);
}

/// census: S2-freeze-uncertain-error gpu.rs freeze_uncertain_batch `Some(err) =>`
#[test]
fn c0_2ci_guard_failed_uncertain_freeze_is_reported_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    // A key from another incarnation cannot be frozen here.
    let err = managed_submit_failure(
        &mut service,
        &[(wrong_incarnation(held.key()), obligation)],
        true,
        PresentError::NoFb,
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "a failed freeze of an uncertain submission must reach the caller, got {err} [census:S2-freeze-uncertain-error]"
    );
    assert_eq!(gate.state(), TransportState::Closed);
}

/// Round-2 B-1: a close that arrives through the service's handle is as
/// terminal as `close()`. The handle can only set the shared flag, so every
/// transition has to read the effective state, not the raw field. This
/// covers `begin_quiescing`; the Owner-side transitions need the handover
/// evidence Task 4 reshapes, so they are proven in Task 5.
#[test]
fn c0_2ci_service_driven_close_stops_the_gate_quiescing() {
    let (service, _held, _obligation, mut gate) = submission_fixture();
    service.close_transport_gate();
    assert_eq!(gate.state(), TransportState::Closed);
    assert_eq!(
        gate.begin_quiescing(),
        Err(ResourceError::Detached),
        "a transport closed through its handle must refuse to quiesce"
    );
}

#[test]
fn c0_2ci_uncertain_submission_freezes_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        true,
        PresentError::NoFb,
    );
    assert!(matches!(err, PresentError::NoFb), "got {err}");
    assert_eq!(
        gate.state(),
        TransportState::Closed,
        "an uncertain submission must close the transport"
    );
    assert!(
        service.is_frozen(&held.key()),
        "an uncertain submission must freeze its entries"
    );
}

#[test]
fn c0_2ci_pre_submit_failure_cancels_and_leaves_transport_open() {
    let (mut service, held, obligation, gate) = submission_fixture();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::NoFb,
    );
    assert!(matches!(err, PresentError::NoFb), "got {err}");
    assert_eq!(gate.state(), TransportState::Legacy);
    assert!(!service.has_pending_obligation(&held.key(), obligation));
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo +nightly fmt && cargo test -p yserver --lib _transport`
Expected: the eight new tests pass — the two gate-install guards, the four submission-failure tests, the quiescing half of the terminal-close test and the device-loss test — along with any other matching tests. A failure is an F8 stop.

- [ ] **Step 4: Run the oracle**

Run: `python3 tools/guard-census.py --files gpu.rs --deterministic-only --require-oracle`, then `python3 tools/guard-census.py --files mod.rs --fn set_transport_gate --deterministic-only --require-oracle`
Expected: `S2-cancel-pre-submit-error` and `S2-freeze-uncertain-error` `CAUGHT_BY_ORACLE` (summary 2); then `S2-gate-install-identity` and `S2-gate-install-replacement` `CAUGHT_BY_ORACLE` (summary 2); exit 0 both times.

- [ ] **Step 5: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; `c0_2ci` 171 passed, 18 ignored.

*Reviewer mutations* (coordinator): delete `service.close_transport_gate();` from the `gpu_submitted` branch of `abandon_unsubmitted_batch` (fails `…failed_uncertain_freeze…` and `…uncertain_submission_freezes…`); delete the `if result.is_err() { … }` close (fails `…failed_pre_submit_cancel…`); revert `begin_quiescing`'s `self.state()` check to the raw `self.state` (fails `c0_2ci_service_driven_close_stops_the_gate_quiescing`; the other four are Task 5's); delete the `PresentError::ManagedUnwind` arm of `present_error_is_device_lost` (fails `c0_2ci_failed_unwind_keeps_a_device_loss_recognisable`); and read both `return Err(managed_submit_failure(` arms in `submit_shared_scanout_frame` (plan correction 1).

```bash
git add crates/yserver/src/kms/render/resources/mod.rs crates/yserver/src/kms/render/resources/transport.rs crates/yserver/src/kms/render/resources/gpu.rs crates/yserver/src/kms/render/scene.rs crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "fix(kms): propagate a failed managed-batch unwind and close the transport

Stage 2c-i debt, spec 4.2. submit_shared_scanout_frame discarded the
results of cancel_pre_submit_batch and freeze_uncertain_batch with
let _, so their error arms could not be proven and an unwind failure
vanished. Both failure arms now return managed_submit_failure, which
reports an unwind failure alongside the cause. An uncertain submission
closes the transport installed on the ResourceService (2c-i design
section 4), and so does a failed pre-submit cancel. The gate handle now
names its gate, and a service refuses one for another transport or a
silent replacement (review round 1, B-2). Inert in production, where no
gate is installed (R8).

Round 2: a close arriving through the handle is terminal -- every
transition now reads the effective state (B-1) -- and a failed unwind
keeps its cause structurally in PresentError::ManagedUnwind, so a
device loss still latches the fatal renderer state (B-2).

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 3: The reset boundary (spec 4.3, with its F8 stop)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/mod.rs`
- Create: `crates/yserver/src/kms/render/resources/reset_boundary_tests.rs`

**Interfaces:**
- Consumes: `yserver_core::core_loop::reset::force_destroy_all_clients`; `KmsBackend::{for_tests, install_resource_service, resource_service_mut}` and its `store` field; `resources::tests::SpyAllocation`.
- Invariants proven: after the forced teardown the old XID no longer resolves while its backing stays rooted; after the numeric protocol and host XIDs are reused over the same backend, the old generation's late proof releases only the old entry, and the new drawable resolves to its own backing.
- **Not proven, by decision:** the reset's own generation replacement. The module doc carries the F8 stop; do not add wiring to close it.

- [ ] **Step 1: Declare the module**

Extract this task's `diff` block 1 to `/tmp/s2-task3.patch` and apply it.

```diff
diff --git a/crates/yserver/src/kms/render/resources/mod.rs b/crates/yserver/src/kms/render/resources/mod.rs
--- a/crates/yserver/src/kms/render/resources/mod.rs
+++ b/crates/yserver/src/kms/render/resources/mod.rs
@@ -16,6 +16,8 @@ mod adapter_tests;
 #[cfg(test)]
 mod guard_tests;
 #[cfg(test)]
+mod reset_boundary_tests;
+#[cfg(test)]
 pub(crate) mod tests;
 
 use std::{
```

- [ ] **Step 2: Create the test module**

Extract this task's `rust` block 1 straight to `crates/yserver/src/kms/render/resources/reset_boundary_tests.rs`.

```rust
//! Stage 2c-i debt, session 2, item 4.3: the server reset's forced teardown
//! erases a managed drawable's identity but not its proof-gated backing, and
//! a proof arriving after the numeric XIDs are reused reaches only the old
//! incarnation's entry. Spec:
//! docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md §4.3.
//!
//! **What this does NOT prove (F8 stop, round-1 B-3).** Spec 4.3 asks the
//! test to continue across the reset's own generation replacement.
//! `reset_generation` is `pub(crate)` in `yserver-core` and needs a live
//! poller, a setup registry and an input inventory, none of which this crate
//! can supply, and spec 4.3 says that an undrivable half is an F8 stop to
//! report rather than a reason to add wiring. So the second half below
//! reuses the numeric XIDs over the same backend without crossing the real
//! boundary: it proves the ledger keys proofs by allocation and not by XID,
//! and it does not prove the reset's invariant 6. The crossing itself stays
//! open, for whoever owns the boundary next.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    os::unix::net::UnixStream,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU16},
};

use yserver_core::{
    backend::PixmapHandle,
    core_loop::reset::force_destroy_all_clients,
    resources::ROOT_WINDOW,
    server::{ClientState, ServerState},
};
use yserver_protocol::x11::{ClientByteOrder, ClientId, CreatePixmapRequest, ResourceId};

use super::{
    AllocationKey, AllocationPayload, ObligationKind, ResourceService,
    storage::{PixelIdentity, StorageBacking, StorageLease},
    tests::SpyAllocation,
};
use crate::{
    kms::{
        owner::identity::IncarnationId,
        render::{
            backend::KmsBackend,
            store::{DrawableId, DrawableKind, Storage},
            target::PaintTarget,
        },
    },
    platform::drm::DrmDeviceKey,
};

const CLIENT: u32 = 7;
/// The protocol XID both generations use, deliberately identical.
const PIXMAP: ResourceId = ResourceId(0x0070_0002);
/// The backend's host XID both generations use, deliberately identical.
const HOST_XID: u32 = 0x0400_0002;

fn install_client(state: &mut ServerState, id: u32) {
    let (a, _b) = UnixStream::pair().unwrap();
    state.clients.insert(
        id,
        ClientState {
            writer: Arc::new(Mutex::new(yserver_core::transport::Transport::Unix(a))),
            byte_order: ClientByteOrder::LittleEndian,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base: 0,
            resource_id_mask: u32::MAX,
            event_masks: HashMap::new(),
            save_set: HashSet::new(),
            big_requests_enabled: false,
            xi2_masks: HashMap::new(),
            xi1_event_classes: HashSet::new(),
            xi1_window_event_classes: HashMap::new(),
            outbound: VecDeque::new(),
            watching_writable: false,
            focused_window: ROOT_WINDOW,
            reader_control: None,
            is_local: true,
            fd_passing: true,
        },
    );
}

/// One client owning `PIXMAP`, backed by a managed drawable at `HOST_XID`
/// whose allocation is a fresh spy entry. Returns the drawable, its key and
/// the spy's drop counter.
fn seed_managed_pixmap(
    state: &mut ServerState,
    backend: &mut KmsBackend,
) -> (DrawableId, AllocationKey, Rc<Cell<usize>>) {
    install_client(state, CLIENT);
    let drops = Rc::new(Cell::new(0));
    let service = backend.resource_service_mut().expect("service installed");
    let lease = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops),
        }))
        .expect("adopt");
    let key = lease.key();
    let storage = Storage::from_backing(StorageBacking::Managed(StorageLease {
        allocation: lease,
        pixels: PixelIdentity {
            target: PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24),
            allocation: key,
            content_offset: (0, 0),
            extent: ash::vk::Extent2D {
                width: 16,
                height: 16,
            },
            format: ash::vk::Format::B8G8R8A8_UNORM,
            image_view: ash::vk::ImageView::null(),
            sample_view: ash::vk::ImageView::null(),
            image: ash::vk::Image::null(),
        },
    }));
    let id = backend
        .store
        .allocate(HOST_XID, DrawableKind::Pixmap, 24, false, storage)
        .expect("the host XID is free");
    state.resources.create_pixmap(
        ClientId(CLIENT),
        CreatePixmapRequest {
            pixmap: PIXMAP,
            drawable: ROOT_WINDOW,
            width: 16,
            height: 16,
            depth: 24,
        },
    );
    assert!(
        state
            .resources
            .set_pixmap_host_xid(PIXMAP, PixmapHandle::from_raw(HOST_XID).unwrap())
    );
    (id, key, drops)
}

#[test]
fn c0_2ci_reset_forced_teardown_keeps_gated_backing_and_late_proof_skips_reused_xid() {
    let mut backend = KmsBackend::for_tests();
    backend.install_resource_service(ResourceService::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
    ));

    // Generation 1: a managed drawable with GPU work still pending.
    let mut state = ServerState::new();
    let (old_id, old_key, old_drops) = seed_managed_pixmap(&mut state, &mut backend);
    let pending = backend
        .resource_service_mut()
        .unwrap()
        .register(old_key, ObligationKind::Gpu)
        .expect("register");

    // The reset's own forced teardown.
    force_destroy_all_clients(&mut state, &mut backend);

    // Identity is erased...
    assert!(state.resources.pixmap(PIXMAP).is_none());
    assert_eq!(
        backend.store.lookup(HOST_XID),
        None,
        "the old host XID still resolves after the forced teardown"
    );
    // ...but the backing is still rooted, its release gated on the proof.
    let service = backend.resource_service_mut().unwrap();
    service.service_ready();
    assert!(
        service.contains(&old_key),
        "the forced teardown released a backing whose GPU obligation is pending"
    );
    assert_eq!(old_drops.get(), 0, "the pending backing was destroyed");

    // A second session reuses both numeric XIDs over the same backend. This
    // is not the reset's own generation replacement -- see the module's F8
    // stop -- it is the numeric reuse that replacement would produce.
    let mut state = ServerState::new();
    let (new_id, new_key, new_drops) = seed_managed_pixmap(&mut state, &mut backend);
    assert_ne!(new_id, old_id);
    assert_ne!(new_key, old_key);

    // The old generation's proof arrives late.
    let service = backend.resource_service_mut().unwrap();
    service
        .apply_validated_proof(old_key, pending)
        .expect("the late proof names the old entry");
    service.service_ready();
    assert!(
        !service.contains(&old_key),
        "the old backing was never released"
    );
    assert_eq!(old_drops.get(), 1);
    assert!(
        service.contains(&new_key),
        "the late proof released the new generation's backing"
    );
    assert_eq!(new_drops.get(), 0);

    // The new drawable still resolves, to its own backing.
    assert_eq!(backend.store.lookup(HOST_XID), Some(new_id));
    let lease_key = backend
        .store
        .get(new_id)
        .and_then(|d| d.managed_lease())
        .map(|lease| lease.allocation.key());
    assert_eq!(lease_key, Some(new_key));
    drop(state);
}
```

- [ ] **Step 3: Run the test**

Run: `cargo +nightly fmt && cargo test -p yserver --lib c0_2ci_reset_`
Expected: `1 passed`. A failure is an F8 stop.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; `c0_2ci` 172 passed, 18 ignored.

*Reviewer mutations* (coordinator, the first two prototyped): in `availability.rs` `can_destroy`, drop `&& entry.pending_obligation_count() == 0` (fails at "the forced teardown released a backing whose GPU obligation is pending"); in `store.rs` `destroy_now`, force `if self.by_xid.get(&drawable.xid).copied() == Some(id)` to `if false` (fails at "the old host XID still resolves"); in `ResourceService::apply_validated_proof`, resolve the key's newest sibling entry of the same device and incarnation instead of the key itself (must fail the test).

```bash
git add crates/yserver/src/kms/render/resources/mod.rs crates/yserver/src/kms/render/resources/reset_boundary_tests.rs
git commit -m "test(kms): prove the forced teardown keeps a proof-gated backing

Stage 2c-i debt, spec 4.3. Drives force_destroy_all_clients over a
KmsBackend holding a managed drawable with GPU work pending: the XID
stops resolving while the backing stays rooted. Reusing both numeric
XIDs afterwards shows the late proof releases only the old entry, since
the ledger keys proofs by allocation, not by XID.

The reset's own generation replacement is NOT driven: reset_generation
is pub(crate) in yserver-core and needs a live poller, setup registry
and input inventory. Spec 4.3 calls that an F8 stop to report rather
than a reason to add wiring, and the module doc records it (review
round 1, B-3).

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 4: Handover evidence (spec 4.4, first half)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/transport.rs`, `crates/yserver/src/kms/render/resources/handoff.rs`, `crates/yserver/src/kms/render/resources/tests.rs`, `crates/yserver/src/kms/render/platform.rs`
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Produces: `RecipientReservation::new_for_tests(DrmDeviceKey, IncarnationId)`; `TestWriterCoverage::{OwnerMediatedMock, Disabled}`; `TestWriterCoverageEvidence` (one field per `WriterClass`, with an exhaustive `coverage(WriterClass)`); `WriterCoverageProof::new_for_tests(TestWriterCoverageEvidence)`; `resources::tests::writer_coverage_for_tests() -> WriterCoverageProof`.
- Produces: `issue_handover_permit` refuses a reservation for another device or incarnation with `WrongIncarnation` — a new production guard, after the proof's incarnation check.
- Produces for Task 5 (in the appended block): `handover_device()`, `drained(IncarnationId)`, `quiescing_gate(DrmDeviceKey, IncarnationId)`, `permit_for(&mut TransportGate)`.

- [ ] **Step 1: Apply the production diff**

Extract this task's `diff` block 1 to `/tmp/s2-task4.patch` and apply it.

```diff
diff --git a/crates/yserver/src/kms/render/platform.rs b/crates/yserver/src/kms/render/platform.rs
--- a/crates/yserver/src/kms/render/platform.rs
+++ b/crates/yserver/src/kms/render/platform.rs
@@ -7908,7 +7919,7 @@ mod tests {
     #[test]
     fn c0_2ci_sink_cursor_gate_four_states() {
         use crate::kms::render::resources::{
-            FakeDirectOwnershipState, RecipientReservation, TransportGate, WriterCoverageProof,
+            FakeDirectOwnershipState, RecipientReservation, TransportGate,
         };
 
         let mut platform = PlatformBackend::for_tests();
@@ -7960,8 +7971,8 @@ mod tests {
                     lifecycle: crate::kms::owner::lifecycle::LifecycleEpochId::first(),
                 },
                 &[],
-                &WriterCoverageProof::new_for_tests(),
-                RecipientReservation::new_for_tests(),
+                &crate::kms::render::resources::tests::writer_coverage_for_tests(),
+                RecipientReservation::new_for_tests(key, incarnation),
             )
             .unwrap();
         platform
diff --git a/crates/yserver/src/kms/render/resources/handoff.rs b/crates/yserver/src/kms/render/resources/handoff.rs
--- a/crates/yserver/src/kms/render/resources/handoff.rs
+++ b/crates/yserver/src/kms/render/resources/handoff.rs
@@ -337,7 +337,11 @@ impl RetainingSupervisor {
         device: DrmDeviceKey,
         incarnation: IncarnationId,
     ) -> RecipientSlot {
-        RecipientSlot::new(device, incarnation, RecipientReservation::new_for_tests())
+        RecipientSlot::new(
+            device,
+            incarnation,
+            RecipientReservation::new_for_tests(device, incarnation),
+        )
     }
 
     pub(crate) fn issue_teardown_release(
diff --git a/crates/yserver/src/kms/render/resources/tests.rs b/crates/yserver/src/kms/render/resources/tests.rs
--- a/crates/yserver/src/kms/render/resources/tests.rs
+++ b/crates/yserver/src/kms/render/resources/tests.rs
@@ -2612,6 +2612,24 @@ fn c0_2ci_completion_waiter_registration_and_recheck() {
     assert!(wakes.contains(&ResourceConsumer::DirectCapacity));
 }
 
+/// Spec 4.4 (stage 2c-i debt): a writer-coverage proof built from explicit
+/// evidence for every writer class. Every class is an owner-mediated mock
+/// here; a test that needs one disabled builds its own evidence.
+pub(crate) fn writer_coverage_for_tests() -> WriterCoverageProof {
+    use super::transport::{TestWriterCoverage::OwnerMediatedMock, TestWriterCoverageEvidence};
+    WriterCoverageProof::new_for_tests(TestWriterCoverageEvidence {
+        primary: OwnerMediatedMock,
+        unflip: OwnerMediatedMock,
+        modeset: OwnerMediatedMock,
+        dpms: OwnerMediatedMock,
+        vt: OwnerMediatedMock,
+        topology: OwnerMediatedMock,
+        cursor: OwnerMediatedMock,
+        gamma: OwnerMediatedMock,
+        helper_mutation: OwnerMediatedMock,
+    })
+}
+
 /// M-14: a real-shaped `LegacyDrained` proof for `issue_handover_permit`,
 /// matching `incarnation` the way the backend's genuine
 /// `issue_legacy_drained` output would.
@@ -2674,8 +2692,8 @@ fn c0_2ci_transport_gate_vocabulary_and_table() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
@@ -2788,8 +2806,8 @@ fn c0_2ci_transport_gate_owner_write_contract() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
@@ -2834,8 +2852,8 @@ fn c0_2ci_transport_gate_owner_write_contract() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate2.device(), gate2.incarnation()),
         )
         .unwrap();
     gate2.publish_owner(permit2).unwrap();
@@ -2903,8 +2921,8 @@ fn c0_2ci_transport_gate_consume_owner_write_checked_subtraction() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
@@ -2942,8 +2960,8 @@ fn c0_2ci_transport_gate_close_refuses_outstanding_grants() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
@@ -2990,8 +3008,8 @@ fn c0_2ci_transport_gate_handover_validates_proof_and_dispositions() {
         .issue_handover_permit(
             legacy_drained_for_tests(foreign_incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap_err();
     assert_eq!(err, ResourceError::WrongIncarnation);
@@ -3008,8 +3026,8 @@ fn c0_2ci_transport_gate_handover_validates_proof_and_dispositions() {
             &[LegacyEventDisposition::Cancelled(
                 LegacyEventCancellation::BackendFailure,
             )],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap_err();
     assert_eq!(err2, ResourceError::InvalidProof);
@@ -3019,8 +3037,8 @@ fn c0_2ci_transport_gate_handover_validates_proof_and_dispositions() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[LegacyEventDisposition::Applied],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     assert!(gate.publish_owner(permit).is_ok());
@@ -3050,8 +3068,8 @@ pub(crate) fn owner_gate_for_tests(
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
@@ -3089,8 +3107,8 @@ fn sink_gate_at_state(target: TransportState) -> TransportGate {
             .issue_handover_permit(
                 legacy_drained_for_tests(incarnation),
                 &[],
-                &WriterCoverageProof::new_for_tests(),
-                RecipientReservation::new_for_tests(),
+                &writer_coverage_for_tests(),
+                RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
             )
             .unwrap();
         gate.publish_owner(permit).unwrap();
@@ -3273,8 +3291,8 @@ fn c0_2ci_sink_gamma_gate_four_states_drm() {
                     .issue_handover_permit(
                         legacy_drained_for_tests(IncarnationId::first()),
                         &[],
-                        &WriterCoverageProof::new_for_tests(),
-                        RecipientReservation::new_for_tests(),
+                        &writer_coverage_for_tests(),
+                        RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
                     )
                     .unwrap();
                 gate.publish_owner(permit).unwrap();
@@ -5390,8 +5408,8 @@ fn c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines() {
         .issue_handover_permit(
             legacy_drained_for_tests(incarnation),
             &[],
-            &WriterCoverageProof::new_for_tests(),
-            RecipientReservation::new_for_tests(),
+            &writer_coverage_for_tests(),
+            RecipientReservation::new_for_tests(gate.device(), gate.incarnation()),
         )
         .unwrap();
     gate.publish_owner(permit).unwrap();
diff --git a/crates/yserver/src/kms/render/resources/transport.rs b/crates/yserver/src/kms/render/resources/transport.rs
--- a/crates/yserver/src/kms/render/resources/transport.rs
+++ b/crates/yserver/src/kms/render/resources/transport.rs
@@ -84,9 +84,13 @@ impl Drop for OwnerWriteGrant {
     }
 }
 
-/// Opaque capability representing reservation of the recipient endpoint for Owner publication.
+/// Opaque capability representing reservation of the recipient endpoint for
+/// Owner publication. Spec 4.4 (stage 2c-i debt): it names the device and
+/// incarnation of the recipient slot it reserves, and `issue_handover_permit`
+/// refuses one reserved for another.
 pub(crate) struct RecipientReservation {
-    _private: (),
+    device: DrmDeviceKey,
+    incarnation: IncarnationId,
 }
 
 impl RecipientReservation {
@@ -98,8 +102,11 @@ impl RecipientReservation {
     // back under `#[cfg(test)]` in `handoff.rs`, so this constructor can
     // live under `#[cfg(test)]` again with no production caller needing it.
     #[cfg(test)]
-    pub(crate) fn new_for_tests() -> Self {
-        Self { _private: () }
+    pub(crate) fn new_for_tests(device: DrmDeviceKey, incarnation: IncarnationId) -> Self {
+        Self {
+            device,
+            incarnation,
+        }
     }
 }
 
@@ -108,9 +115,56 @@ pub(crate) struct WriterCoverageProof {
     _private: (),
 }
 
+/// Test-only coverage for one writer class (spec 4.4, stage 2c-i debt): the
+/// stage-2c design allows Owner publication only when every writer class is
+/// owner-mediated or disabled.
+#[cfg(test)]
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub(crate) enum TestWriterCoverage {
+    OwnerMediatedMock,
+    Disabled,
+}
+
+/// Test-only evidence naming the coverage of every `WriterClass`, one field
+/// each, so none can be left out.
+#[cfg(test)]
+#[derive(Debug, Clone, Copy)]
+pub(crate) struct TestWriterCoverageEvidence {
+    pub(crate) primary: TestWriterCoverage,
+    pub(crate) unflip: TestWriterCoverage,
+    pub(crate) modeset: TestWriterCoverage,
+    pub(crate) dpms: TestWriterCoverage,
+    pub(crate) vt: TestWriterCoverage,
+    pub(crate) topology: TestWriterCoverage,
+    pub(crate) cursor: TestWriterCoverage,
+    pub(crate) gamma: TestWriterCoverage,
+    pub(crate) helper_mutation: TestWriterCoverage,
+}
+
+#[cfg(test)]
+impl TestWriterCoverageEvidence {
+    /// Exhaustive over `WriterClass`: a class added without a field here
+    /// stops this from compiling, and so every test that builds a proof.
+    pub(crate) fn coverage(&self, class: WriterClass) -> TestWriterCoverage {
+        match class {
+            WriterClass::Primary => self.primary,
+            WriterClass::Unflip => self.unflip,
+            WriterClass::Modeset => self.modeset,
+            WriterClass::Dpms => self.dpms,
+            WriterClass::Vt => self.vt,
+            WriterClass::Topology => self.topology,
+            WriterClass::Cursor => self.cursor,
+            WriterClass::Gamma => self.gamma,
+            WriterClass::HelperMutation => self.helper_mutation,
+        }
+    }
+}
+
 impl WriterCoverageProof {
+    /// Spec 4.4: consumes explicit coverage evidence for every writer class,
+    /// so possessing a proof proves the coverage.
     #[cfg(test)]
-    pub(crate) fn new_for_tests() -> Self {
+    pub(crate) fn new_for_tests(_evidence: TestWriterCoverageEvidence) -> Self {
         Self { _private: () }
     }
 }
@@ -456,9 +538,9 @@ impl TransportGate {
         proof: crate::kms::render::platform::LegacyDrained,
         dispositions: &[crate::kms::render::backend::LegacyEventDisposition],
         _coverage: &WriterCoverageProof,
-        _reservation: RecipientReservation,
+        reservation: RecipientReservation,
     ) -> Result<HandoverPermit, ResourceError> {
-        if self.state != TransportState::Quiescing {
+        if self.state() != TransportState::Quiescing {
             return Err(ResourceError::Busy);
         }
         if self.outstanding_owner_writes != 0 {
@@ -467,6 +549,9 @@ impl TransportGate {
         if proof.incarnation != self.incarnation {
             return Err(ResourceError::WrongIncarnation);
         }
+        if reservation.device != self.device || reservation.incarnation != self.incarnation {
+            return Err(ResourceError::WrongIncarnation);
+        }
         let backend_failure = dispositions.iter().any(|d| {
             matches!(
                 d,
```

Expected: four files patched, no rejects, no fuzz.

- [ ] **Step 2: Append the helpers and the reservation test**

Extract this task's `rust` block 1 to `/tmp/s2-task4.rs` and append it to `guard_tests.rs`.

```rust

// ---------------------------------------------------------------------------
// Session 2, spec 4.4: the Owner handover, on evidence that proves coverage
// of every writer class and a reservation bound to the recipient's identity.
// ---------------------------------------------------------------------------

fn handover_device() -> DrmDeviceKey {
    DrmDeviceKey {
        major: 226,
        minor: 0,
    }
}

fn drained(incarnation: IncarnationId) -> crate::kms::render::platform::LegacyDrained {
    crate::kms::render::platform::LegacyDrained {
        incarnation,
        lifecycle: crate::kms::owner::lifecycle::LifecycleEpochId::first(),
    }
}

fn quiescing_gate(device: DrmDeviceKey, incarnation: IncarnationId) -> TransportGate {
    let mut gate = TransportGate::for_tests(device, incarnation);
    gate.begin_quiescing().unwrap();
    gate
}

/// Issues a permit from `gate` on complete evidence for its own identity.
fn permit_for(gate: &mut TransportGate) -> Result<HandoverPermit, ResourceError> {
    let (device, incarnation) = (gate.device(), gate.incarnation());
    gate.issue_handover_permit(
        drained(incarnation),
        &[],
        &super::tests::writer_coverage_for_tests(),
        RecipientReservation::new_for_tests(device, incarnation),
    )
}

/// census: S2-permit-reservation-identity transport.rs issue_handover_permit `reservation.device != self.device || reservation.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_handover_permit_refuses_a_reservation_for_another_recipient() {
    let incarnation = IncarnationId::first();
    let other_device = DrmDeviceKey {
        major: 226,
        minor: 1,
    };
    for reservation in [
        RecipientReservation::new_for_tests(other_device, incarnation),
        RecipientReservation::new_for_tests(handover_device(), incarnation.next()),
    ] {
        let mut gate = quiescing_gate(handover_device(), incarnation);
        let result = gate.issue_handover_permit(
            drained(incarnation),
            &[],
            &super::tests::writer_coverage_for_tests(),
            reservation,
        );
        assert!(
            matches!(result, Err(ResourceError::WrongIncarnation)),
            "a reservation for another recipient must be refused, got {result:?} [census:S2-permit-reservation-identity]"
        );
    }
}

```

- [ ] **Step 3: Run the tests**

Run: `cargo +nightly fmt && cargo test -p yserver --lib transport_gate`, then `cargo test -p yserver --lib c0_2ci_guard_handover_permit_refuses_a_reservation`
Expected: every existing `transport_gate` test still passes; the new test passes. A failure is an F8 stop.

- [ ] **Step 4: Run the oracle**

Run: `python3 tools/guard-census.py --files transport.rs --fn issue_handover_permit --deterministic-only --require-oracle`
Expected: `S2-permit-reservation-identity` `CAUGHT_BY_ORACLE`; exit 0.

- [ ] **Step 5: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; `c0_2ci` 173 passed, 18 ignored.

```bash
git add crates/yserver/src/kms/render/resources/transport.rs crates/yserver/src/kms/render/resources/handoff.rs crates/yserver/src/kms/render/resources/tests.rs crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "fix(kms): bind handover evidence to its recipient and to every writer class

Stage 2c-i debt, spec 4.4. WriterCoverageProof's test constructor took
no evidence and RecipientReservation carried no identity, so tests on
them could certify a handover the contract forbids. The proof's test
constructor now consumes explicit coverage for every WriterClass, and
the reservation names its recipient's device and incarnation, which
issue_handover_permit now checks. Production issuers stay absent (R8).

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 5: Family A and `consume_owner_write` (spec 4.4, second half)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: Task 4's `handover_device`, `quiescing_gate`, `permit_for`; `TransportGate::{for_tests, force_close, publish_owner, authorize_owner_write, consume_owner_write, set_outstanding_owner_writes_for_tests, outstanding_owner_writes}`.

- [ ] **Step 1: Append the tests**

Extract this task's `rust` block 1 to `/tmp/s2-task5.rs` and append it to `guard_tests.rs`.

```rust
/// census: A-permit-quiescing transport.rs issue_handover_permit `self.state() != TransportState::Quiescing`
#[test]
fn c0_2ci_guard_handover_permit_refuses_outside_quiescing() {
    let mut gate = TransportGate::for_tests(handover_device(), IncarnationId::first());
    let result = permit_for(&mut gate);
    assert!(
        matches!(result, Err(ResourceError::Busy)),
        "a permit must not be issued from Legacy, got {result:?} [census:A-permit-quiescing]"
    );
}

/// No public transition reaches `Quiescing` with a grant outstanding --
/// `begin_quiescing` refuses one -- so the test setter builds the state the
/// guard exists to refuse.
/// census: A-permit-outstanding transport.rs issue_handover_permit `self.outstanding_owner_writes != 0`
#[test]
fn c0_2ci_guard_handover_permit_refuses_with_an_outstanding_grant() {
    let mut gate = quiescing_gate(handover_device(), IncarnationId::first());
    gate.set_outstanding_owner_writes_for_tests(1);
    let result = permit_for(&mut gate);
    assert!(
        matches!(result, Err(ResourceError::Busy)),
        "a permit must not be issued while an owner write grant is outstanding, got {result:?} [census:A-permit-outstanding]"
    );
}

/// census: A-publish-identity transport.rs publish_owner `permit.device != self.device || permit.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_publish_owner_refuses_a_permit_for_another_incarnation() {
    let incarnation = IncarnationId::first();
    let mut gate = quiescing_gate(handover_device(), incarnation);
    let mut other = quiescing_gate(handover_device(), incarnation.next());
    let foreign = permit_for(&mut other).unwrap();
    assert_eq!(
        gate.publish_owner(foreign),
        Err(ResourceError::WrongIncarnation),
        "a permit issued for another incarnation must be refused [census:A-publish-identity]"
    );
    assert_eq!(gate.state(), TransportState::Quiescing);
}

/// census: A-publish-quiescing transport.rs publish_owner `self.state() != TransportState::Quiescing`
#[test]
fn c0_2ci_guard_publish_owner_refuses_once_the_gate_left_quiescing() {
    let mut gate = quiescing_gate(handover_device(), IncarnationId::first());
    let permit = permit_for(&mut gate).unwrap();
    gate.force_close();
    assert_eq!(
        gate.publish_owner(permit),
        Err(ResourceError::Busy),
        "a permit must not publish Owner once the gate left Quiescing [census:A-publish-quiescing]"
    );
}

/// As for the permit: only the test setter reaches `Quiescing` with a grant
/// outstanding.
/// census: A-publish-outstanding transport.rs publish_owner `self.outstanding_owner_writes != 0`
#[test]
fn c0_2ci_guard_publish_owner_refuses_with_an_outstanding_grant() {
    let mut gate = quiescing_gate(handover_device(), IncarnationId::first());
    let permit = permit_for(&mut gate).unwrap();
    gate.set_outstanding_owner_writes_for_tests(1);
    assert_eq!(
        gate.publish_owner(permit),
        Err(ResourceError::Busy),
        "Owner must not be published while an owner write grant is outstanding [census:A-publish-outstanding]"
    );
    assert_eq!(gate.state(), TransportState::Quiescing);
}

/// census: B-consume-owner-write-state transport.rs consume_owner_write `self.state() != TransportState::Owner`
#[test]
fn c0_2ci_guard_consume_owner_write_refuses_once_the_gate_left_owner() {
    let mut gate = quiescing_gate(handover_device(), IncarnationId::first());
    let permit = permit_for(&mut gate).unwrap();
    gate.publish_owner(permit).unwrap();
    let grant = gate.authorize_owner_write(WriterClass::Primary).unwrap();
    gate.force_close();
    let result = gate.consume_owner_write(grant);
    assert!(
        matches!(result, Err((ResourceError::Detached, _))),
        "a grant issued in Owner must not be consumed once the gate left Owner [census:B-consume-owner-write-state]"
    );
    assert_eq!(gate.outstanding_owner_writes(), 1);
}

/// Round-2 B-1, Owner side: the same close, arriving through the service's
/// handle, must also stop a permit being issued or published and a grant
/// being minted or consumed. Task 2 proved the `begin_quiescing` half; these
/// need Task 4's handover evidence to reach Owner at all.
#[test]
fn c0_2ci_service_driven_close_is_terminal_for_owner_transitions() {
    // A permit in hand before the close cannot publish Owner after it, and
    // the closed gate issues no further permit.
    let (service, _held, _obligation, mut gate) = submission_fixture();
    gate.begin_quiescing().unwrap();
    let permit = permit_for(&mut gate).unwrap();
    service.close_transport_gate();
    assert_eq!(
        gate.publish_owner(permit),
        Err(ResourceError::Busy),
        "a transport closed through its handle must refuse to publish Owner"
    );
    assert!(
        matches!(permit_for(&mut gate), Err(ResourceError::Busy)),
        "a transport closed through its handle must issue no handover permit"
    );

    // And in Owner: no new grant, and no consumption of one taken earlier.
    let (service, _held2, _obligation2, mut owner) = submission_fixture();
    owner.begin_quiescing().unwrap();
    let permit = permit_for(&mut owner).unwrap();
    owner.publish_owner(permit).unwrap();
    let grant = owner.authorize_owner_write(WriterClass::Primary).unwrap();
    service.close_transport_gate();
    assert!(
        matches!(
            owner.authorize_owner_write(WriterClass::Primary),
            Err(ResourceError::Detached)
        ),
        "a transport closed through its handle must mint no owner write grant"
    );
    assert!(
        matches!(
            owner.consume_owner_write(grant),
            Err((ResourceError::Detached, _))
        ),
        "a transport closed through its handle must consume no owner write grant"
    );
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo +nightly fmt && cargo test -p yserver --lib c0_2ci_guard_`
Expected: every `c0_2ci_guard_` test passes. A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `python3 tools/guard-census.py --files transport.rs --deterministic-only --require-oracle`
Expected: eight tagged sites, all `CAUGHT_BY_ORACLE` — `B-authorize-write-closed`, `B-consume-owner-write-state`, `A-permit-quiescing`, `A-permit-outstanding`, `S2-permit-reservation-identity`, `A-publish-identity`, `A-publish-quiescing`, `A-publish-outstanding`; exit 0.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: clean; `c0_2ci` 180 passed, 18 ignored.

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove the Owner handover refusals on bound evidence

Stage 2c-i debt, family A: issue_handover_permit refuses outside
Quiescing and with a grant outstanding; publish_owner refuses a permit
for another incarnation, outside Quiescing and with a grant
outstanding. Also consume_owner_write's refusal once the gate has left
Owner, which session 1 moved here.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 6: Session acceptance — full census, hardware gate, record

**Files:**
- Create ([H], coordinator): `docs/superpowers/findings/2026-09-17-stage-2c-i-debt-census-session-2.md`
- Modify ([H], coordinator): `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (status line)

- [ ] **Step 1: Implementer's final checks, then hand off**

Run and keep the output for the coordinator:
- `cargo +nightly fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo clippy -p yserver --all-targets --features tcp-transport -- -D warnings`
- `cargo clippy -p yserver --all-targets --features xdmcp -- -D warnings`
- `cargo check -p yserver --target x86_64-unknown-linux-musl`
- `cargo check -p yserver --target x86_64-unknown-freebsd`
- `for i in $(seq 1 12); do cargo test -p yserver --lib c0_2ci 2>&1 | grep "test result"; done`
- `cargo test --workspace`
- `python3 tools/guard-census.py --deterministic-only --require-oracle` — 39 tagged sites, all `CAUGHT_BY_ORACLE`, exit 0.
- `python3 tools/guard-census.py --files drm_cleanup.rs --deterministic-only --require-oracle` — 3 tagged sites, all `CAUGHT_BY_ORACLE`, exit 0.

Expected: all clean; twelve identical `c0_2ci` results with zero failures. **Stop and hand off.**

- [ ] **Step 2 [H]: Full acceptance census**

Run: `python3 tools/guard-census.py --require-oracle --json /tmp/census-session-2.json` (about an hour; 71 sites)
Expected: `CAUGHT_BY_ORACLE` 39, `CAUGHT` 32, `SURVIVES` 0; no `CAUGHT_NOT_BY_ORACLE`, `CAUGHT_WHOLE_BODY`, `ORPHAN_TAG` or `A_MANO`; exit 0.

- [ ] **Step 3 [H]: Hardware gate and reviewer mutations**

Run: `cargo test -p yserver --lib c0_2ci -- --ignored`. Expected: 18 passed — Task 1 changed `ScanoutBo`'s device alias and Task 2 the compositor's error type, which these exercise. Then run every *Reviewer mutation* of Tasks 1–3, each confirmed to have compiled, and record which test failed.

- [ ] **Step 4 [H]: Record and commit**

Create `docs/superpowers/findings/2026-09-17-stage-2c-i-debt-census-session-2.md` with: the census summary and a table of site, verdict, bound test and strategy; the `drm_cleanup.rs` oracle result; each reviewer mutation and the test that caught it, and the arms checked by reading; the plan corrections, the 4.3 F8 stop and any others found while executing; and both gate transcripts. In the spec's status line add: "Session 2 executed: 71 census sites, zero survivors; 4.2's real-path acceptance half and 4.3's generation-replacement half left open, recorded; see `…-census-session-2.md`." Commit with the coordinator's trailer.

## Provenance

The coordinating session prototyped every block above on `12a32854`, then restored the tree. On the prototype: `cargo test -p yserver --lib c0_2ci` 179 passed, 18 ignored; the hardware run 18 passed; `cargo clippy --all-targets -- -D warnings` clean; `cargo +nightly fmt --check` clean; `cargo check` clean for musl and FreeBSD; `guard-census.py --deterministic-only --require-oracle` gave `CAUGHT_BY_ORACLE` for all 39 tagged sites in the default files and all 3 in `drm_cleanup.rs`; and every *Reviewer mutation* named above that the prototype could run was run: the reset test failed under Task 3's `can_destroy` and `destroy_now` mutations, and Task 2's two round-2 mutations each failed their named test. The diffs were cut from the prototype per task and re-applied in order with `patch` to a clean export of `12a32854`, reproducing every prototype file byte for byte.
