# Stage 2c integration with upstream v1.5.0

Date: 2026-09-09. Local merge/source comparison; not an adversarial review.

## Inputs

- Feature parent: `d4c30877cf311e96a96b2e51f5e3fefb19c67536`.
- Upstream parent: `99d02b16114f1366397f550626edfb416f6f5aac`.
- Release: v1.5.0, `e2d17ec523e9202d8f0db5a72edad5eebb416803`.
- Previous imported master: `f6c79967`.
- Preserved local work: round-1 adversarial report, B-1/B-2 corrections,
  M-2 transport contract, and unchanged no-new-Present-credit decision.

The eight upstream commits add SHAPE unset-clip preservation, server-side
borders, root-source Picture IncludeInferiors, dependency/release updates and
tools/docs. Only the test import list in `kms/render/backend.rs` conflicted.
Resolution preserves sequence test imports and upstream snapshot predicates;
the old free-function import of `resolve_picture_for_render` is removed because
master made it a backend method. Other overlapping core/backend files merged
automatically and require the integration checks recorded in `docs/status.md`.

## Design changes incorporated

| Upstream contract | Consequence for 2c |
| --- | --- |
| `Drawable::content_offset` belongs to actual pixel layout | Allocation leases capture layout identity; live window geometry cannot reinterpret old pixels. |
| `decref` only detaches the XID if it still maps to that drawable | Old leased storage retirement cannot orphan its replacement. |
| Border migration can copy into new storage or relocate in place | Layout mutation must respect outstanding KMS/read leases; a new generation number alone is insufficient. |
| Typed `PaintTarget`, `Src`, `Dst` carry content bounds | Producer conversion retains bounds/offsets, with explicit server-backing access and request-specific read rules. |
| Direct excludes any resolved border clip | Initial and retirement-promoted admission preserve ancestor-border rejection and revalidate queued geometry. |
| Root Picture IncludeInferiors snapshots OnScreenOnly before Composite | Keep read/scratch GPU lifetimes separate from KMS completion and damage ack; preserve transforms, fallback and one-site scratch cleanup. |
| Canonical scene-copy design is present but not implemented | Do not assume a canonical allocation exists or expand 2c to implement it. |

The main 2c document now assigns these requirements to all three blocks with
named source sites and regression scenarios. The 2c-i resource design adds
layout/read lifetime constraints. Six physical direct positions and the
supervisor handoff remain the chosen local corrections; their previous
adversarial verdict is not retroactively upgraded by this merge.

## Required regression coverage in implementation

Preserve upstream border/SHAPE/render acceptance tests and combine them with
owner evidence permutations. Explicitly test border mutation while submitted
or successor-held, old-XID retirement after reallocation, ancestor borders,
named pixmap versus window read bounds, and root snapshot during direct/unflip.
Zero-area Composite must not allocate a snapshot, and failure after snapshot
creation must still release it once through fence-aware cleanup.

No hardware capture or new adversarial pass is part of this integration.
Fresh merge verification logs are under `/tmp/yserver-v150-integration-20260909/`.
