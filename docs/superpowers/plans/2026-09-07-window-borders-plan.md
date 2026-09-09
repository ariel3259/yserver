# Server-side window borders — implementation plan

Implements `../specs/2026-09-07-window-borders-design.md`. Read that first; this
plan does not restate its reasoning, only what to build, in what order, and what
proves each part. Piece labels (P1–P9) are the spec's.

**Branch:** `fix/133-window-borders`. The spec is committed there as `41132def`,
based on master at `735c7c6f`. One branch for the whole issue, including the
`7e1484b4` re-land.

**Deliverable:** awesome at `border_width = 16` draws its borders. The scanout
pixel scan that found the bug flips from **0 exact `#ff0000` and 0 `#00ff00`** to
~90 k split between them, content lands at `x = 16` with the titlebar at
`y = 33`, and `bw == 0` desktops are observably unchanged.

## Ordering principle

Dependency order, with one rule on top: **every step must be provable on its
own**, and the `bw == 0` no-op invariant is checked at *every* step rather than
once at the end. Regressing the `bw == 0` path would break every desktop we
currently support to fix one that nobody's WM configures by default.

The risk is not evenly spread. Step 1 is pure state and lands invisibly; step 2
is invisible on the KMS path but **visible through a host X server** (see its
proof). **Step 3 is the one that can go wrong quietly and that everything after
depends on** — it gets an explicit review gate, and it already moves `bw > 0`
pixels. Steps 4–9 are individually observable.

## Prerequisites

- [x] Nothing to merge; master is the base.
- [x] **Capture the pre-change baselines and keep them out of the repo** (they
  are gitignored, and `feedback_no_raw_captures_in_repo` applies):
  - awesome at `bw = 16` with red/green borders — scanout PPM plus the
    `0` / `0` exact-colour pixel counts, and the `windows.txt` dump. This is the
    A/B reference for step 4.
  - MATE and e16 idle — painted fractions and walk counts. The `bw == 0`
    reference for every step.

  Outcome: the awesome reference WAS captured and drove step 4 (0 red / 0 green
  exact-colour pixels in 3.69M, against 19920 / 19920 after). The MATE and e16
  idle reference was never captured as a snapshot and did not need to be — 9.3
  ran master through the same recipe instead, which is strictly better.

  **NOT blocking (jos, 2026-09-07):** "we can always run it from master." The
  spec's §Verification asks for numbers *identical to master*, not to a
  snapshot, so the reference can be produced at any time by building master and
  re-running — which is also the more trustworthy comparison, since it holds the
  machine and the workload constant with the candidate run.
- [ ] **Prove the xts oracle before changing anything.** Run
  `tools/xts-vs-baseline.py` against
  `../findings/2026-06-18-xorg-xts-baseline.tsv` on unmodified `HEAD` and
  confirm it produces a clean diff. Step 7 depends on that comparison being
  trustworthy, and runs are not deterministic
  (`feedback_xts_iteration`, `feedback_xts_vacuous_passes`).

  **Not done as written, and superseded.** No clean-diff run against
  `2026-06-18-xorg-xts-baseline.tsv` on unmodified `HEAD` is on record. What
  the campaign did instead was capture a fresh MASTER journal per section
  (`journal-xlib4-master`, `journal-xlib9-master`, `journal-xi-master`, all
  2026-09-07) and diff every step against those — which is the comparison
  `feedback_xts_iteration` actually prefers ("measure against our OWN previous
  run, never against Xorg") and which makes the oracle's own calibration moot:
  both arms come from the same machine, same suite, same day.
- [x] No open decisions. Pixmap borders are **in** scope (spec P5) — and they
  are now the one path with a purpose-written client and a pixel-exact Xorg
  comparison behind it (9.1).

Per-commit, exactly as CI runs it (`AGENTS.md:12` — a crate-scoped or
non-`--all-targets` run misses lints in test code and they surface only on
GitHub):

```
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

No commit before a HW smoke for any change that moves pixels
(`feedback_no_commit_before_smoke`). Applied per *sub-step*, because the
boundary does not fall on a step boundary:

- **1.1–1.4, 2.1–2.3, 2.5** — KMS-side state only, no pixels. Commit on static
  checks.
- **2.4 (host-X11 / nested forwarding)** — visible, because the host X server
  draws the ring itself. It is covered by the rule: commit it behind the nested
  visible proof in step 2. **If the nested path cannot be run** — the ynest dev
  harness is deprecated in favour of vng
  (`project_discontinue_ynest`) — then this is an *explicit, reasoned
  exemption*: commit it stating "unsmoked, no runnable nested harness", mark
  2.4 unverified, and carry it to 9.5. What is not acceptable is committing it
  silently under a rule that says pixel changes start later.
- **3.1–3.5 onward** — moves `bw > 0` pixels (storage layout and target
  offsets) even though nothing paints the ring until step 4. Either commit
  behind step 3's own narrow awesome smoke, or leave deliberately uncommitted
  until step 4's smoke covers both; say which in the commit.

### The xts A/B is a hard gate, not a closing sweep

**Every step that touches a drawing, clipping or storage path must pass an
xts5 `Xlib4` + `Xlib9` A/B against master before it is considered done** —
per step, not once at step 9. Diff per test purpose with
`tools/xts-vs-baseline.py`; the pass condition is **zero PASS → FAIL and zero
PASS → UNRESOLVED**. Run both halves back-to-back on the same box, since the
runs are not deterministic. `Xlib9` is the important half: it covers CopyArea,
CopyPlane, PutImage, GetImage/GetSubImage, ClearArea/ClearWindow, every fill,
every draw and the text ops — i.e. essentially everything the target API
touches. `Xlib4` covers the window and attribute ops.

This was added after step 3, on evidence. Step 3 passed **2807 unit tests, a
`clippy --all-targets -D warnings` gate, three codex review rounds, and a MATE
visual check** — and still shipped two `bw == 0` regressions that only the
hardware xts A/B found:

- 14 `Xlib9` purposes PASS → FAIL, every one the `subwindow_mode =
  IncludeInferiors` assertion, across CopyPlane, PutImage and every fill and
  draw op — one root cause in the descendant fan-out.
- `Xlib4` `XSetWindowBackgroundPixmap` tp2 FAIL → **SIGSEGV** in the client, on
  the ParentRelative background path.

The lesson generalises past this campaign: **static review cannot see this class
of bug, and neither can a unit suite that has no coverage of the path.** Both
regressions were in code paths with no existing test, which is exactly where a
refactor of the clip mechanism was always going to break something. Treat a
green unit suite plus a clean review as necessary and not sufficient for any
step that moves pixels.

## Step 1 — protocol parse + resource state (P1, P2)

- [x] 1.1 Add `border_pixel: Option<u32>` and `border_pixmap: Option<ResourceId>`
  to `CreateWindowRequest` and `ChangeWindowAttributesRequest`; read value
  indices 2 and 3 in `create_window_request` and
  `change_window_attributes_request` (`yserver-protocol/src/x11/mod.rs:912`,
  `:941`).
- [x] 1.2 `BorderSource { Pixel(u32), Pixmap { id, host_xid } }` on `Window`,
  replacing the vestigial `border_pixmap_host_xid`. CreateWindow **inherits the
  parent's** (`dix/window.c:879`), not `Pixel(0)`.
- [x] 1.3 Validation: `CopyFromParent` resolution including the
  parent-is-pixel → switch-to-pixel case; depth/screen BadMatch; BadPixmap for a
  stale xid; pixel-overrides-pixmap when both bits are set; pixmap
  refcount/destroy on replacement.
- [x] 1.4 The `dix/window.c:818` BadMatch — depth differs from the parent, class
  is not InputOnly, and no border attribute supplied.

**Proof.** Unit tests in `resources.rs` and `process_request.rs` for each
validation branch. xts5 Xlib4 `XChangeWindowAttributes` / `XCreateWindow`
subsections diffed against the baseline.

**Expect no visible change: the scanout scan must still read 0 / 0.** That is
the point of step 1 and the temptation to "check it on screen" here is wasted
time.

## Step 2 — backend geometry and propagation (P3)

- [x] 2.1 `WindowGeometry` (`kms/render/backend.rs:90`) gains `border_width` and
  the resolved `BorderSource`.
- [x] 2.2 `create_subwindow` stops discarding `_border_width` (`:17204`);
  `configure_subwindow` honours `config.border_width` (`:17325`).
- [x] 2.3 A **new** CWA forward path for the border source. The existing one
  fires only when a background changes (`process_request.rs:20660`), so border
  changes reach the backend today by no route at all.
- [x] 2.4 `host_x11` / nested (`nested.rs:779` already reads `border_width`) and
  the recording/test backends (`backend/recording.rs:1143`).
- [x] 2.5 Border-pixmap orphan / reference tracking, mirroring the background
  helpers.

**Proof, split by path — step 2 is not uniformly invisible.**

- **KMS path: no pixel change.** Nothing consumes the state yet. Backend unit
  tests assert the geometry mirror matches the resource tree for `bw != 0`,
  **including a test that the two trees agree** — divergence between the
  authoritative tree and the render mirror is a standing bug class here.
- **host-X11 / nested path: visible immediately.** Once 2.4 forwards a real
  border source to a host X server, that host draws the border itself. So this
  is a visible change and must not be claimed as invisible. It is also an
  *opportunity*: the host's own border rendering is a known-good oracle, giving
  an independent check on the border source well before yserver can paint one.
  Proof is a nested run showing the ring in the host's colours.

⚠ **Risk on that oracle:** the ynest dev *harness* is deprecated in favour of
vng, though the nested-backend architecture is retained
(`project_discontinue_ynest`). If the nested path cannot actually be run, 2.4
ships **unverified** — mark it so explicitly and carry it to step 9's host-X11
parity item; do not let "no test ran" read as "no change".

## Step 3 — the drawable-target API (P4) ⚠ review gate

The spec's central mechanism, and the step where a mistake is invisible until
much later.

- [x] 3.1 Introduce the opaque drawable-target handle as the **only** route to
  window storage: destination clipping in content coordinates, source
  clipping/read bounds, backing translation, and separate privileged
  backing-space operations.
- [x] 3.2 Migrate every destination op and every read onto it. Enumerate
  explicitly rather than by grep-and-hope: `put_image` (`:19632`), fills, glyphs,
  RENDER, `CopyArea` destination **and** source, `GetImage`,
  `clear_window_area_with_background` (`:3407`).
- [x] 3.3 Storage sizing `(w + 2bw) × (h + 2bw)` in
  `sync_window_leaf_storage_to_geometry` (`:3301`) and at creation; content
  offset per the spec's four cases — leaf storage, ancestor backing, root
  redirection, nested descendants — extending `resolve_paint_target` (`:4855`)
  to accumulate content offsets, not only `x`/`y`.
- [x] 3.4 Redirected windows: content clip derives from the window itself with
  no ancestor term, both redirect modes (spec §The redirection exception,
  rule 1).
- [x] 3.5 **Reject direct scanout for `bw > 0`.** This belongs here, not in a
  later step: 3.3 is the commit that makes bordered storage exist, and the flip
  path assumes content begins at storage `(0,0)`. A bordered fullscreen
  candidate must never reach it. Gating after the layout change would leave a
  window where a bordered window can be flipped with a `bw`-shifted source.

**Proof.** Three parts, all required:

- At `bw == 0` the migration must be a **pure refactor** — MATE and e16 A/B
  against the step-0 baseline with painted fractions and walk counts unmoved.
- At `bw > 0`, one content-clip test **per migrated operation family**, not a
  sample. The checklist migrates more than the obvious three:
  - a core fill destination and a core text/glyph destination;
  - a RENDER destination;
  - `CopyArea` **source** and **destination** separately;
  - `PutImage` at negative coordinates;
  - `GetImage` returning the rectangle asked for, ring included when the
    request reaches into the border — it is border-inclusive per
    `dix/dispatch.c:2179`, and a clamped read that shortens the reply while the
    header advertises the full length segfaults the client;
  - and the complement — the **privileged** background/border route *can* reach
    the ring while every client route cannot. Without that last one the tests
    pass equally well on an implementation that simply cannot write the ring at
    all, which would then fail at step 4.
- A bordered fullscreen window does not take the flip path (3.5).

**Gate: jos reviews the API shape before step 4 starts.** Steps 4–9 all build on
this boundary; discovering it is wrong at step 8 means redoing them.

## Step 4 — the ring fill (P5) — first **KMS**-visible step

> **Steps 1-3 are DONE and xts-gated** (`f6edf4da`, pushed). Xlib4:
> 180 PASS / 105 FAIL / 3 UNRESOLVED vs master's 171 / 112 / 5 — 11 changes,
> **0 regressions**, and two of master's own crashes fixed. Xlib9:
> 846 / 363 / 0, **identical to master across all 1472 purposes**.
>
> ⚠ **CORRECTED after step 4 — the oracle below does NOT hold.** It was
> written from the purposes' names and assertions without checking how they
> *observe* the result. Two blockers were then measured through `ProtoFixture`:
>
> 1. **Six of the seven need step 6, not step 4.** `XSetWindowBorder` tp1-3,
>    `XSetWindowBorderWidth` tp1 and `XSetWindowBorderPixmap` tp1-2 all create
>    the window via `crechild` — `XCreateSimpleWindow(..., border_width = 0)`
>    (`xts5/src/lib/crechild.c:189`) — and only *then* call
>    `XSetWindowBorderWidth`. Per step 3's invariant that leaves the allocation
>    unchanged, so there is no ring to paint at all. The realloc-and-migrate is
>    6.1/6.2. Only **`XSetWindowBorderPixmap` tp3** supplies the width at
>    `CreateWindow` (`mkwinchild(..., parent, 5)`).
> 2. **All seven read the PARENT, and a parent read sees no child pixel.**
>    `getpixel`/`checkpixel` and `PIXCHECK`→`verifyimage` are a plain
>    `XGetImage` on the passed drawable (`xts5/src/lib/checkpixel.c:145`,
>    `verimage.c:176`). Xorg answers those because `DoGetImage` reads
>    `pBoundingDraw` — for an unredirected window, the screen pixmap, which
>    holds its children's pixels too. **yserver gives every window its own
>    storage and `get_image` on a NON-ROOT window reads only that window's**,
>    with no descendant compositing, so a child's border *and* content are both
>    invisible through a parent read. (Root is fine: `get_image` on the root
>    reads the composited scanout via `read_root_scanout_assembled`.)
>
> Blocker 2 is a **standing architectural divergence, in no plan step**, and it
> is worth its own investigation: xts verifies drawing by reading pixels, often
> through a parent, so it may account for a meaningful share of the ~363
> pre-existing `Xlib9` failures. Do not fold it into this campaign.
>
> **So step 4's proof is the awesome `bw = 16` scanout pixel scan plus the
> `ProtoFixture` replays**, which assert the ring on the window's own backing —
> the same pixel the purposes reach for. The seven purposes become **step 6's**
> oracle, and even then only if blocker 2 is fixed.
>
> ✅ **Step 4 DOES have an oracle, found by running it — and it PASSES.**
> `XChangeWindowAttributes` **tp18** and `XCreateWindow` **tp16** both assert
> *"when border_pixel is specified the value is truncated to the depth of the
> window"* and do it by **reading one pixel back from the border**. Neither
> blocker applies: they read the border on the window itself (not through a
> parent) and specify it at CreateWindow/CWA (not via a later
> `XSetWindowBorderWidth`). They also independently exercise the depth-32 alpha
> rule via the truncation check.
>
> Their history across the campaign is the cleanest signal in it:
> **master UNRESOLVED (crash) → step 3 FAIL (crash fixed) → step 4 PASS (ring
> painted)**. Xlib4 at step 4: 182 PASS / 103 FAIL / 3 UNRESOLVED against
> master's 171 / 112 / 5 — 11 changes, **0 regressions**.
>
> **Xlib9 adds two more, also unpredicted:** `XGetImage` **tp7** and
> `XGetSubImage` **tp7** — *"when the specified rectangle includes the window
> border, the contents of the border are obtained"*. The handler already
> permitted the read (`process_request.rs:25564` cites this very purpose); it
> failed because nothing was painted there. Step 3 made the read return the
> requested rectangle ring included, step 4 put the pixels in it, and they
> compose. Xlib9 at step 4: **848 / 361 / 0, zero regressions.**
>
> **Step 4 total: 4 purposes flipped to PASS, zero of them predicted by me, and
> zero regressions across all 1796 purposes of both sections.**
>
> Lesson for the remaining steps: I picked the original seven by *name* and by
> what they assert. These two were found by *running the thing* and diffing.
> Prefer the diff to the guess.
>
> Not step 4's or step 6's, despite the names: `XSetWindowBorderWidth` tp3 is a
> SubstructureRedirect / ConfigureRequest assertion, and
> `XSetWindowBackgroundPixmap` tp2's surviving sub-check is a plain-background
> ParentRelative defect (its two *tiled* sub-checks were fixed by the
> tile-origin work in step 3).

- [x] 4.1 A server-internal backing-space fill modelled on
  `clear_window_area_with_background`, taking exactly `outer − content` in
  backing coordinates and bypassing the content clip by construction.
- [x] 4.2 Solid and tiled. Tile origin from the **ParentRelative walk**
  (`miexpose.c:458`), not the inner origin.
- [x] 4.3 The depth-32 alpha rule. Landed: the solid path records a plain clear
  with no shader so nothing forces α and the rule is applied explicitly; the
  tiled path does not need it, since Xorg's rule sits inside `if (solid)`.
- [x] 4.4 Trigger on creation, border-source change, `border_width` change,
  geometry change, and redirect-backing allocation (`compwindow.c:137`).

**Proof — achieved.** awesome at `bw = 16`: exact `#ff0000` / `#00ff00` went
from `0` / `0` to nonzero, and at step 5 every ring matched
`2·bw·(outer_w + outer_h − 2·bw)` to the pixel. Tiled borders still have **no
WM coverage** — awesome sets `border-pixmap` zero times on either server — so
the `ProtoFixture` replays are the only proof for that half.

**Restored note:** this checklist was accidentally deleted when I rewrote the
oracle blockquote above (the replacement ran to the next heading). Recovered
from `88c60bda`.

## Step 5 — scene spaces and the coordinate recurrence (P6)

- [x] 5.1 Two absolutes per node in the walk:
  `child_outer = parent_content + child.x/y`, then
  `child_content = child_outer + child.border_width`. Today there is one
  (`scene.rs:6141`).
- [x] 5.2 Outer region for node sampling and sibling occlusion; **inner** region
  for the descendant clip (`mivaltree.c:386`).
- [x] 5.3 Shaped variants: `winSize` ∩ bounding ∩ clip, intersected in
  window-local coordinates (`dix/window.c:1735`); the `SetBorderSize` rule for
  the outer region.
- [x] 5.4 The redirection exception — **reset** the descendant clip to the
  window's own outer region for a window owning its own `redirected_target`
  (`scene.rs:6264`), rather than narrowing it (`clip_x0.max(abs_x)` at `:6157`),
  and let that propagate to descendants.

**Proof.** Content at `x = 16`, titlebar at `y = 33`. A child cannot overlap its
parent's border. A nested bordered child under a redirected ancestor is not
displaced, in both redirect modes. `bw == 0` A/B unmoved.

## Step 6 — border-width change (P8)

> **Steps 1-5 are DONE and xts-gated.** Xlib4 182/103/3 and Xlib9 848/361/0
> against master's 171/112/5 and 846/363/0 — **zero regressions across all 1796
> purposes**, four purposes gained (border-pixel depth truncation ×2,
> border-inclusive GetImage ×2). Step 5 moved nothing in either section, which
> is the `bw == 0` identity claim confirmed on the walk.
>
> **Step 6 has a live reproduction waiting.** An awesome session at `bw = 16`
> shows a **1 px** border on whichever frame was most recently mapped, and a
> correct 16 px border on the rest. Measured cause: `content_offset` is
> re-recorded only by `sync_window_leaf_storage_to_geometry`, which fires on a
> **w/h** change and not on a border-width change. A frame created with
> `border_width = 1` and then given 16 keeps `content_offset = 1`, so the ring
> correctly follows its allocation. The other frames were resized by tiling
> after their border was set, so they reallocated and picked up 16.
>
> Ring sizes matched `2·bw·(outer_w + outer_h − 2·bw)` to the pixel for all
> three frames (62 176 + 5 270 red, 62 784 green), so step 5 is right and this
> is purely the missing realloc-and-migrate.

- [x] 6.1 Reallocate only if the bordered extent changed.
- [x] 6.2 Relocate content whenever the **content offset** changes, realloc or
  not, overlap-safe.
- [x] 6.3 Damage `old outer ∪ new outer`.
- [x] 6.4 A pure `x`/`y` move relocates nothing storage-local.

**Proof.** Client content preserved across a `border_width` change, **including
`w=100,bw=2 → w=98,bw=3`** (outer stays 104, no realloc, content must still
migrate from offset 2 to 3). That case is the one an implementer working from
prose would skip.

## Step 7 — the origin re-land (P7)

- [x] 7.1 `window_absolute_position`: `ax += x + bw`, `ay += y + bw`
  (`resources.rs:~2707`) — re-landing `7e1484b4`.

**Proof.** xts5 `XI/GrabDeviceButton-4` reports x_root 104, not 103. Diff the
**full** XI run against the eiger baseline, never eyeballing pass counts
(`reference_xorg_not_100pct_on_xts_xi`).

> ✅ **DONE and gated — `959642a7`.** XI: exactly **one** change across 316
> purposes, `GrabDeviceButton` **tp4 FAIL → PASS** — the oracle itself — with
> **zero regressions**; Xlib4 and Xlib9 unchanged. Stock Xorg passes tp4 too, so
> we now agree with it on the assertion this change was written for.
>
> **The revert mystery is answered.** Nothing regressed anywhere, so whatever
> prompted `d08d6933` was either never a test regression or has since been fixed
> by other work. Its stated premise — that the term is "invisible on real
> desktops, where window managers use border_width 0" — was simply wrong, and
> awesome is the counter-example.
>
> Unrelated, recorded while there: `XI/GrabDeviceButton` **tp11** fails on master
> *and* branch while stock Xorg passes it. A real divergence, pre-existing, not
> this step's. Worth its own look.

**This is the step most likely to reproduce whatever caused `d08d6933`.** If it
regresses something, that regression *is* the missing revert rationale: record
it in the spec and decide deliberately, rather than reverting blind a second
time.

## Step 8 — input (P9)

- [x] 8.1 Border-inclusive hit region (outer space) — `outer_contains_content_point`
  (`resources.rs:3163`), with `hit_test_rejects_one_pixel_outside_the_border`
  (`:4532`) pinning the far edge.
- [x] 8.2 Coordinates relative to the content origin, **negative on the left and
  top borders**, neither rejected nor clamped —
  `hit_test_border_sides_and_corners_report_content_relative_coords`
  (`resources.rs:4520`, all four edges and all four corners via
  `BORDER_PROBES`), `hit_test_border_term_accumulates_per_level` (`:4571`) and
  `content_and_parent_coord_translations_are_inverses` (`:4703`).
- [x] 8.3 Input shape applied in content coordinates (`dix/window.c:2986`) —
  `input_shape_is_tested_in_content_coordinates` (`server.rs:5296`), which also
  regresses the old "outer origin called (0,0)" bug.
- [x] 8.4 **Both** implementations: `hit_test_child` (`server.rs:2401`) and
  `child_containing_point` — `child_containing_point_agrees_with_pointer_target_at_on_borders`
  (`resources.rs:4642`) and `..._one_level_down` (`:4671`) assert the two agree
  rather than testing each in isolation.
- [x] `bw == 0` identity — `hit_test_is_identity_at_border_width_zero`
  (`resources.rs:4598`).

**Proof.** Synthetic clicks on all four sides and all four corners at `bw = 16`,
asserting the window hit and the exact (possibly negative) coordinates, against
both implementations. HW smoke: border drag and resize work under awesome.

> **Step 8 was implemented and smoked but never ticked here** — jos caught the
> same drift on steps 5 and 6 earlier in this campaign. Implementation landed
> in `78a96780` (`03af57b4` before the rebase); jos verified on bee that the
> decoration buttons land on the decoration rather than a border-width above
> it, which was the reported symptom that motivated P9, and re-tested awesome
> on hardware on 2026-09-08. The test evidence above was read assertion by
> assertion during the 9.1 audit, not matched by name.

## Step 9 — verification sweep

- [x] 9.1 The full test matrix from the spec's §Verification. Audited case by
  case against the existing suite; cases 2-8, 10, 11 and 13 already had real
  tests (assertions read, not names trusted), including the
  `w=100,bw=2 -> w=98,bw=3` unchanged-outer-extent case and both redirect
  modes. Four gaps closed, one of which was a live defect:
  - **`FreePixmap` ignored a window's BORDER reference** — a real bug, fixed in
    `72d6b9b2`. `XCreatePixmap` -> `XSetWindowBorderPixmap` -> `XFreePixmap` is
    ordinary client code and freed the host handle underneath a painted ring.
    `host_xid_referenced_by_window_border` already existed for the CWA path,
    with a doc comment promising exactly this retention; only the call site was
    missing. Two tests, both verified red without the fix, the shared-tile one
    ending on a positive control so it cannot pass by never observing a free.
  - **`ClearArea` content-clip** — had no test at all.
  - **`CopyPlane` DESTINATION against a border** — the only CopyPlane test uses
    `bw = 0` deliberately, to isolate redirect routing. The expected colour
    comes from `core.current_foreground`, which this level cannot set, so the
    test calibrates it from the same CopyPlane into a plain pixmap.
  - **Explicit `CWBorderPixmap = CopyFromParent`** (the bit SET with value 0, a
    different arm from omitting the attribute) — only its depth-mismatch
    failure was pinned. Success path now covered on CreateWindow and on
    ChangeWindowAttributes.

  Negative control for the two new acceptance tests: nulling `Dst`'s content
  bounds in `PaintTarget::dst` turns both red, alongside core fill, PutImage
  and core text. Worth noting that `copy_area_destination`, `copy_area_source`
  and the two RENDER tests stay green under that control — they are clipped
  through a different mechanism, so `dst()`'s bounds are not what those four
  are testing.
  - [x] **Tiled borders (`CWBorderPixmap`) — the path the spec says the awesome
    smoke cannot reach.** `tools/vng-scenarios/border-pixmap-client.c` +
    `border-pixmap.sh` + `tools/border-pixmap-check.py`: an override-redirect
    client, no WM, one window with a 64x64 tile of 16 identifiable cells and one
    with a solid `CWBorderPixel`, then EVERY ring pixel checked rather than
    sampled. Run on both servers in the same guest:
    **yserver and Xorg both PASS all 10624 tiled + 10624 solid ring pixels**,
    content 40000 px black on both, zero `ffffff` on either.
    The oracle is Xorg's source, not prose: a border tile aligns to the
    window's **content** origin (`mi/miexpose.c:461` `tile_x_off =
    pWin->drawable.x`, with `dix/window.c:888` making `drawable.x = x + bw`
    because CreateWindow's `x,y` is the OUTER corner). Both servers put
    `e0e080` — cell (3,3) — at the outer corner; outer-aligned would be
    `202080`. Our implementation already cited `miexpose.c:461`, and this is the
    end-to-end confirmation it was right.
- [x] 9.2 xts5 diff against the baseline; compare to our own previous run in
  `docs/test-status.md`, never to Xorg.
- [x] 9.3 `bw == 0` A/B on MATE and e16 **against master**, painted fractions
  and walk counts unmoved. (Not against a pre-captured snapshot — see
  Prerequisites.) **Both desktops done, both arms, on silence.** Every idle and
  restack phase identical on every counter; the moving phases within ±3.9% with
  mixed signs, and `resize` moving in opposite directions on the two desktops
  rules out a systematic cost. Desktop smoke: e16, e27, XFCE, MATE, Cinnamon.
- [x] 9.4 awesome HW smoke: borders drawn, focus recolour red↔green on
  focus change, drag/resize, and a multi-output check. **Done by jos on
  hardware (2026-09-08).** The paired scanout dump from that session pins the
  measurable half: silence, dual 2560x1440, both rings `1140x1090` with
  **8904 px** each — exactly `1140·1090 − 1136·1086`, a pixel-perfect 2 px ring
  on all four sides of both windows — focused green against unfocused red, both
  outputs clean, and **zero `ffffff`** anywhere. Drag/resize and the tiling
  layouts were exercised interactively across this session and the 2026-09-07
  one (floating → tiling, layout switches, maximise/unmaximise, decoration
  buttons landing on the decoration rather than the ring after step 8).
- [x] 9.5 host-X11 parity — **CLOSED BY DECISION (jos, 2026-09-08): "ynest is
  not a supported use-case ATM."** So there is nothing to verify: the nested
  path is not a shipped configuration, and step 2.4's forwarding rides along
  with the seam until it becomes one.

  For the record, it was not runnable either. The `ynest` BINARY was
  deliberately removed — `crates/yserver/Cargo.toml` sets `autobins = false`
  specifically so `src/bin/ynest.rs` cannot come back — so nothing drives
  `HostX11Backend` at all. The nested backend survives as the architecture seam
  for a future macOS/Windows substrate (`project_discontinue_ynest`); if it is
  ever revived, border forwarding is the thing to re-check first, because it
  has never run.

  What WAS testable is done: `default_shape_rect` is the only place `nested.rs`
  reads `border_width`, and it is shared with the KMS path via
  `shape_rects_for`, so it is now pinned directly —
  `the_default_shape_region_includes_the_border_for_bounding_only` (bounding
  default is `(-bw,-bw, w+2bw, h+2bw)`, clip and input are `(0,0, w,h)`, per
  the SHAPE spec) and `the_default_shape_regions_agree_at_border_width_zero`.
  That asymmetry is exactly what the white-block fix turned on, and it had no
  test before.
- [x] 9.6 `docs/status.md` note — judge drift per change
  (`feedback_update_status_md`); `docs/status.md` is an agent doc, not
  user-facing. Two sections written: the vng/Venus harness and device-selection
  change, and the white-block root cause with the before/after measurement
  matrix and the Xorg comparison.

> **9.2 DONE — the branch is xts-gated at step 9** (silence, 2026-09-08,
> journals `journal-xlib{4,9}-step9`). The change under test since step 8 is the
> clip-shape mirror fix, so the load-bearing A/B is against our OWN previous
> run, not against master:
>
> | | purposes | vs step 7 | vs master |
> |---|---:|---|---|
> | Xlib4 | 324 | **324/324 agree** | 0 regressions, +11 |
> | Xlib9 | 1472 | **1472/1472 agree** | 0 regressions, +2 |
> | XI | 316 | **316/316 agree** | 0 regressions, +1 |
>
> Counts unchanged from step 7: Xlib4 182/103/3, Xlib9 848/361/0, XI 219/48/10,
> against master's 171/112/5, 846/363/0 and 218/49/10. **Zero PASS→FAIL across
> all 2112 purposes**, and the shape fix is verdict-neutral in both
> directions — it changes no xts verdict at all. Master's 13 Xlib gains are
> `XChangeWindowAttributes` tp18/23/41/42, `XCreateWindow` tp16/21/39/40,
> `XSetWindowBorderPixmap` tp5/6/7, `XGetImage` tp7 and `XGetSubImage` tp7; the
> two UNRESOLVED→PASS are the purposes that used to abort. XI's single gain is
> `GrabDeviceButton tp4` — the same purpose that flipped at step 7 and answered
> the `d08d6933` revert mystery, now reproduced in a second run. It is still a
> grab case, so read it as "0 regressions", not as progress
> (`project_xi_grab_cases_nondeterministic`).
>
> **9.3 arm 1 — the e16 phased workload on the branch** (jos, 2026-09-08,
> `just yserver-e16-hw-workload`, silence). Numbers quoted here because the
> logs are clobbered by the next run (`feedback_no_raw_captures_in_repo`):
>
> | phase | n | painted | region | structural | overdraw | gpu_us | cb_us | composes/s | full/s | clipped/s | paint/s |
> |---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
> | idle | 19 | 0.141 | 0.141 | 0.000 | 1.01 | 15.4 | 120.3 | 34 | 0.0 | 34.0 | 1014 |
> | drag | 20 | 0.173 | 0.173 | 0.022 | 1.01 | 18.2 | 73.9 | 38 | 0.0 | 38.0 | 2614 |
> | idle2 | 20 | 0.141 | 0.141 | 0.000 | 1.01 | 15.5 | 112.1 | 34 | 0.0 | 34.0 | 1014 |
> | resize | 20 | 0.165 | 0.160 | 0.016 | 1.01 | 16.9 | 87.0 | 37 | 0.0 | 37.0 | 2066 |
> | restack | 12 | 0.134 | 0.134 | 0.000 | 1.01 | 15.8 | 77.8 | 36 | 0.0 | 35.5 | 2328 |
> | idle3 | 10 | 0.141 | 0.141 | 0.000 | 1.01 | 15.6 | 107.3 | 34 | 0.0 | 34.0 | 1014 |
>
> And the same workload under MATE:
>
> | phase | n | painted | region | structural | overdraw | gpu_us | cb_us | composes/s | full/s | clipped/s | paint/s |
> |---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
> | idle | 20 | 0.141 | 0.141 | 0.000 | 1.00 | 18.5 | 149.9 | 24 | 0.0 | 24.0 | 24 |
> | drag | 19 | 0.183 | 0.183 | 0.028 | 1.00 | 23.7 | 165.6 | 30 | 0.0 | 30.0 | 108 |
> | idle2 | 20 | 0.141 | 0.141 | 0.000 | 1.00 | 15.6 | 158.9 | 24 | 0.0 | 24.0 | 24 |
> | resize | 20 | 0.173 | 0.171 | 0.023 | 1.00 | 19.4 | 156.0 | 28 | 0.0 | 28.0 | 208 |
> | restack | 12 | 0.141 | 0.141 | 0.000 | 1.00 | 16.7 | 160.8 | 24 | 0.0 | 24.0 | 24 |
> | idle3 | 9 | 0.141 | 0.141 | 0.000 | 1.00 | 14.5 | 166.2 | 24 | 0.0 | 24.0 | 24 |
>
> Self-consistent and healthy on its own terms: `full/s` is **0.0 in every
> phase** (nothing forces a full repaint), `structural` is **0.000 in all three
> idle phases** (the scene diff contributes nothing at rest, so the border work
> adds no churn), `painted == region` everywhere except resize (0.165 vs 0.160,
> i.e. almost no bounding-box waste), `overdraw` 1.01 throughout, and the three
> idle phases reproduce each other to three decimals — and MATE's idle
> `painted` lands on the same 0.141 as e16's despite a ~40x lower `paint/s`,
> which is what you would expect if the border work costs nothing at rest on
> either desktop. `structural` is 0.000 in MATE's restack too. **e16's A/B is now
> COMPLETE** (jos ran master `f6c79967` through the same recipe on silence,
> 2026-09-08):
>
> | phase | painted before → after | structural | composes/s | full/s |
> |---|---|---|---|---|
> | idle | 0.141 → 0.141 | 0.000 → 0.000 | 34 → 34 | 0.0 → 0.0 |
> | drag | 0.171 → 0.173 (+0.9%) | 0.022 → 0.022 | 38 → 38 | 0.0 → 0.0 |
> | idle2 | 0.141 → 0.141 | 0.000 → 0.000 | 34 → 34 | 0.0 → 0.0 |
> | resize | 0.164 → 0.165 (+0.3%) | 0.017 → 0.016 | 37 → 37 | 0.0 → 0.0 |
> | restack | 0.138 → 0.134 (−2.9%) | 0.000 → 0.000 | 36 → 36 | 0.0 → 0.0 |
> | idle3 | 0.141 → 0.141 | 0.000 → 0.000 | 34 → 34 | 0.0 → 0.0 |
>
> **All three idle phases are identical to three decimals on every counter** —
> painted, region, structural, overdraw, composes/s, full/s, clipped/s and
> paint/s. At rest `bw == 0` is a pure no-op, which is exactly what step 3's
> hazard note demanded. The moving phases differ by **at most 2.9%, with mixed
> signs** (drag and resize marginally up, restack down), which reads as
> run-to-run variation rather than a systematic cost — with one run per arm
> that cannot be separated from noise numerically, and the idle phases matching
> exactly is what bounds the harness's reproducibility. `gpu_us` and `cb_us`
> wander −13% to +9% and are the noisiest fields in the set.
>
> **MATE's A/B is COMPLETE too** (same master build, same machine):
>
> | phase | painted before → after | structural | composes/s | full/s |
> |---|---|---|---|---|
> | idle | 0.141 → 0.141 | 0.000 → 0.000 | 24 → 24 | 0.0 → 0.0 |
> | drag | 0.182 → 0.183 (+0.3%) | 0.027 → 0.028 | 30 → 30 | 0.0 → 0.0 |
> | idle2 | 0.141 → 0.141 | 0.000 → 0.000 | 24 → 24 | 0.0 → 0.0 |
> | resize | 0.180 → 0.173 (−3.9%) | 0.025 → 0.023 | 28 → 28 | 0.0 → 0.0 |
> | restack | 0.141 → 0.141 | 0.000 → 0.000 | 24 → 24 | 0.0 → 0.0 |
> | idle3 | 0.141 → 0.141 | 0.000 → 0.000 | 24 → 24 | 0.0 → 0.0 |
>
> **Four of the six phases are identical on every counter** — both idles, idle3
> and restack — and the two that move go in OPPOSITE directions (drag +0.3%,
> resize −3.9%, the latter favouring the branch). `full/s` is 0.0 throughout on
> both arms and both desktops.
>
> And the strongest reading of the pair together: **`resize` moves +0.3% on e16
> and −3.9% on MATE.** The same phase moving in opposite directions on two
> desktops is something a systematic per-frame cost cannot do, so the residual
> is run-to-run variation. That is the numerical separation the single-run
> caveat above could not make.
>
> Archived arms live in `target/ab/` (gitignored).
> Archives live in `target/ab/` (gitignored). NOTE: the e16 branch phases file
> was clobbered mid-write by the next run and had to be reconstructed from the
> markers captured at the time — validated by reproducing the recorded table
> cell for cell. Archive each arm BEFORE starting the next one.
>
> **`bw == 0` desktop smoke DONE (jos, 2026-09-08): e16, e27, XFCE, MATE and
> Cinnamon, no issues seen.** That is five reparenting desktops on the
> `bw == 0` path, i.e. the whole existing user base, and it is the observation
> `feedback_no_commit_before_smoke` asks for. It is NOT 9.3's numeric half:
> painted fractions and walk counts still have to be compared against master.
>

(The direct-scanout gate was a separate step in an earlier draft. It is now
3.5, because it must land with the storage-layout change rather than after it.)

## Hazards

- **Step 1 is invisible by design, and step 2 only on the KMS path.** Their proof
  is unit tests and xts, not the screen — do not conclude they failed because
  nothing changed. But step 2's host-X11 forwarding *is* visible through a host
  X server, so "nothing changed" is the wrong expectation there.
- **Step 3 is the load-bearing one.** At `bw == 0` it must be a pure refactor; if
  the MATE/e16 A/B moves at all, stop and find out why before adding border
  behaviour on top.
- **Do not reorder 7 before 5.** The origin change and the scene recurrence both
  move content; landing them together makes a regression unattributable.
- **`bw == 0` is the whole existing user base.** Every step's exit criteria
  include it.
- **The tiled path has no WM coverage.** If the purpose-written client does not
  get written, tiled borders ship untested — say so rather than assuming the
  awesome smoke covered it.
- **`-dirty` in the startup hash is stale** — a clean-looking hash does not mean
  a clean binary. Confirm a live change by grepping for a string only it emits
  (`reference_build_hash_dirty_is_stale`).
