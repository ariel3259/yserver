# Stage 2c-i debt — census baseline (session 1, Task 1 steps 4–6)

**Run by:** the coordinating session (Opus), which has GPU and DRM access, on
2026-09-16. These steps are [H] in the plan: codex's workspace-write sandbox has
no `/dev/dri` and no Vulkan ICD.

**Tool:** `tools/guard-census.py` at `e0f0e1c4`, written by codex
(gpt-5.6-luna, xhigh) and verified byte-identical to the plan's Task 1 code.

## Step 4 — fidelity: the tool reproduces the published census

```
tools/guard-census.py --legacy-enumeration --json census-legacy.json
=== SUMMARY ===
CAUGHT: 32
SURVIVES: 35
```

67 sites, no `A_MANO`, no other verdict. Matching totals alone could hide two
opposite errors, so the survivor set was also compared per (file, function)
against the hand census recorded in the spec's section 2.1 and the refusal
inventory:

| File | Function | Hand census | Tool |
| --- | --- | --- | --- |
| `commit.rs` | `is_resource_releasable` | 2 | 2 |
| `commit.rs` | `register_commit_dependencies` | 1 | 1 |
| `commit.rs` | `consume` | 2 | 2 |
| `commit.rs` | `on_available` | 2 | 2 |
| `gpu.rs` | `cancel_pre_submit_batch` | 1 | 1 |
| `gpu.rs` | `freeze_uncertain_batch` | 1 | 1 |
| `transport.rs` | `authorize_write` | 1 | 1 |
| `transport.rs` | `consume_owner_write` | 1 | 1 |
| `transport.rs` | `issue_handover_permit` | 2 | 2 |
| `transport.rs` | `publish_owner` | 3 | 3 |
| `mod.rs` | `adopt` | 1 | 1 |
| `mod.rs` | `adopt_unchecked` | 1 | 1 |
| `mod.rs` | `reserve` | 1 | 1 |
| `mod.rs` | `register` | 2 | 2 |
| `mod.rs` | `freeze` | 1 | 1 |
| `mod.rs` | `cancel` | 1 | 1 |
| `mod.rs` | `validate_proof_target` | 1 | 1 |
| `mod.rs` | `record_kms_discharged` | 1 | 1 |
| `mod.rs` | `apply_teardown_release` | 3 | 3 |
| `mod.rs` | `validate_gpu_batch` | 7 | 7 |
| | **Total** | **35** | **35** |

No difference. The six sites the hand census mutated by hand are resolved by the
tool's swallow strategies with the same verdicts. **The tool is faithful; later
tasks may rely on it.**

## Step 5 — what the legacy enumeration could not see

`diff <(… --legacy-enumeration --list) <(… --list)` adds exactly one site:

```
commit.rs consume `self.capacity.is_vacant(DirectRole::OrdinaryRetirement) && let Err(err) = self .capacity .move_role(role, DirectRole::OrdinaryRetirement)`
```

Its census (`--files commit.rs --fn consume`, full suite): **SURVIVES**.

It is the `} else if` branch of `consume`'s `CompletionRetired` handling: when no
retirement slot was pre-reserved for the commit and `OrdinaryRetirement` is
vacant, a failed move of the old `Current` into it is returned as an error with
admission closed. That is the same invariant, in the same function and path, as
family C's two `consume` guards — **an unproven sibling of family C**, the
pattern this stage exists to close. Per the plan it is reported here and not
added to session 1; its scope is the user's call.

A cosmetic note for anyone tagging it: the tool joins a multi-line method chain
with spaces, so its identity contains `self .capacity .move_role(…)`. The text is
stable, and a tag must reproduce it exactly.

## Step 6 — session 1's target

The 27 guards of Tasks 2–7, as amended into spec section 3.1 at `e0f0e1c4`. The
acceptance census (Task 8) runs the legacy enumeration, so the site above is
outside its count either way; if it is added to session 1, the plan and spec
must say so and Task 8's expectation becomes 28 proven and eight survivors.
