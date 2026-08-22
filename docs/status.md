# Status — KMS render backend

Working doc for the KMS render backend. The governing spec still
lives at `docs/superpowers/specs/2026-05-15-rendering-model-v2.md`;
that filename is historical, and this file tracks execution
against it.

Historical sections below still mention `v2` where they quote old
branch names, plan/spec filenames, file paths, test names, env vars,
or log strings from the migration period.

Earlier program docs are archived:

- `status-archive-2026-05-21.md` — Stage 4 close diagnosis chain
  on `cow-authoritative-mode` (Phase 1+2 plan + correctness
  fix-chain narrative + 4d.8 reverted pragmatic-floor attempt +
  4d open-investigation items that closed by the cow-authoritative
  branch).
- `status-archive-2026-05-15.md` — the v1 rendering re-architecture
  (Phases 3A–3F-2, sync rework, pixmap pool, GPU traps, the paused
  timeline-semaphore migration, the abandoned convolution filter +
  Manual-redirect work). Re-read it for context on what's already
  in tree, what was tried and reverted, and what was deliberately
  paused.
- `status-archive-2026-05-13.md` — pre-rework history (Phases 1–6
  + host-X11 era).

Cross-cutting bugs and followups that don't fit a stage live in
[`known-issues.md`](known-issues.md).

The repository-wide code-quality and technical-debt review from 2026-07-26
lives in [`code-quality-audit-2026-07-26.md`](code-quality-audit-2026-07-26.md).

---

- **2026-08-14 render-node resolution on split display/render SoCs (Asahi):**
  cinnamon flashed and rendered unstably on Apple Silicon (`air`) while
  MATE/XFCE looked fine; bisected to `bbc9d30f` (the FreeBSD render-node
  fallback above). That commit made the `/dev/dri` walk return `Ok(None)`
  when the card's sysfs parent *is* readable but no render node shares it.
  On Asahi that is the only reachable branch and it can never match: the
  scanout card hangs off `soc:display-subsystem` while `renderD128` hangs
  off `<addr>.gpu`. `open_for_card` therefore failed, `platform_init`
  logged `DRI3 render node unavailable` and continued with no render fd,
  and every GL client dropped to llvmpipe — invisible on non-composited
  desktops, fatal for muffin's per-frame GL compositing. The choice now
  lives in a pure `select_render_node`: sysfs-sibling match first, then
  the *sole* candidate if exactly one exists, and still a hard error
  (naming `YSERVER_DRI_RENDER_NODE`) when several candidates exist and
  none match. FreeBSD is unchanged — no `/sys` means no card parent, so
  it lands on the same lone-candidate/ambiguous rules as before.
  Regression coverage includes a live-hardware test asserting that on a
  host with exactly one render node *every* card node resolves to it.
- **2026-08-12 DRI3 syncobj identity and XID lifetime follow-up:** accepted
  `PresentPixmapSynced` requests now pin the exact release syncobj handle instead
  of resolving its numeric XID when the Present eventually completes. Freeing and
  reusing an XID while a Present is parked therefore cannot signal the replacement
  object. DRI3 syncobjs are also registered as ordinary core X resources, so they
  participate in global XID collision checks, XC-MISC allocation, resource-owner
  lookup, `KillClient`, and `DestroyAll`/`RetainPermanent`/`RetainTemporary`
  close-down semantics. Backend destruction follows removal of the core resource
  row; a Present-held `Arc` may intentionally outlive that row, matching Xorg's
  reference-counted resource behavior. Regressions cover reverse XID collision,
  parked-Present identity across free/reimport, all three close-down modes, zombie
  cleanup, and the 19-namespace XC-MISC occupancy audit. Design/implementation plan:
  [`2026-08-12-dri3-syncobj-identity.md`](superpowers/plans/2026-08-12-dri3-syncobj-identity.md).

## Where we are

- **2026-08-13 Present 1.4 release fences now publish submitted GPU work:**
  synced Copy presents immediately import the still-pending Vulkan completion
  `sync_file` into the client's release timeline instead of host-signalling
  the release point only after completion. This matches Xorg/Xwayland
  semantics: clients can queue dependent work while buffer reuse remains
  fenced until yserver's GPU read retires. Both ordinary and COW-batched
  copies are covered, with the previous host-signal path retained as a safe
  fallback if fence publication fails. A real DRM syncobj round-trip passed on
  renderD129. In live Warframe testing the demanding steady phase improved
  from roughly 11–13 presents/s and 240–280 ms request-to-completion to
  roughly 22–23 presents/s and 129–134 ms, with no visual or interaction
  regressions observed. The RADV modifier-order experiment is not included.
- **2026-08-22 Direct Scanout with Async Page Flip (Tearing) & Hardware Pipeline Hardening (branch `feat/direct-scanout-async-tearing`):**
  Uncapped, GPU-native framerates (300–400+ FPS) and immediate hardware screen tearing unlocked for fullscreen games with VSync disabled via KMS Atomic Async Page Flips (`PAGE_FLIP_ASYNC`), eliminating the 50% framerate cap while preserving the VBlank-locked `latest-wins` behavior for synced presentations. Implemented present capabilities advertising (`async_may_tear`), immediate scheduler dispatch (`ExecuteNow`), sub-ms buffer recycling via DRI3 wake signaling on skipped frames, socket flushing prior to epoll poll, bypassing user-space sync-file export in direct scanout, full `XFixesHideCursor`/`XFixesShowCursor` unbinding, off-thread hardware cursor movement to eliminate stutter on `nvidia-drm`, admission of fullscreen redirected windows, and removal of premature direct unflips. **Hardware validation (2026-08-22, NVIDIA RTX 5060 Ti, CS2/Marvel Rivals/MGSV, Cinnamon):** uncapped framerates with smooth real-time tearing pacing achieved with 0 unexpected unflips.
- **2026-08-12 No-vsync fullscreen present flood — async defer+supersession, fullscreen direct scanout, and the late-materialization store ref (branch `fix/fullscreen-novsync-stutter`):** a no-vsync fullscreen game (CS2) kept `page_flip/s` collapsing from refresh to 27–47 Hz because every `PresentOptionAsync` present re-composed. Phase A parks async presents while a flip is in flight and lets async successors scrap parked async predecessors (Xorg scrap semantics), so the flood coalesces to roughly one present per flip; synced-present behavior is unchanged. Phase B treats fullscreen Unredirected windows as authoritative root: such presents become direct-scanout eligible, fullscreen explicit-sync presents are accepted on the direct path, and fullscreen sources are pre-probed before admission.    A composed unflip while a game holds direct scanout now materializes the presented source into the frame's actual fallback target (the window's own backing for Unredirected frames, not only the COW) and, if the synchronized atomic unflip fails, degrades to the per-output composed flips without releasing the still-scanned dma-buf or exiting the server. Also on this branch: a deferred store ref recorded for Pictures whose backing materializes late, fixing the game-start transparency bug where `free_pixmap` destroyed a drawable under a live Picture. **Hardware validation (2026-08-12, nvidia box, CS2 no-vsync, Cinnamon): the stutter is fixed** — `page_flip/s` holds 59.8–61.0 through the flood (was 27–47) with `present_skips/s` coalescing at a 266/s median. The original finding's `options=0x8`=async premise was wrong: 0x8 is `PresentOptionSuboptimal` (synced), so the observable fix on this box comes from the pre-existing synced supersession plus the merged DRI3 syncobj fixes (PR #122), while the Phase A async defer+supersession addresses the spec's genuinely-async flood case. Direct scanout (Phase B) did not engage here — `m1_probe_pass=0` with no rejects/errors because the Hw-cursor precondition was unmet (CS2 hides the OS cursor in fullscreen), recorded as the plan's known software-cursor blocker rather than a regression.

