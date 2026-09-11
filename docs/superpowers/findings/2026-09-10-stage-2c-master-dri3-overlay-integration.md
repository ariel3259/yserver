# Stage 2c — master DRI3 and overlay integration, 2026-09-10

Merged upstream `a06cf0e00c5ba41431966732b71df802b7d0a51b` into feature HEAD
`14dd92d818e619357c903ad64e5357f47619111e` as **`67bf3491`**. No push.
This is a source integration and local contract comparison, not an external
adversarial verdict. The previous accepted design and first implementation-plan
review used the earlier baseline.

## Integrated changes

| Upstream commit | Change |
| --- | --- |
| `670e6637` | README updates |
| `64d4b6e4` | README path correction |
| `2248b352` | Composite overlay ownership moves to per-client core claims |
| `ecec0794` | Ignore picom output |
| `a06cf0e0` | Chrome hardware-video DRI3 client-buffer round-trip correction |

The Chrome correction is buffer identity/layout handling, not a new decoder.
`Dri3ImportModifier` distinguishes implicit legacy imports from explicit
modifiers. Imported export duplicates the original client dma-buf and preserves
stride/offset and the stated legacy size. The retained `DrawableImage` now
carries reported modifier, plane-0 description and optional client size;
`ImportedDmabufMetadata::implicit_layout` prevents a guessed linear view from
entering direct scanout. Window modifier advertisement remains single-plane.
Upstream explicitly retains a limitation: arbitrary server sampling of such
unresolved implicit layouts is not made correct by this client round-trip fix.

Overlay claims now live only in `ServerState::cow_claims`; backend get/release
operations are first/final claim edges. Protocol failure retains a caller's
claim; disconnect cleanup uses a sticky `cow_teardown_failed` latch when the
physical operation fails. Physical COW/fallback storage can remain retained
behind direct unflip without retaining a logical client claim.

## Conflict resolution and preserved work

- `crates/yserver-core/src/backend/mod.rs`: preserve local
  `PresentSequenceTarget` and upstream `Dri3ImportModifier` exports.
- `docs/status.md`: preserve both sets of status entries.
- Source/backend and core-loop changes otherwise merged automatically. No
  manual runtime behavior rewrite was needed beyond combining the export list.
- Before merging, eight local files were copied under
  `/tmp/yserver-a06cf0e0-integration-20260910/before/`. Git autostash `4f05774b`
  preserved tracked edits and applied cleanly after the merge commit.
  Untracked review/plan/inventory files remained present throughout.

## Required 2c updates

The resource design, adapter inventory and executable plan now explicitly retain
original imported FD/reporting metadata, preserve implicit direct rejection,
and keep overlay logical claims separate from physical leases. Plan Tasks 3,
7 and 8 carry the new tests and integration seams. The main 2c design adds the
same requirements for later producer/damage conversion. Neither earlier review
certifies these new requirements.

The [first plan review](2026-09-09-stage-2c-i-implementation-plan-review-round1.md)
reported 1 blocking, 2 major, 1 minor with complete declared coverage. Local
verification confirmed all four: GPU proof batching could partially mutate and
lose a retaining batch on early error; the consumer lacked capacity-token
completion routing; writer tests exercised only an enum; group membership did
not retain topology/CRTC generation. The plan now corrects those contracts and
adds their regression cases. Counts remain the original external verdict;
corrections have not received another external pass. No 2c-i code was executed.

## Validation

All commands were run on the resolved source before committing; subsequent
autostash restoration and contract edits changed documentation only.

| Check | Result |
| --- | --- |
| `cargo +nightly fmt` and format check | Passed |
| `cargo clippy --all-targets -- -D warnings` | Passed |
| `cargo test --all-targets --locked` | 3,211 passed, 0 failed, 220 ignored across 15 binaries |
| `cargo check -p yserver --target x86_64-unknown-linux-musl` | Passed |
| `cargo check -p yserver --target x86_64-unknown-freebsd` | Passed |
| `dri3_imported_pixmap_exports_the_clients_own_description --ignored --nocapture` | Sandbox attempt skipped for Vulkan initialization failure; outside sandbox ran and passed, 1 test |
| Documentation/merge whitespace and conflict-marker checks | Passed |

Logs: `/tmp/yserver-a06cf0e0-integration-20260910/`. Live Chrome playback,
compositor restart under actual scanout and cross-platform runtime behavior were
not exercised. Upstream's own hardware observations are not counted as local
validation. Production remains Legacy; this merge does not activate C0 Owner.
