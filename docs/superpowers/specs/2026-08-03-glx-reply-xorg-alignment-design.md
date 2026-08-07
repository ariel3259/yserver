# GLX reply parity with Xorg — design

**Status:** **approved** (rev 4 — Opus review rounds 1-3 closed),
2026-08-03
**Base:** branch `glx-reply-xorg-alignment`, off `master` @ `468e4f2`.
**All yserver line numbers in this document are against that base.**
Xorg citations are against `~/Projects/xserver` @ `5541a5c`.
**Scope:** `crates/yserver-core/src/core_loop/process_request.rs`,
`crates/yserver-core/src/server.rs`,
`crates/yserver-protocol/src/x11/glx.rs`.

## Discovery context

Found while investigating why `libGLX_nvidia` cannot bind a GL context
against yserver (`BadAlloc` on `X_GLXMakeCurrent`). That investigation is
**not** resolved by this design and this document does not claim to fix
it — see "What this design does not claim". What the investigation
produced is a set of GLX replies that measurably disagree with Xorg, each
a defect on its own terms under the AGENTS.md rule that yserver follows
Xorg's observable behaviour.

Evidence lives in `~/yserver-glx-logs/2026-08-03-investigation/`
(`probe-run/`, `ab-fbconfig/`, `xwayland-control/`), captured with
`~/yserver-glx-probes/glxprobe2`, NVIDIA 610.57.04, RTX 5060 Ti.

## Problem — measured on the wire

**Six defects in scope (D1-D6)**, plus two recorded but deliberately
**not** implemented here (D7, D8 — see "Deferred"). D1 and D2 are
measured client-visible values; the rest are read off both
implementations' sources.

### D1 — `GetDrawableAttributes` reports 0×0 for a GLXWindow/GLXPixmap

`ab-fbconfig/nvidia-glxwindow.txt:9-10` — a 64×64 X window wrapped by
`glXCreateWindow` reports `GLX_WIDTH = 0`, `GLX_HEIGHT = 0`.

`drawable_attributes_for` (`process_request.rs:10647-10659`) resolves
geometry by looking the *GLX* XID up in the X resource tables: it tries
`state.resources.window(ResourceId(xid))`, then
`state.resources.pixmap(ResourceId(xid))`, then `unwrap_or((0, 0))`.
A GLXWindow XID is a fresh client-allocated id with no X resource behind
it, so the lookup always misses and falls through to `(0, 0)`. The
backing drawable is already recorded as `GlxDrawable::x_drawable`
(`server.rs:1189`) and is simply not consulted.

Xorg reads `pGlxDraw->pDraw->width` / `->height` (`glxcmds.c:1891-1892`),
where `pDraw` is the backing drawable (`glxcmds.c:1881-1882`).

**Severity:** highest of the six in scope. The existing comment at
`process_request.rs:10643-10644` records that Mesa's `loader_dri3` sizes
its buffer from these fields and gets "failed to create drawable" when
they are 0 — a live defect against Mesa clients taking the
`glXCreateWindow` path, independent of NVIDIA.

### D2 — `GLX_FBCONFIG_ID = 0` for a naked X window

`ab-fbconfig/nvidia-naked.txt:7` — an X window handed straight to
`glXMakeCurrent` (the GLX 1.2 pattern) reports `GLX_FBCONFIG_ID = 0`.
`fbconfig` comes from `drawable.map_or(0, |d| d.fbconfig)`
(`process_request.rs:10638`); with no `glx_drawables` entry the `0`
default is sent as if it were a real answer. No FBConfig with id 0
exists — `synthesise_glx_fb_configs` (`process_request.rs:10749`)
produces `0x101` and `0x103`.

Xorg has an explicit path for this case ("hack for GLX 1.2 naked
windows", `glxcmds.c:1875-1880`): with `pGlxDraw == NULL` the whole
`GLX_FBCONFIG_ID` / `GLX_TEXTURE_TARGET_EXT` / `GLX_EVENT_MASK` block is
skipped (`glxcmds.c:1894-1906`) and the attribute is **absent** rather
than present-and-zero.

Measured control, captured in
`xwayland-control/nvidia-xwayland.txt`: against XWayland on this same
GPU, `glxprobe2 naked` gets `GLX_FBCONFIG_ID` *(not answered)* and GL
works (4.6.0 NVIDIA 610.57.04); `glxprobe2 glxwindow` gets `0x12a` and
also works. NVIDIA tolerates the attribute being absent.

### D3 — `GLX_DRAWABLE_TYPE` is never sent

Xorg sends it unconditionally, for every drawable kind including the
naked-window case (`glxcmds.c:1908-1914`, `GLX_EXT_get_drawable_type`).
yserver never emits it (`process_request.rs:10661-10680`).

### D4 — `GLX_RENDER_TYPE` is sent and Xorg never sends it

`process_request.rs:10667` pushes `GLX_RENDER_TYPE` into the drawable
reply. It is not in Xorg's attribute set at all — `GLX_RENDER_TYPE` is a
property of an FBConfig and of a context, not of a drawable.

### D5 — `GLX_EVENT_MASK` / `GLX_STEREO_TREE_EXT` missing; the override pass is wrong

For a registered GLX drawable Xorg sends `GLX_EVENT_MASK`
(`glxcmds.c:1898`) and `GLX_STEREO_TREE_EXT = 0` for windows
(`glxcmds.c:1903-1905`). yserver sends neither.

Separately, yserver's `CHANGE_DRAWABLE_ATTRIBUTES` arm
(`process_request.rs:11978`) stores **every** attribute pair the client
sends into `GlxDrawable::attributes`, and `drawable_attributes_for`
replays all of them over the computed set at
`process_request.rs:10674-10679`:

```rust
for (id, value) in &d.attributes {
    attribs.retain(|(a, _)| a != id);
    attribs.push((*id, *value));
}
```

Xorg's `ChangeDrawableAttributes` is a `switch` with a **single** case —
it records `GLX_EVENT_MASK` and silently ignores everything else
(`glxcmds.c:1494-1503`). yserver's generic echo means a client can inject
arbitrary attributes into the reply, and even for the legitimate
`GLX_EVENT_MASK` case the retain-then-push reorders it after
`GLX_DRAWABLE_TYPE`, so no fixed reply order is achievable while the pass
exists.

### D6 — `MakeCurrent` never returns `contextTag = 0`

`process_request.rs:11783-11784` allocates a fresh monotonic tag for
every `MakeCurrent`, including the release form (`context == None`), and
never returns 0. Observed in `ab-fbconfig/yserver.log:94,98` — `tag=1`
then `tag=2`, the second being the release.

Xorg returns `contextTag = 0` when there is no new context
(`vndcmds.c:232-234`, `vndcmds.c:271-273`); tag 0 is reserved by the
protocol to mean "no context current", which `server.rs:1064-1067`
already documents.

Xorg additionally rejects an unknown incoming tag with
`GLXBadContextTag` (`vndcmds.c:219-224`) and returns the *existing* tag
when the (context, draw, read) triple is unchanged
(`vndcmds.c:235-241`). yserver does neither, so a client that tracks tags
sees them grow without bound. Both are out of scope here (follow-ups).

## Design

### D1 — resolve geometry from the backing drawable

In `drawable_attributes_for`, when a `GlxDrawable` record exists and is
not a pbuffer, resolve geometry from `d.x_drawable` instead of the GLX
XID. Naked-window and pbuffer behaviour unchanged.

### D2/D3/D4/D5 — mirror Xorg's attribute set exactly

Restructure `drawable_attributes_for` around the same branch Xorg uses.

Always:
- `GLX_Y_INVERTED_EXT = 0`
- `GLX_WIDTH`, `GLX_HEIGHT` (per D1)
- `GLX_SCREEN = 0`

Only when a `GlxDrawable` record exists:
- `GLX_TEXTURE_TARGET_EXT`
- `GLX_EVENT_MASK` (see below)
- `GLX_FBCONFIG_ID = d.fbconfig`
- `GLX_PRESERVED_CONTENTS = 1` if pbuffer
- `GLX_STEREO_TREE_EXT = 0` if window

Always, last:
- `GLX_DRAWABLE_TYPE` = `PBUFFER_BIT` / `PIXMAP_BIT` / `WINDOW_BIT`
  (Xorg's no-record fallthrough is `WINDOW_BIT`, `glxcmds.c:1909-1910`).

`GLX_RENDER_TYPE` is dropped (D4).

**The generic override pass at `process_request.rs:10674-10679` is
deleted.** `GlxDrawable` gains an `event_mask: u32` field;
`CHANGE_DRAWABLE_ATTRIBUTES` (`process_request.rs:11978`) stores **only**
`GLX_EVENT_MASK` into it and ignores all other attributes, matching
`glxcmds.c:1494-1503`. `GlxDrawable::attributes` is removed if nothing
else reads it. This is what makes a fixed reply order achievable and
therefore testable.

### Error arm — two different error codes, not one

`glxcmds.c` is **not** the front door. GLXVND is unconditionally the GLX
extension (`glx/vndext.c:212` is the only `GlxExtensionInit`;
`mi/miinitext.c:163` registers it as `"GLX"`), and the in-tree GLX is
merely a registered vendor. XWayland links `libglxvnd`
(`hw/xwayland/meson.build:161`), so the control run in
`xwayland-control/nvidia-xwayland.txt` went through this path too.

`glx/vnd_dispatch_stubs.c:456-472` resolves the XID via
`glxServer.getXIDMap` and, when that returns NULL, returns **core
`BadDrawable` (9)** — `DoGetDrawableAttributes` is never reached.
`GlxGetXIDMap` (`glx/vndservermapping.c:56-73`) falls back to
`dixLookupResourceByClass(..., RC_DRAWABLE, ...)`, and since
`X11_RESTYPE_PIXMAP` carries `RC_DRAWABLE` (`include/resource.h:62,77`),
**pixmaps do resolve** and are forwarded. Only then does
`dixLookupWindow` — windows only — fail and produce
`__glXError(GLXBadDrawable)` (`glxcmds.c:1873-1880`).

Xorg's actual observable behaviour is therefore four-way:

| XID | Xorg result |
|---|---|
| registered GLX drawable | full reply |
| naked X window | reply without the `pGlxDraw` block |
| naked X **pixmap** | `GLXBadDrawable` (extension error, base + 2) |
| unknown / destroyed XID | **core `BadDrawable` (9)** |

yserver falls back to `window()` then `pixmap()` and always replies
Success (`process_request.rs:10647-10659`, `12008-12034`). Today that
ships an obviously-bogus `GLX_FBCONFIG_ID = 0`; once D2 removes it, an
unknown or destroyed XID would get a fully-formed, superficially
conformant `WIDTH=0/HEIGHT=0` reply — a silent-success regression, which
the project's silent-success audit forbids.

So `GET_DRAWABLE_ATTRIBUTES` must emit **both** codes, per the table:
a naked X pixmap gets `GLX_FIRST_ERROR + 2` (`GLXBadDrawable`); an XID
that is not a drawable at all gets core `BadDrawable`. Collapsing them
into one code would be wrong for the majority case, and error codes are
what xts and real clients branch on.

`GLXBadDrawable = 2` (`glxproto.h:43`) is **absent** from
`yserver-protocol/src/x11/glx.rs` — the only GLX error constants today
are `ERROR_GLX_BAD_PIXMAP = 3`, `ERROR_GLX_BAD_RENDER_REQUEST = 6` and
`ERROR_GLX_UNSUPPORTED_PRIVATE_REQUEST = 8` (`glx.rs:73-75`).

### D6 — return tag 0 on release

Return `contextTag = 0` when the request carries a null context. This
requires parsing the request bodies, which the arm does not do today. The
two minors differ (`glxproto.h:225-233`, `glxproto.h:471-481`):

| minor | 0..4 | 4..8 | 8..12 | 12..16 |
|---|---|---|---|---|
| 5 `MakeCurrent` | drawable | context | oldContextTag | — |
| 26 `MakeContextCurrent` | oldContextTag | drawable | readdrawable | context |

Returning 0 is server-side safe: yserver reads an incoming `contextTag`
nowhere — the only occurrences are the reply write
(`process_request.rs:11783-11787`) and a comment marking the
vendor-private tag informational (`process_request.rs:12064`). No
lookup-by-tag exists.

## Deferred — recorded here, implemented elsewhere

Both were in rev 2 and were cut on scope. `docs/high-level-design.md:34`
lists **"supporting indirect or remote GLX"** as an explicit non-goal,
and `:197` states that coverage is "what real clients actually drive".
Neither defect is abandoned; both belong in a follow-up.

### D7 (deferred) — `QueryContext` returns zero attributes

`process_request.rs:12047-12056` replies with an empty attribute list.
Xorg returns five properties (`glxcmds.c:1660-1677`, `GLX_QUERY_NPROPS`):
`GLX_SHARE_CONTEXT_EXT`, `GLX_VISUAL_ID_EXT`, `GLX_SCREEN_EXT`,
`GLX_FBCONFIG_ID`, `GLX_RENDER_TYPE`. `glXQueryContext` therefore returns
uninitialised values to any client that calls it.

**Why deferred:** `QueryContext` has **zero calls across every captured
log** — no client in this investigation ever drove it, and
`glXQueryContext` is largely indirect-era API. It also cannot be
implemented before D8, since the state it would report is wrong.

Whoever picks this up must also handle the two body layouts, which
differ: minor 25 has `context` at `body[0..4]` (`glxproto.h:461-466`),
while the `QueryContextInfoEXT` vendor-private form
(`X_GLXvop_QueryContextInfoEXT = 1024`, `glxproto.h:2570`) has it at
`body[8..12]` (`glxproto.h:920-928`). Xorg routes both to the same
`DoQueryContext` (`glxcmds.c:1697-1718`); yserver handles only minor 25.

Also already established, so the follow-up need not re-derive it:
- **The existing encoder is reusable.** `encode_get_drawable_attributes_reply`
  works for this reply too — verified against `glxproto.h:885-894` and
  `:933-945`: both replies are 32 bytes, `type / unused / sequenceNumber
  / length / <count> / pad`, with `length = n << 1`. Renaming it to
  something neutral is then worthwhile (four call sites today:
  `process_request.rs:12023, 12050, 12130, 12336`).
- `GLX_SHARE_CONTEXT_EXT` is **absent** from `yserver-protocol/src/x11/glx.rs`.
- `GLX_SCREEN_EXT` is 0. `GLX_VISUAL_ID_EXT` is the `GLX_VISUAL_ID` of the
  matching `synthesise_glx_fb_configs` entry, 0 when unknown — Xorg's own
  null-config fallback (`glxcmds.c:1671`). Call
  `synthesise_glx_fb_configs(false)`, as `glx_fbconfig_depth` already
  does; the `(visual, fbconfig)` pairs do not depend on the TFP flag.
- **Test trap:** the config table is `(visual 0x102, fbconfig 0x101)` and
  `(visual 0x103, fbconfig 0x103)` (`process_request.rs:10765,10768`).
  The second entry's two ids are numerically identical, so a test built
  on `0x103` passes even if the mapping is a no-op. Use `0x101 → 0x102`.

### D8 (deferred) — `CreateContext` misparses all three request layouts

Found by review round 1, not by the original investigation.
`process_request.rs:11728-11758` handles `CREATE_CONTEXT`,
`CREATE_NEW_CONTEXT` and `CREATE_CONTEXT_ATTRIBS_ARB` with **one**
assumed layout — `[0..4] xid, [4..8] fbconfig, [8..12] render_type` — and
its comment at `11732-11733` asserts that layout. The real layouts,
body-relative (`/usr/include/GL/glxproto.h:197-208, 444-456, 1340-1354`):

| minor | 0..4 | 4..8 | 8..12 | 12..16 | 16..20 | 20..24 |
|---|---|---|---|---|---|---|
| 3 `CreateContext` | context | **visual** | screen | shareList | isDirect | — |
| 24 `CreateNewContext` | context | fbconfig | **screen** | renderType | shareList | isDirect |
| 34 `CreateContextAttribsARB` | context | fbconfig | **screen** | shareList | isDirect | numAttribs |

So `body[8..12]` is `screen` in every case: `GlxContext::render_type`
(`server.rs:1179`) always holds the screen number, which is 0 on this
single-screen server — it is never a render type. And for minor 3,
`GlxContext::fbconfig` holds a **visual id**, not an FBConfig id.

**Why deferred:** nothing in production reads either field, so D8 is
**inert** — the only reader anywhere is a test assertion at
`process_request.rs:52905`, and that is on the correctly-parsed SGIX path
below. It stops being inert the moment D7 reports them, which is exactly
why it was pulled in as D7's prerequisite and exactly why cutting D7 lets
it go too. Note this is a cut on *inertness*, **not** on lack of client
traffic: `CreateContext` is driven 7 times across the captured logs, more
than `MakeCurrent`'s 6. Task 5 records it in `docs/status.md` so it is not
rediscovered.

**Only three of the four `glx_contexts` insert sites are wrong.**
`process_request.rs:12212-12220` — `CreateContextWithConfigSGIX` (vendor
code 65541) — **already parses correctly** via
`parse_create_context_with_config_sgix` (fbconfig at `body[12..16]`,
renderType at `body[20..24]`) and is covered by
`create_context_with_config_sgix_inserts_context()`
(`process_request.rs:52844`). Do not "fix" it.

Two traps for whoever implements it, both already verified:
- `renderType` for minor 34 comes from the attribute list, defaulting to
  `GLX_RGBA_TYPE` (`glx/createcontext.c:90`, `:184-191`, `:353`). Xorg
  also rejects the attrib with `BadValue` when `req->fbconfig == 0`
  (`:188-189`) and validates via `validate_render_type` (`:66-78`).
- **`GLX_RGBA_TYPE` is `0x8014`**, not 1. `glx.rs` has only
  `GLX_RGBA_BIT = 0x1` (`glx.rs:376`), and the comment at `glx.rs:1127`
  already miscalls a `renderType` of 1 "`GLX_RGBA_TYPE`" — that comment
  is wrong and should be fixed with this work.

## What this design does NOT claim

- **It does not claim to fix the NVIDIA `BadAlloc`.** The A/B run in
  `ab-fbconfig/` was confounded: the naked arm had a bad fbconfig and
  good geometry, the GLXWindow arm the reverse, so both arms failed and
  neither tested a correct reply. Whether a fully Xorg-conformant
  `GetDrawableAttributes` unblocks `libGLX_nvidia` is **unknown** and is
  precisely what landing D1-D5 lets us measure.
- It does not touch the hardcoded `GLX_VENDOR_NAMES_EXT = "mesa"`
  (`glx.rs:172`), the suppressed `GLX_EXT_texture_from_pixmap` on NVIDIA
  (`kms/render/backend.rs:737`), or the 2-entry FBConfig list. Separate
  open defects in the same investigation.

## Risk and compatibility

The load-bearing risk is Mesa regression: comments at
`process_request.rs:10634-10636` and `10643-10644` record that Mesa's
`loader_dri3` consults `GLX_TEXTURE_TARGET_EXT`, `GLX_Y_INVERTED_EXT`,
`GLX_FBCONFIG_ID`, `GLX_WIDTH` and `GLX_HEIGHT`; `#96` records that
ANGLE/Chromium behaviour is sensitive here.

D2 (dropping `GLX_FBCONFIG_ID` for naked windows) is the change most
likely to regress Mesa. In-tree blast radius is zero — review confirmed
no existing test asserts the drawable attribute set or order (the one
"first attrib is `GLX_FBCONFIG_ID`" assertion,
`yserver-protocol/src/x11/glx.rs:1089-1093`, is about
`GetFBConfigsSGIX`). The supporting argument is that Mesa has always run
against Xorg, which does not send it here — an argument, not a
measurement. Mesa on this box currently falls back to llvmpipe
(`glx: failed to create dri3 screen`), so a local Mesa run exercises the
software path only and is a weak signal. Chromium/ANGLE (#96) and Firefox
are the useful client-side checks.

The error arm is the second-riskiest change: it turns a previously-Success
path into an error. It is what Xorg does, but any client relying on
yserver's laxity will now see an error.

It also makes a **pre-existing, out-of-scope gap client-visible**:
yserver does not implement `X_GLXCreateGLXPixmap` (minor 13) — no
constant, no match arm, so it falls to the `other =>` default at
`process_request.rs:12363` and returns `GLXBadRequest`. A GLX 1.2 / TFP
client that nonetheless queries the GLXPixmap XID it was handed today
gets a bogus-but-Success 0×0 reply; after this change it gets
`GLXBadDrawable`. That is more honest, but the new failure's root cause
is the missing minor 13, not this change — do not mistake it for a
regression introduced here.

D4 is low risk — Xorg never sent it, so no client can depend on it.

## Testing

- Unit, `drawable_attributes_for`: GLXWindow, GLXPixmap, pbuffer and
  naked X window, asserting the exact attribute set **and order** against
  `glxcmds.c:1890-1914`. The GLXWindow geometry case must be verified to
  fail against current code (it reports 0×0).
- **Request-level wire tests** (not just the helper): drive
  `GET_DRAWABLE_ATTRIBUTES` through `process_request` and assert the
  reply bytes — `numAttribs`, `length = 2n`, byte order — plus **both**
  error arms separately, per the error-arm table: a naked X pixmap →
  `GLX_FIRST_ERROR + 2` (`GLXBadDrawable`), an XID that is not a drawable
  at all → **core `BadDrawable` (9)**. `drawable_attributes_for`
  returns a `Vec` with no error channel, so helper-level tests alone
  cannot catch a non-conformant reply. Follow the existing convention in
  `glx_create_pixmap_records_x_drawable_and_destroy_clears_it`
  (`process_request.rs:52040`).
- Unit: `MakeCurrent` release returns tag 0, non-null context nonzero,
  for both minor layouts.
- On-hardware: rerun `~/yserver-glx-probes/run-probe-ab.sh` and record
  whether `nvidia-glxwindow` still yields `BadAlloc` with a fully
  conformant reply. This is the measurement the design exists to enable.
- Regression: a Plasma/Chromium session on yserver to confirm Mesa and
  ANGLE still initialise.

## Review history

- **Round 1 (Opus, 2026-08-03): REJECT.** Five blocking findings, all
  confirmed and all fixed in this revision: every `process_request.rs` /
  `server.rs` citation was written against the unmerged
  `present-deferred-supersession` branch while the plan declared `master`
  as its base (B1); D7 rested on the false premise that `GlxContext`
  carries a usable `fbconfig`/`render_type`, which D8 documents (D8 is
  now deferred) (B2); "mirror Xorg exactly" contradicted retaining the
  client-override pass (B3); Task 2 named one of three
  `glx_drawables.insert` sites (B4); the error arm was absent (B5).
  Non-blocking N1-N8 also applied, including capturing the
  previously-uncaptured XWayland control.
- **Round 2 (Opus, 2026-08-03): APPROVE WITH EDITS.** All five round-1
  blockers verified closed; the reviewer re-derived every citation
  independently and found no wrong line number. Three new blockers, all
  in behaviour rev 2 introduced. **B1 applied:** the error arm named one
  error code where Xorg observably emits two, and picked the wrong one
  for the majority case — GLXVND, not `glxcmds.c`, is the front door.
  B2 and B3 both landed inside D7/D8 and were dissolved by the scope cut
  below rather than fixed in place; what survives of B2 is
  `ERROR_GLX_BAD_DRAWABLE = 2`, which is now in the error arm. The traps
  B2/B3 identified (`GLX_RGBA_TYPE = 0x8014` vs `GLX_RGBA_BIT = 0x1`, the
  two `QueryContext` body layouts, vendor code 1024) are preserved in
  "Deferred" so the follow-up does not have to rediscover them.
- **Round 3 (Opus, 2026-08-03): APPROVE WITH EDITS → applied, approved.**
  One blocker: the Testing section still assigned `GLXBadDrawable` to the
  unknown-XID case, contradicting the error-arm table three sections
  above — round-2 B1 surviving in the one place an implementer reads to
  know what to assert. Fixed. Non-blocking N1-N6 also applied: the
  `high-level-design.md` citation split into `:34` and `:197`, four
  bullets added to make "Deferred" self-sufficient (including that a
  *fourth* `glx_contexts` insert site already parses correctly and must
  not be "fixed"), and the D7/D8 cut rationale de-conflated. The reviewer
  independently recounted the GLX request mix across all six captured
  server logs and confirmed every figure in the scope decision.

## Scope decision (2026-08-03, resolved)

Prompted by the question of whether this work fits the project's
mandates. Weighed against `docs/high-level-design.md:34`, which lists
**"supporting indirect or remote GLX"** as an explicit non-goal, and
`:197`, which says coverage is "what real clients actually drive":

- D1-D5 and the error arm touch `GetDrawableAttributes`, the most-driven
  GLX request in every captured log (30 calls, more than any other).
  Squarely mandated.
- **D7 has zero calls across every captured log**, and `glXQueryContext`
  is largely indirect-era API. D8 existed only as its prerequisite and is
  otherwise inert.
- D6's `contextTag` exists to label *indirect* rendering requests —
  `process_request.rs:11778-11781` says as much. It is the weakest item
  retained; kept because `MakeCurrent` **is** driven by real clients (6
  calls) and the fix is small and unambiguous.

**Decided: D7 and D8 are cut**, recorded above under "Deferred"; Task 5
records them in `docs/status.md`.

Note the two cuts rest on **different** grounds, and conflating them
would overstate the case: D7 is cut for lack of client traffic (zero
calls), D8 for **inertness** — its arm *is* driven, 7 times, more than
`MakeCurrent`, but nothing in production reads what it mis-stores.
