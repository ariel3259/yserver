# Phase C.0 — tests deferred to the real Owner server (stages 4 and 5)

**Why this file exists (user, 2026-09-26).** Some C.0 evidence can only be
produced once a real server runs on the `Owner` route with everything wired:
the real core loop, real clients, real KMS devices. Until stage 4 switches
production to `Owner` and stage 5 activates it by capability, production
always starts on `Legacy`, so these cases can only be simulated, and a
simulation harness would be obsolete the moment the real server exists. Each
plan closes with what it can test for real now; everything that needs the
assembled server is listed here and run in stages 4 and 5. It is the
checklist; `docs/status.md` records the outcomes.

Rules: every entry names its origin (plan and test or case), what must hold,
and the stage that owns it. An entry is closed only by a run on the real
server (or on hardware for a device-dependent entry), never by a fixture.

| Origin | What must hold | Needs | Stage |
| --- | --- | --- | --- |
| 3b-ii Task 5 (plan rev 9) | Legacy vs Owner wire parity through the real core loop and `RandrMutationGate`, per connection (reply, status, events, in order), for: supersession by a `REC-4` event; two concurrent clients incl. the MATE sequence; cross-device and cross-transport order; the requester disconnecting while parked; each `E` stage expired and `Q`; the VT released; a requester-less publication and the design §7.5 supersession sequence; the §8.2 compound scripts (FIFO admission vs ready-ring order; `Q` expiry behind a predecessor whose installation changes the waiter's validation; a malformed waiter; a synchronous waiter behind a slow mutation; the mixed server Owner A in flight, Legacy B and Owner C queued, B stalled past C's `Q`, C answered on the first iteration after B returns). Differences only the design §8.4 named exceptions. | a real server on Owner with real RANDR clients (three requesters + a listener) | 5 (layer-3 rerun against the user's golden capture) |
| 3b-i-2 Task 5 F15 (plan rev 5) | `c0_3bi_enable_on_b_unflips_a`: all outputs on A with a direct frame current, enable an output on Owner B: A's unflip retires before B's modeset dispatches. Blocked today by the single `ResourceService`/`DrmCleanupRegistry`; activation must install one per Owner device, then this test is written. | one resource service per Owner device | 4/5 (activation) |
| 3b-i-2 Task 5 hardware | the second-device step of `c0_hw_3b_modeset_owner_on_card1_drm`: a mode change on one device while the other composes, the other receiving no lifecycle commit. Skips today: card0 (amdgpu) has no lit output. | a monitor lit on a second device | 5 (final-tip hardware, with 3a §5.3.1's iGPU check) |
| 3a §5.3.1 | the iGPU's DPMS off-fence loop and bounded delivery check | a monitor on the iGPU | 5 (final-tip bounded delivery check) |
