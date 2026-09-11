## Verdict

**0 blocking, 0 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

Reviewed HEAD: `14dd92d818e619357c903ad64e5357f47619111e`, with local
working-tree design corrections. Explicit user authorization preceded this pass.
Observed reviewer usage: **54,280 tokens**, excluding author work.
Raw log: `/tmp/yserver-stage2ci-review-round3.log`.
Input copies: `/tmp/yserver-stage2ci-review-round3-inputs/`.

| Reviewed input | SHA256 before status-only updates |
| --- | --- |
| Resource terminalization design | `932d8725be844cc3c8b0e93bda12dc0a8165694a9b3ef62144b3a8f03eb517c2` |
| Resource adapter inventory | `14b6814ec896ccce5a6794055831e7759798fbc1a7fcb076e151e1d612c33ab5` |
| Main 2c design, M-2 context | `9d6638ee41aefbb51f49540deff3454d2dfd6d84f513b7d1f1f8cc2b3e0a8dfa` |


Within the bounded review, the revised design is ready to serve as the basis for writing the 2c-i implementation plan. This does not approve implementation, establish compilability, or authorize production activation.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| Round-2 B-1 — availability ownership and completion handoff | **APPLIED** | One authoritative per-incarnation/allocation-generation resource-service entry now owns reservations, KMS obligations, GPU/read tickets, FOREIGN state, subscriptions, and backing context. Producers route generation-tagged evidence through one inbox; acquisition and waiter registration use the same serialized path; teardown atomically transfers entries, inbox, pending evidence, and wakes to the sole supervisor recipient ([target lines 149–210](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:149), [191–200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:191)). The inventory identifies concrete acquisition, completion, polling, replacement, and teardown adapters ([inventory lines 29–74](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md:29)). |
| Round-2 M-1 — regression coverage | **APPLIED** | The matrix now covers relayout exclusion, old-generation XID safety, both KMS/GPU completion orders, synchronous snapshot versus scratch lifetime, pending/uncertain reads across handoff, stale/duplicate completion, lost-wake avoidance, and immediate retirement progress ([target lines 445–483](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:445)). The inventory additionally requires completion-only progress while VT/DPMS suppress composition and old-generation promotion/pool-return coverage ([inventory lines 76–82](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md:76)). |
| Round-2 m-1 — original X11 depth | **APPLIED** | Deferred pixel interpretation must retain `PaintTarget::x11_depth` or the complete typed target, independently of backing depth, and the regression matrix includes depth-24 over depth-32 redirected storage ([target lines 60–70](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:60), [456–458](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:456)). |
| Previously unconfirmed M-2 — transport exclusion | **APPLIED** | The companion design establishes a single per-device transport state, enumerates all writer classes, revokes legacy admission before draining, and permits `Owner` only after every writer is mediated or disabled and the teardown receiver is installed. Failure/unknown closes transport with no same-incarnation legacy return ([2c design lines 247–278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:247)). Production remains `Legacy` until stages 3/4 provide the receiver and remaining conversions. |
| Inherited B-2 — bounded physical resources | **APPLIED** | The accepted six roles remain explicit; reservation precedes import, both retirement roles remain charged until proof, and failure closes transport instead of allocating overflow ([target lines 341–414](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:341)). |
| Inherited Skip-metadata bound | **NOT APPLIED** | The metadata remains structurally unbounded and is accurately disclosed ([target lines 429–441](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:429)). Under the explicitly accepted policy, this is not a 2c-i readiness defect and the design correctly makes no total Present-memory claim. |

## Findings

### Blocking

None.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation audit:** Assessed all three round-2 findings, the previously unconfirmed M-2 gate, and the two inherited dispositions material to this design.
- **Architecture and cross-task contracts:** Confirmed authoritative availability ownership, producer-to-consumer completion routing, serialized reservation/recheck, physical-role ownership, atomic teardown transfer, and the production activation boundary. These align with the spec’s bounded primary model ([spec lines 1393–1428](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1393)).
- **Safety, ownership, and failure semantics:** Checked generation correlation, duplicate/stale evidence, cancellation, unknown submission, read/GPU/KMS/FOREIGN conjunctions, fd-family limitations, late completion after backend detachment, and process-exit retention. This matches the independent retirement milestones ([spec lines 2079–2114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2079)) and resource-specific teardown barriers ([spec lines 1941–2031](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1941)).
- **Spec compliance and verification:** Present completion remains separate from release ([spec lines 2212–2239](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2212)); direct conversion preserves bounded successor and retirement semantics ([spec lines 2338–2368](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2338)). Required tests observe real adapter routing and allocation/destruction behavior rather than only synthetic membership.

Used **12/12 bounded excerpts** beyond the target and prior review: inventory 1, M-2 companion 1, authoritative spec 7, source 3. Target and prior review were each read once; the target’s tool-truncated middle was completed without rereading overlapping text.

Verified baseline ground included core-loop completion servicing independent of composition, the currently composition-local scanout completion drain, wakeup gating, and nonblocking fence-status behavior. Exhaustive converted-call-site coverage, undiscovered raw-handle escapes, exact Rust APIs, buildability, formatting, clippy, portability, and executable test results remain unassessed implementation work—not implicitly sound.

## Author disposition — 2026-09-09

The pass completed successfully. No new finding requires a design correction.
The result covers readiness to write the 2c-i implementation plan and accepts
the prior unavailable M-2 coverage within this declared scope. It is not a
clean review of all 2c-ii/2c-iii behavior or a completed implementation audit.

After recording the result, only review-status text in the three design inputs
and `docs/status.md` was updated; resource contracts were not changed. Earlier
rounds retain their original verdicts and coverage. Documentation validation:
`git diff --check` and whitespace checks for new documents. No code, build,
clippy, hardware validation, commit or production activation occurred in this
pass. The next deliverable is the executable 2c-i plan with concrete task
interfaces, tests and normal implementation/CI gates.
