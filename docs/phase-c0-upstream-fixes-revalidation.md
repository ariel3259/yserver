# Phase C.0 — upstream fixes to re-validate after stage 4

**Why this file exists (user, 2026-09-21).** While Phase C.0 is developed on
`feat/phase-c0-atomic-kms-migration`, upstream `joske/master` keeps fixing
issues (1.5.1 shipped on 2026-09-12 and more fixes followed). Every upstream
commit is merged into this branch, and the merge is validated by the branch's
software and hardware gates. **That is not evidence the fix still holds.** Many
fixes live in code the C.0 conversion replaces on the Owner route — the direct
scanout producer, the composed flip, the damage acknowledgement, the M1 probe
cache, topology/hotplug — and the behaviour a fix relied on may no longer exist
once production runs on that route. So, **after stage 4 switches production to
`Owner`**, each fix below is re-validated on this branch, on the real desktop,
with its original reproduction where one exists. This file is the checklist;
`docs/status.md` records the outcomes.

Rules for keeping it current:

- Every upstream merge adds its fix commits here, classified by exposure.
- **Exposure** says whether the fixed behaviour depends on the route C.0 converts:
  **high** — lives in a producer, the damage transaction, the M1 cache, unflip,
  topology or cursor/gamma writers (stages 2c/3/4 replace it); **medium** —
  touches shared state those routes read (store layout metadata, imports);
  **low** — render, GLX, XKB, DIX, input, protocol paths that never reach KMS.
- Low-exposure fixes are re-run once as a smoke; high and medium ones get an
  explicit A/B check against the Owner route, with the reproduction named here.
- A fix whose reproduction no longer applies (the code path is gone) is marked
  **superseded** with the reason, never silently dropped.

## Fixes merged from upstream since the C.0 branch (v1.5.0 → master)

| Upstream commit | Issue | What it fixes | Exposure | Re-validation after stage 4 |
| --- | --- | --- | --- | --- |
| `fc76b743` | #129 | no-vsync Present pacing; fullscreen direct scanout enabled | **high** — the direct producer is Cii's; pacing is `Presented`-driven on Owner | fullscreen game + `glxgears`-style pacing probe on Owner; compare Present completion cadence with Legacy (`tools/present-pacing-*` if present, else the #129 reproduction) |
| `c09358a1` | — | hide cursor planes before VT suspend | **high** — C.0 §12 names it a mandatory conversion site (stage 3/4: atomic detach in the VT request) | VT switch away/back with a hardware cursor on Owner; no stale cursor plane, no flash |
| `8d448583` | — | damage repaint follow-ups (`owes_repaint`, dormancy, snapshots) | **high** — the damage transaction moved to owner milestones in Ci (spec 2c §5 carried them) | Ci's `c0_conv_ci_*` scene-contract tests are the software half; on the desktop: invalidated output repaints without new paint, skipped-output dormancy |
| `a9a3fcf3` | — | padded linear DMA-BUF imports restored | **medium** — import metadata feeds the M1 probe and Cfb's adoption | Chrome/mpv hardware video on Owner direct scanout; `tools/chromium-yserver-*.sh` |
| `2248b352` | — | composite overlay ownership | **high** — overlay claims interact with unflip/direct (spec 2c baseline addendum: 0→1/1→0 overlay edges) | overlay window mapped/unmapped while a fullscreen client is direct on Owner |
| `a06cf0e0` | #138, #139 | scrambled hardware-decoded video in Chrome (DRI3 implicit layout) | **medium** — M1 refuses implicit layouts; Cfb adoption keeps that refusal | Chrome hardware video on Owner: no scramble, and an implicit-layout pixmap never reaches direct scanout |
| `6ece3e66` | #137 | Java AWT/Swing text invisible | low | smoke: a Swing app renders text |
| `d8f726a7` | — | instance buffer pin in the frame-pin ceiling | low | covered by render tests |
| `afa87ef7` | — | subpixel-antialiased text as solid blocks | low | smoke: subpixel text in a GTK app |
| `81b0cd1a` | #142 | restore window content on unredirect; background-None not painted white | **high** — unredirect/redirect changes composed damage and the scene walk that Ci converted | redirect/unredirect a window under a compositor on Owner; content restored, no white flash |
| `788d5f51` | #141, #144 | XI2 selection absorbs the core press | low | input smoke |
| `236cc1f2` | #147 | direct-scanout diagnostics no longer flood INFO | **medium** — the Owner direct path logs from `direct_owner.rs`, not the legacy sites | run a direct-scanout session at INFO on Owner; log volume comparable |
| `4d3244f6` | #151 | XKB GetMap advertises an empty KeyBehaviors section | low | `tools/xkb-behaviors-probe.c` |
| `bb780617` | #143 | BadPixmap/BadGC/BadCursor for unresolvable XIDs | low | DIX smoke |
| `369192d4` | #143 | rotate a redirected backing when the window shrinks | **high** — redirected backing relayout bumps Cfb's backing serial and invalidates M1 entries | `tools/composite-shrink-probe.c` on Owner |
| `25a69edb` | — | depth-24 destination stays opaque across RENDER composites | low | render smoke |
| `5d7270c1`, `c6d6f0d7` | #143 | border ring reported as protocol damage (and on resize) | **medium** — damage reporting feeds the transaction Ci owns | `tools/border-damage-probe.c` on Owner |
| `afa0c4aa`, `b2269566` | #143 | background-None content kept across resize; expose on shrink | **medium** — resize relayout is a storage replacement (Cfb serial) | `tools/resize-expose-probe.c`, `tools/vng-scenarios/resize-expose.sh` on Owner |
| `21d23fbf`, `b0728f88` | — | borders preserved across redirected copies; redirected backing geometry | low/medium | `tools/border-damage-probe.c` |
| `94ee9914` | — | read the **flipped source**, not the pool, while direct scanout is up | **high** — on Owner the "current direct frame" is the ledger's `Current`, not `scanout_m2`'s legacy field; GetImage/readback must read the same buffer | GetImage of the root while a client is direct on Owner returns the client's pixels; `tools/direct-scanout-*` probe if present |
| `2ea40635` | #152, #157 | QtWebEngine native-pixmap GLX config | low | GLX smoke |
| `2d476303` | #158 | RANDR CRTC model, hotplug relight | **high** — topology installation is stage 3's; the CRTC model feeds Ciii-identity's `CommitKey` and Cfb's index key | hotplug/unplug and DPMS relight on Owner; outputs relight, no stale index or commit correlation |
| `61fec18a` | #159 | stencil-free glmark visual | low | `glmark2` smoke |
| `00211310` | #162 | GC clip honoured in MIT-SHM PutImage | low | SHM PutImage smoke |
| `0b1bb820` | #163 | direct scanout revoked on top-level restack; covered fullscreen kept out; depth-32 kept | **high** — folded into the shared predicate `direct_present_eligibility` (both routes), but the revocation runs through `request_direct_unflip`, which on Owner is the unflip producer of Ciii | restack a top-level over a direct fullscreen window on Owner: unflip happens, later Presents compose; depth-32 fullscreen stays direct |
| `dc753c13` | — | direct scanout kept on a restack beneath the composite overlay (a compositing desktop no longer unflips on every raise; Cinnamon showed stale frames) | **high** — merged with a deliberate difference on the Owner route: the unflip takes the COW exemption, but every real order change still advances the conductor's layout generation, so on Owner a restack under the COW **withdraws a queued direct successor** (a skipped frame, never a stale one) where Legacy keeps it | Cinnamon (or any compositing desktop) with a fullscreen direct client on Owner: raises, tooltips and notifications cause no unflip and no stale frame; count successor withdrawals against Legacy — if they cost visible frames, scope the layout note to changes the COW does not cover |
| `a15fdcb1` | — | compose timestamps not read before the pool's queries were first written ("query not reset") | **medium** — the compose path is shared; the merge extended the flag to the two Owner compose targets upstream did not have (`ManagedCopiedComposeTarget`, `ManagedSharedComposeTarget`), and the shared backing moves the flag with its `TransferResources` | a validation-layer session (`just` validation recipe) on Owner composed and copied routes: no "query not reset" on the first compose of a new pool |
| `f56dcf20`, `14391bc0`, `b757e253` | — | TFP export: unadvertised export reported as unsupported; tiling chosen by asking the driver; export semaphore kept alive until its submit retires | low — GLX texture-from-pixmap, no KMS route | GLX TFP smoke (a compositing WM on NVIDIA and on amdgpu) |
| `36405c09` | — | a depth-24 child stays opaque in its depth-32 parent's backing | low — render | render smoke |
| `16581ab3` | — | telemetry: VRAM, per-GPU engine load, pixmap-pool residency (`drm/fdinfo.rs`, `kms/vk/vram.rs`) | low — observability; reads fdinfo | none beyond the telemetry output existing on Owner |
| `2282c94f` | — | telemetry: VRAM accounted by use; every `vkAllocateMemory`/`vkFreeMemory` goes through `kms/vk/mem_accounting` (clippy disallows the raw calls) | **medium** — the merge routed three raw `free_memory` calls of C.0's resource service (`resources/scanout.rs` ×2, `resources/storage.rs`) through the ledger, and the managed-storage promotion now recategorises the exportable memory from either backing (`engine.rs`); a missed free would leave phantom ledger entries, not a leak | on Owner, the `vram by use` line's `untracked` stays flat across a composed/direct/unflip cycle and a TFP promotion |
| `fc0917be` | #167 | protocol: BIG-REQUESTS advertises and accepts Xorg's `MAX_BIG_REQUEST_SIZE` (4194303 units), not 256K | **none** — protocol framing, identical on Legacy and Owner; the merge also moved 3b-ii's gate-expiry stateless check to the same constant | no Owner-specific revalidation; the protocol tests cover it |
| `69766606`, `baa57e8e`, `d6c034a9`, `aae171cf`, `6b347f2e` | — | window storage only while viewable: `realize_window_storage`/`release_window_storage` allocate and free a window's backing on viewability; Pictures follow their window; copies/presents with an unviewable window behave as Xorg | **high** — releasing the storage of the current direct frame's source window goes through `request_direct_unflip`, which on Owner is Ciii's unflip producer; the merge put C.0's admission layout notes on the new sites (`allocate_window_leaf`, `release_window_storage`, and the geometry registration that replaced storage allocation) | unmap/iconify a window that is the direct frame's source on Owner: ordinary unflip, image alive until it retires, then freed (`c0_merge_unmap_direct_window_unflips_vulkan` is the software half); a compositing desktop map/unmap churn with a fullscreen direct client: no stale frame, no leaked backing (`export holders` report flat) |
| `2405ced8`, `1643b435`, `4b4677bc` | — | composite: a resized redirected window, or an app exiting without DestroyWindow, frees its old backing; every window-pixmap name returns its own reference | **medium** — redirected backing replacement bumps Cfb's backing serial and M1 entries | `tools/composite-shrink-probe.c` on Owner; a redirected app killed mid-session leaves no backing (`export holders`) |
| `ffc9fbdb`, `0ebcef48` | — | telemetry: list what holds every exported backing; root-readback warning paced | low — observability | none beyond the report existing on Owner |
| `ad71461c` | #172 | XKB: ChangeKeyboardMapping as Xorg | low — XKB | its Xorg-golden tests |
| `76d541d0`, `1e7680b6`, `3ccd665c`, `d9e11922` | — | `just rendercheck-yserver-hw` runs on this machine's KMS | none — tooling | — |
| `d5db7ccb` | #171, #174 | XKB: ChangeKeyboardMapping and SetModifierMapping edit and reinstall the real keymap (xmodmap reaches XKB clients and key cooking), Xorg's event order and per-key repeat | low — XKB/input, no KMS route | its Xorg-golden tests; smoke: `xmodmap` caps-as-control on Owner |
| `d18921a4` | #171, #175 | XKB: xkbcomp uploads reach the keymap; an `XkbDesc` model (`kms/xkb_desc/`) backs every XKB/core readback | low — XKB/input, no KMS route; the merge dropped the branch's copy of `MappingEdit`, which upstream removed | its Xorg-golden tests (`tools/xkb-mutation-goldens.sh`); smoke: `xkbcomp` upload on Owner |
| `b2b46162` | #173, #176 | XI2: `XI_RawKeyPress`/`XI_RawKeyRelease` delivered | low — input | `tools/vng-scenarios/xi2-raw-keys.sh` |
| `d746bc3c` | #178 | extension audit: GLX 1.0 pixmaps, XI warp/focus, XFIXES cursor, SYNC await/alarms, XTEST relative motion, byte order | **medium** — SYNC await suspends clients in the core's request queue; the merge folded that suspension (`client_runnable`) into 3b-ii's RANDR-gate runnable checks (`has_runnable_with_gate`, `pop_front_runnable`); the XFIXES displayed-cursor report sits next to the Owner cursor path | a client blocked in SYNC `Await` while a RANDR mutation holds the gate on Owner: neither starves the other; `tools/sync-await-probe.c`, `tools/xfixes-probe.c` on Owner |
| `06a71042` | #177 | the paint frame closes on its timeout while DPMS-off / VT-away; the dark returns drain submit telemetry | **high** — `maybe_composite`'s gates differ on Owner (`scene_composition_enabled()` replaces the `kms_outputs_active` gate); the merge kept the timeout close ahead of every gate and the telemetry drain on both dark returns | with DPMS off and with the VT away on Owner, a drawing client: no live-allocation/VRAM sawtooth (`vram by use` flat), frames close on their timeout |
| `f6f2d2c5` | #177 | no Present vblank arm against a CRTC DPMS powered off | **medium** — gated on `kms_outputs_active`; the Owner route's DPMS projection and Owner clock arms are separate, so the fix may not cover Owner arms | DPMS off on Owner with a Present client: no `CRTC_QUEUE_SEQUENCE` EINVAL storm, parked waits re-arm at wake |
| `fcbc89aa`, `57de7104`, `5656235c` | #177 | per-request upload buffers from a size-classed pool, then per-frame arena blocks (`render/upload_arena.rs`); per-site `VkDeviceMemory` churn telemetry | low — render, no KMS route | `vram churn [1s]` on Owner with a scrolling terminal: glyph-run allocations near zero |
| `c208ade5` | #180, #183 | RECORD 1.13 extension: device events, client start/death, context lifecycle (pynput and other global input listeners) | low — core protocol plus one hook in the KMS key-input path (`on_host_input` reports device events to RECORD, autorepeat as a flagged press), shared by Legacy and Owner | `tools/record-probe.c` / `tools/vng-scenarios/record.sh` on Owner; smoke: a pynput key listener sees presses, repeats and releases |
| `75f15a8d` | — | list RECORD among the implemented extensions | low — documentation only; no KMS route behavior | check the extension list against the RECORD query result |
| `906c2442` | #184 | reuse host-cached readback storage and a one-shot copy operation to lower CPU use during screen capture | **medium** — shared pool readback and managed Shared allocations use this path on either route; the direct-source lookup still uses Legacy `scanout_m2`, so Owner direct-current readback is not demonstrated by the merge | run continuous `x11vnc` capture and compare CPU use; while Owner direct scanout is active, probe root `GetImage` against the currently displayed client |
| `35b7d520` | — | update the dependency lockfile | none — dependency resolution only; no route-specific behavior | `cargo build --bin yserver` |
| `1867e2ab` | — | document Xorg's slow-client and write-error handling for fanout | none — documentation only | compare fanout behavior with the documented Xorg cases |
| `c8550b23` | — | make the upload-arena reuse test wait for frame A's fence before reuse | low — test-only synchronization; no production Owner path | run the upload-arena reuse unit test without ignored or hardware tests |
| `88ec089a` | #185 | add an Xorg scenario measuring `xrandr --scale` behavior | none — probe tooling only | `tools/vng-scenarios/xrandr-scale.sh` against Xorg and yserver |
| `9db808e6` | #185 | specify the CRTC transform model and its coordinate spaces | none — design documentation only | use the scale and crop scenarios as the behavior checklist |
| `44c01228` | #186 | implement RANDR CRTC transforms, fractional scaling, pointer confinement, cursor restoration, and transform intermediates | **high** — transform composition and transformed readback are in the Legacy scene path; C.0 Owner keeps mode-sized output/commit state and does not install transforms into its layout | `tools/vng-scenarios/xrandr-scale.sh`, `xrandr-scale-crop.sh`, `xrandr-scale-cursor.sh`, and `pointer-scale.sh` on Legacy and Owner; on Owner the transform behavior is currently unsupported and must be recorded as such |
| `93a186bd` | #187 | propagate `xrandr --dpi` screen-size changes to applications started afterward | **medium** — shared RANDR screen physical-size state and client setup are used on both routes | `tools/vng-scenarios/xrandr-dpi.sh`; set DPI, launch a fresh client, and compare its reported resolution |
| `70a6d20e` | #188 | implement RANDR virtual monitor set/list/delete requests and monitor state | **medium** — request handling and monitor metadata are shared in core, while this feature does not configure KMS outputs | `tools/vng-scenarios/xrandr-monitors.sh`; set, list, and delete monitors on both routes |
| `736a8036` | #189 | implement screen rotation and reflection through `xrandr --rotate` / `--reflect`, including CRTC and legacy SetScreenConfig paths, transformed rendering, cursor/pointer behavior and probes | **high** — shared core accepts and publishes rotation/reflection under `RandrMutationGate`; CRTC apply uses C.0's async token/publication path. Legacy rendering applies the combined matrix. Owner currently reports the rotated RANDR footprint but filters Owner outputs from transformed layout/composition, so its scanout pixels, allocation and commit identity remain mode-sized. SetScreenConfig's other-CRTC pre-disable cannot yet sequence an Owner pending operation; that token is canceled and the request fails rather than becoming unowned work. | `tools/vng-scenarios/xrandr-rotate.sh`, `xrandr-orientation.sh`, `xrandr-rotate-scanout.sh`, and `pointer-rotate.sh` on Legacy and Owner; compare pixels, CRTC/screen replies and notifications, pointer confinement and cursor placement |
| `d9af91d1` | — | preserve and tile the root background pixmap across screen-size reallocations | **medium** — root/COW storage is shared state used by the resize and repaint paths; C.0 changes how root paints resolve through redirection | `tools/vng-scenarios/root-background-resize.sh` on Legacy and Owner with a COW present; verify newly exposed pixels retain the tile phase after grow and shrink |
| `78960eb9` | — | root `GetImage` reads scanout under CRTCs and the root background where no CRTC shows the screen | **high** — root reads choose between the current scanout (including Owner's `owner_current_bo_idx`) and root backing; direct scanout adds another source that must match the displayed pixels | capture a root larger than the enabled CRTC footprint on Owner; compare pixels under each CRTC and in uncovered areas, including while an Owner-current or direct frame is displayed |
| `576d467c` | — | repaint scanout after a same-size CRTC rotation or reflection change | **high** — transform changes affect scanout composition and repaint scheduling; Owner transform composition remains the limitation recorded for #189 | run `tools/vng-scenarios/xrandr-rotate-scanout.sh` on Legacy and verify the same-size transform repaints; on Owner record the current transform support boundary and ensure no stale frame is claimed as supported |
| `028b4ddf` | — | `SetScreenSize` to the current pixel and millimeter size emits no `ScreenChangeNotify` | **medium** — shared RANDR request/event handling and C.0's mutation publication path | issue an identical `SetScreenSize` on both routes and verify no `ScreenChangeNotify` is delivered |
| `821c372f` | — | seed XKB key types and interpret actions from xkeyboard-config sources | low — XKB map construction and key cooking are route-independent | run the XKB golden tests and `tools/vng-scenarios/xkbcomp-upload.sh` on Owner |
| `bf029404` | — | require `xinput` flat-acceleration support in pointer scenarios and regenerate Xorg goldens with it | none — VNG test setup and goldens only | run the pointer VNG scenarios; verify the guest has `xinput` and the regenerated Xorg goldens match |
| `e5dd4c70` | — | synchronize the orientation scenario's event listener at startup and on a root property marker | none — VNG scenario synchronization only | run `tools/vng-scenarios/xrandr-orientation.sh` against Xorg and yserver and verify listener startup and event capture |
| `dd7b3c5d` | #191 | refuse direct scanout when the leaf-to-root Bounding/Clip shape chain does not cover the root; unflip when a shape change clips a retained direct frame | **high** — the shared direct-eligibility predicate and unflip path govern the frame shown on Owner and Legacy; Cinnamon's COW shape is the reproduction | lock and unlock Cinnamon on Owner with its COW stage presenting; verify the empty COW shape exposes the locker and the stage is composed again afterward (`tools/vng-scenarios/cow-empty-shape.sh` is the deterministic shape probe) |
| `a8e7e761`, `f43bdebe` | — | CI: vng regression suite (real yserver in a KVM guest, Xorg goldens, pixel oracle) on ubuntu-26.04, triggered on master pushes | none — tooling; no crate code. The suite runs the default (Legacy) route, so it exercises nothing of the Owner route until stage 5 activation | once Owner is activated, run the vng suite with the Owner route selected and compare its goldens |

## Reproduction tooling already in the tree

`tools/border-damage-probe.c`, `tools/composite-shrink-probe.c`,
`tools/resize-expose-probe.c`, `tools/xkb-behaviors-probe.c` and the
`tools/vng-scenarios/*.sh` runners (upstream `aa2dff45`, "four deterministic
X11 A/B probes for #143 and #150") are the A/B instruments for the #143 family
and #150. Each probe runs against Legacy and Owner and the outputs are compared.

## When

After stage 4's acceptance, before the C.0 squash-merge (C.0 §18): the
re-validation is part of the final-tip evidence, and a fix that no longer holds
on the Owner route is a defect of the conversion, filed and fixed before the
merge. Upstream merges that land in the meantime extend this table.
