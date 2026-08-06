# GLX vendor names derived from the render driver — design

**Status:** rev 2 (Opus adversarial review round 1 closed — REJECT,
all findings applied), 2026-08-05
**Base:** branch `glx-reply-xorg-alignment` @ `bd168cf`.
**All yserver line numbers in this document are against that base.**
Xorg and libglvnd citations are carried from the 2026-08-03
investigation and were **not** re-verified while writing this — see
"Unverified here".
**Scope:** `crates/yserver-protocol/src/x11/glx.rs`,
`crates/yserver-core/src/core_loop/process_request.rs`,
`crates/yserver-core/src/server.rs`,
`crates/yserver-core/src/backend/trait_def.rs`,
`crates/yserver-core/src/nested.rs`,
`crates/yserver/src/lib.rs`,
`crates/yserver/src/kms/render/backend.rs`,
`docs/status.md`.

## Discovery context

This is "defect A" of the NVIDIA GLX investigation. Defect C
(`BadAlloc` on `X_GLXMakeCurrent`) was fixed by the design in
`2026-08-03-glx-reply-xorg-alignment-design.md` and **measured resolved**
on 2026-08-05: with that branch built, a probe forced to the NVIDIA
client vendor reports `GL_VERSION: 4.6.0 NVIDIA 610.57.04`,
`GL_RENDERER: NVIDIA GeForce RTX 5060 Ti/PCIe/SSE2`, `direct=1`,
`RESULT: GL works`.

Evidence lives in
`~/yserver-glx-logs/2026-08-05-plasma-black/probe-run/`, captured with
`~/yserver-glx-probes/glxprobe`, NVIDIA 610.57.04, RTX 5060 Ti.

That result required `__GLX_VENDOR_LIBRARY_NAME=nvidia` on the client.
**Without it nothing changes**, and that is the defect this design
addresses.

**Severity: defect A is the last blocker to hardware GL by default, but
it is not sufficient for a working Plasma.** Defect B (below) breaks KWin
under *both* vendors today. Ranking the two: A is a correctness defect
against every client on the machine; B is a correctness defect against
compositors specifically. A is fixed here; B is not.

## Problem — measured on the wire

In the same run, `vendor-default.txt` (no vendor forced) and
`vendor-mesa.txt` are **byte-identical to the 2026-08-03 baseline**
(md5 `b45bacfb…` for both files in both directories):

```
pci id for fd 4: 10de:2d04, driver (null)
kmsro: driver missing
glx: failed to create dri3 screen
failed to load driver: nvidia-drm
...
GL_RENDERER:  llvmpipe (LLVM 22.1.8, 256 bits)
```

The default path lands on **software rasterisation** on a discrete
NVIDIA GPU. The cause is a single hardcoded constant.

`glx.rs:179`:

```rust
pub const VENDOR_NAMES: &str = "mesa";
```

returned unconditionally by the `GLX_VENDOR_NAMES_EXT` arm of
`QUERY_SERVER_STRING` (`process_request.rs:11665`). libglvnd therefore
loads `libGLX_mesa.so` on every screen yserver serves. Mesa has no DRI
driver for PCI `10de:2d04`, so its DRI3 screen creation fails and it
falls back to llvmpipe.

The server already holds the discriminating fact: `vk.driver_id`, which
`backend.rs:711` consults today to suppress `GLX_EXT_texture_from_pixmap`
on NVIDIA. It is simply never consulted for the vendor query.

Raw-wire confirmation that the constant is what reaches the client
(`raw-wire.txt`, xcb probe with no libglvnd in the path —
`~/yserver-glx-probes/rawglx.c`):

```
VENDOR_NAMES_EXT   -> "mesa" (n=5)
```

`raw-wire.txt` is itself byte-identical to the 08-03 capture, as expected
— the constant did not change between the runs. It is quoted here as the
current server behaviour, not as a fresh observation.

Note that querying the same value *through* libglvnd reports `(NULL)`
(`vendor-*.txt`, line `srv VENDOR_NAMES`). The probe calls
`glXQueryServerString` through the vendor dispatch
(`~/yserver-glx-probes/glxprobe.c:39-40`), which a vendor need not answer
for `0x20F6`. Only the raw-wire figure is trustworthy.

### The causal link is inferred, not observed

"Server sends `mesa` → libglvnd loads `libGLX_mesa.so`" is an inference.
No capture shows libglvnd issuing the `VENDOR_NAMES_EXT` query or acting
on the reply, and it currently **cannot**: the `QUERY_SERVER_STRING` arm
(`process_request.rs:11646-11673`) has no `debug!` call, unlike
`QUERY_VERSION` at `process_request.rs:11636`. The server log is
structurally incapable of showing the query this whole design rests on.

**D5 below closes this**, and it must land with the rest — otherwise the
post-landing verification run cannot distinguish "libglvnd honoured our
list" from "libglvnd guessed nvidia for unrelated reasons".

## Prior measurement of the target configuration

`~/yserver-glx-logs/2026-08-03-investigation/sabado-nvidia-forzado/`
(2026-08-01/02) is a full desktop session with
`__GLX_VENDOR_LIBRARY_NAME=nvidia` exported globally — the client-visible
end state this design produces server-side. Per that tree's `README.md`:
Plasma started and ran; **Steam and games crashed on launch**; Firefox and
Discord worked.

Two things follow, and both matter.

**The Steam regression has a named cause, and it is already fixed.**
`steam-juegos-nvidia/steam-console-linux.txt:63423-63426`:

```
X Error of failed request:  BadAlloc (insufficient resources for operation)
Major opcode of failed request:  148 (GLX)
Minor opcode of failed request:  5 (X_GLXMakeCurrent)
Serial number of failed request:  0
```

That is defect C's exact signature — the one the base branch fixes and
the 08-05 probe confirms resolved. The Steam breakage is therefore
expected to be gone, but that is a **hypothesis requiring
re-measurement**, not a settled fact: Steam is a 32-bit client and was
never re-run against the fixed branch.

**KWin got no GL context under the forced NVIDIA vendor**, which is why
it never reached the TFP path. `sabado-nvidia-forzado/plasma-client.log:72-77`:
`QGLXContext: Failed to create dummy context` /
`kwin_scene_opengl: Creating the OpenGL rendering failed`. That is defect
C again, and its removal is precisely what makes defect B newly reachable
(see Risk).

## Design

### D1 — derive the vendor list from `vk.driver_id`

New pure function in `crates/yserver/src/kms/render/backend.rs`, sibling
to `probe_dmabuf_export_support` (`backend.rs:698`) and modelled on
`scanout_prefers_linear`
(`crates/yserver/src/kms/vk/scanout.rs:922`), the existing precedent for
a per-driver policy keyed on `vk::DriverId`:

```rust
fn glx_vendor_names_for_driver(driver_id: ash::vk::DriverId) -> &'static str {
    if matches!(driver_id, ash::vk::DriverId::NVIDIA_PROPRIETARY) {
        "nvidia mesa"
    } else {
        x11glx::VENDOR_NAMES
    }
}
```

`KmsBackend` caches the result at construction. **`platform.vk` is an
`Option`** — a `KmsBackend` with no Vulkan context is representable — so
the call site mirrors `dmabuf_export_supported` at `backend.rs:1164-1167`
and must supply the `None` arm itself:

```rust
let glx_vendor_names = platform
    .vk
    .as_ref()
    .map_or(x11glx::VENDOR_NAMES, |vk| glx_vendor_names_for_driver(vk.driver_id));
```

`backend.rs:12670` is the *getter* in `impl Backend for KmsBackend`, not
a construction site; the new cached field needs a getter there too.

**There is a second constructor.** `KmsBackend::for_tests_seed`
(`backend.rs:2049`) initialises `dmabuf_export_supported: false` at
`backend.rs:2107`; the new field must be initialised there or the crate
does not compile.

No Vulkan call happens per query.

**The mapping is deliberately binary.** Entries for AMD proprietary,
Imagination, or any other `DriverId` are omitted because nobody working
on this repo can measure them, and an unmeasured mapping that redirects a
configuration that works today onto a nonexistent `libGLX_*.so` is worse
than the status quo. Every non-NVIDIA driver keeps today's `"mesa"`.

### D2 — carry it across the backend seam

`ash` is a dependency of the `yserver` crate only
(`crates/yserver/Cargo.toml:22`, resolving the workspace declaration at
the root `Cargo.toml:16`); `yserver-core` and `yserver-protocol` have
none, and 562 `ash::` references live in 44 files all inside `yserver`.
The `Backend` trait is the seam, so the value crosses it as a string,
mirroring how `glx_tfp_supported` crosses as a `bool`.

`trait_def.rs`, beside `supports_dmabuf_export` (`trait_def.rs:999`):

```rust
fn glx_vendor_names(&self) -> &'static str { x11glx::VENDOR_NAMES }
```

`&'static str`, not `&str`: both implementors return statics, and
borrowing `&self` for no reason would force an allocation the contract
does not need. Note that `trait_def.rs:15` imports
`yserver_protocol::x11::{AtomId, ClipRectangles, FontMetrics, xfixes}`
— **`glx` is not in scope there** (the `x11glx` alias is function-local
to `handle_glx_request`, `process_request.rs:11625`). Add `glx` to that
`use` or spell the path in full.

The default is what preserves current behaviour for the nested backend
and every other implementor without a line of code in them. Three
implementors exist — `KmsBackend` (`backend.rs:10853`), `HostX11Backend`
(`crates/yserver-core/src/host_x11/trait_impl.rs:20`), `RecordingBackend`
(`crates/yserver-core/src/backend/recording.rs:354`) — and only
`KmsBackend` overrides.

`server.rs`, beside `glx_tfp_supported` (`server.rs:1079`, default at
`server.rs:1334`):

```rust
pub glx_vendor_names: String,   // default: VENDOR_NAMES.to_string()
```

`String` rather than `Cow<'static, str>`: D3's override produces an owned
value anyway, so the `Cow` would be `Owned` on exactly the path anyone
cares about and the borrow-vs-own distinction buys one startup
allocation. Simpler field type wins.

Assignment sites, one line each, adjacent to the existing
`glx_tfp_supported` line — `crates/yserver/src/lib.rs:337` and
`crates/yserver-core/src/nested.rs:417`:

```rust
state.glx_vendor_names = resolve_glx_vendor_names(backend.glx_vendor_names());
```

`resolve_glx_vendor_names` lives in **`yserver-core`**, not `yserver`:
both call sites must reach it and `nested.rs` is on the core side. It
depends on nothing but `std` — string handling and one env read.

### D3 — `YSERVER_GLX_VENDOR` override

Modelled on `YSERVER_SCANOUT_MODIFIER`: a pure parse function
(`crates/yserver/src/kms/vk/scanout.rs:1027`) plus a `OnceLock`-cached
reader that logs once (`scanout.rs:1051`). The rationale recorded at
`scanout.rs:988-1003` transfers verbatim — a policy inferred from a
handful of machines, where "which vendor actually works on THIS card"
can only be answered by pointing the server somewhere other than where
the policy points, on hardware the maintainers may not own.

The decisive justification is the measured one, not the analogy: the
only full session ever run in this configuration regressed Steam and
games (see "Prior measurement"). Users on hardware nobody here owns need
a rollback lever that does not require recompiling the display server.

The value is the vendor string verbatim: `nvidia`, `mesa`,
`nvidia mesa`. No enum — the point is to permit names the code does not
know about.

Validation is trim-and-reject-empty only. A name with no matching
`libGLX_<name>.so` needs no server-side check: the client fails to load
it and libglvnd falls through to the next entry, which is the intended
experimental behaviour. As with the scanout knob, a typo must not keep
the display server from starting.

Precedence: **env > `driver_id` derivation > `"mesa"`**.

**Operational constraint:** this is read by the *server*, so changing it
requires restarting the display server — unlike
`__GLX_VENDOR_LIBRARY_NAME`, which takes effect on the next client
launch. Document that where the knob is documented.

### D4 — the reply arm

`process_request.rs:11665` becomes:

```rust
x11glx::VENDOR_NAMES_EXT => &state.glx_vendor_names,
```

This compiles. `handle_glx_request` takes `state: &mut ServerState`
(`process_request.rs:11616-11617`); `&state.glx_vendor_names` borrows the
place `(*state).glx_vendor_names` while `state.clients.get_mut(…)` at
`process_request.rs:11669` borrows the disjoint place `(*state).clients`.
Rust permits disjoint field borrows through a `&mut` reference
irrespective of liveness, so this is not resting on NLL. The hazard to
watch is not statement order — it is replacing either field access with
an accessor method, which would borrow all of `*state`.

`x11glx::VENDOR_NAMES` is **not** deleted. It becomes the documented
default, referenced by the trait default and by the non-NVIDIA arm of
D1.

### D5 — log the query

Add a `debug!` to the `VENDOR_NAMES_EXT` path, matching the convention
of `QUERY_VERSION` at `process_request.rs:11636`, reporting the requested
string id and the value returned.

This is not diagnostics polish. Without it the hardware verification
below cannot observe the query at all (see "The causal link is inferred").

## Divergence from Xorg — stated deliberately

AGENTS.md:19: *"Spec compliance is the goal, but if Xorg deviates from
spec (unlikely), we need to follow Xorg, clients are tested for 40+
years on Xorg."*

**Xorg never emits more than one vendor name.** It resolves a single
vendor (`glamor_egl.c:919`, `xwayland-glamor-gbm.c:1719`, with `"mesa"`
as the fallback at `glamor_glx_provider.c:425`) and sends that. This
design sends two.

**This divergence is discretionary, not forced.** `"nvidia"` alone fixes
defect A and is exactly Xorg-shaped. The case for the second entry has to
carry on its own merits, and it must clear both halves of AGENTS.md:19 —
the conditional *and* its rationale.

**On the conditional.** The rule scopes to "if Xorg deviates from spec".
`GLX_EXT_libglvnd` defines the `GLX_VENDOR_NAMES_EXT` reply as a
space-separated list; Xorg emits a one-element list. A subset is not a
contradiction, so there is no conflict for the rule to break.

**On the rationale**, which is the harder half and which rev 1 of this
document did not address: "clients are tested for 40+ years on Xorg"
means observed-Xorg behaviour is the only behaviour clients have been
exercised against, and a wire value of a shape no X server has ever
emitted is exactly the exposure that clause exists to prevent. The
answer is that the exposure has a single consumer with known handling —
libglvnd, whose `__glXLookupVendorByScreen` splits the reply with
`strtok_r(..., " ", ...)`, tries each name in order, moves on when a
vendor fails to load or its `isScreenSupported` returns False, and uses
`FALLBACK_VENDOR_NAME = "indirect"` only when all fail. No X client
parses this string itself. **This reasoning is unverified on this box
and the divergence rests on it** — see "Unverified here".

**What the second entry buys.** With a bare `"nvidia"` on a system where
the NVIDIA Vulkan ICD is installed but `libGLX_nvidia.so` is not — a
routine package split — libglvnd resolves no vendor and falls to
`"indirect"`, which is *worse* than today's llvmpipe. The `"mesa"` entry
is insurance against exactly that. **Reasoned, not measured.**

**Why this is not the same risk as a broader `DriverId` table**, which
D1 rejects on "nobody can measure it": the two are asymmetric against the
status quo. A broader table would redirect configurations that work today
onto unmeasured vendors — potentially worse than status quo. The second
`"mesa"` entry only ever engages on NVIDIA, where the fallback outcome is
*identical to today*. It is bounded below by the status quo; a speculative
table is not.

If a reviewer rejects the list anyway, D1 returns `"nvidia"` and nothing
else in this design changes.

## Risk and compatibility

### Defect B breaks KWin today, under both vendors

The status quo is **not** "Plasma runs on llvmpipe". Plasma already
crashes there, measured 2026-08-03 under the unmodified `"mesa"` default
this design replaces —
`~/yserver-glx-logs/2026-08-03-investigation/hoy-mesa-tfp-crash/plasma-client.log:103-105`:

```
No provider of glXBindTexImageEXT found.  Requires one of:
    GLX_EXT_texture_from_pixmap
Application::crashHandler() called with signal 6; recent crashes: 1
KCrash: Application 'kwin_x11' crashing... crashRecursionCounter = 2
```

`backend.rs:711` reports no dma-buf export on NVIDIA, so
`GLX_EXT_texture_from_pixmap` is unadvertised and KWin — which does not
take its pre-TFP fallback — aborts in libepoxy.

So this change does not create a new failure class. It **moves an
existing crash from the llvmpipe path to the NVIDIA path**, and defect B
blocks a working Plasma either way. Before defect C was fixed, KWin never
reached the TFP path under the NVIDIA vendor because it could not create
a context at all (`sabado-nvidia-forzado/plasma-client.log:72-77`); now
it can, so it will reach `glXBindTexImageEXT` sooner.

### Precondition to merge

**A Plasma session must be run on this branch and the outcome recorded,
before the branch merges.** Pass/fail: does `kwin_x11` survive under the
NVIDIA vendor with TFP unadvertised?

- If it survives — land, and defect B is downgraded for compositors.
- If it crashes — the change is still correct (defect A is real and
  independently measured), but it must land together with a defect-B
  decision, because "Plasma crashes" is then the shipped state on the
  default path rather than an opt-in one.

This is a gate, not a note. Rev 1 of this document filed it as a passing
remark and misstated the status quo it was reasoning about.

### Out of scope, recorded so it is not rediscovered

- ynest against an NVIDIA host Xorg keeps reporting `"mesa"` to its
  clients: defect A exists in nested mode too, and `HostX11Backend` could
  forward the host's own `GLX_VENDOR_NAMES_EXT`.
- `vk.driver_id` is the driver of the *server's render device*. On a
  PRIME/Optimus box the KMS node and the Vulkan device need not be the
  same vendor. Xorg's analogue derives the vendor from the same device it
  renders on, so the proxy is defensible, but it is an assumption.
- fbconfig synthesis is untouched: 2 FBConfigs against XWayland's 168
  (`synthesise_glx_fb_configs`).
- `QUERY_CONTEXT` still answers 0 attributes where Xorg answers
  `FBCONFIG_ID`/`RENDER_TYPE`/`SCREEN`.

## Error handling

No failure mode can abort startup. Every path resolves to a non-empty
vendor string:

| Condition | Result |
|---|---|
| `KmsBackend` with `platform.vk == None` | `"mesa"` via the `map_or` in D1 |
| Non-KMS backend (ynest, recording) | trait default → `"mesa"` |
| `driver_id` not NVIDIA | `"mesa"` |
| `YSERVER_ALLOW_SOFTWARE_VULKAN` forcing lavapipe on an NVIDIA box | `MESA_LLVMPIPE` → `"mesa"`, which is correct |
| `YSERVER_GLX_VENDOR` unset | derived value |
| `YSERVER_GLX_VENDOR` empty/whitespace | warn once, fall back to derived |
| `YSERVER_GLX_VENDOR=<garbage>` | sent verbatim; client-side load fails, libglvnd falls through |

`GLX_EXT_libglvnd` (`glx.rs:201-204`) continues to be advertised
unconditionally. Xorg gates that advertisement on having resolved a
vendor (`glxscreens.c:425`) and returns `BadValue` for the query when it
has none (`glxcmds.c:2433`); yserver's floor is `"mesa"`, so it can never
enter the vendorless state those two paths exist to handle. The
regression assertion at `glx.rs:938` stays as-is — it guards the
Cinnamon/cogl SIGSEGV on Asahi recorded at `glx.rs:933-937`.

## Testing

**At least one test must be verified failing against current code before
its fix lands**, per the practice of the preceding spec
(`2026-08-03-glx-reply-xorg-alignment-design.md:383`). The wiring test
below is the natural candidate.

**Wiring — the test that actually guards this defect.** The five-site
shape (`server.rs:1079`, `server.rs:1334`, `trait_def.rs:999`,
`nested.rs:417`, `lib.rs:337`) has a hole: an implementer who adds every
piece but forgets the assignment at `lib.rs:337` still passes every
isolated unit test, and the server still ships `"mesa"` on NVIDIA. A test
must drive backend → `ServerState`. `glx_tfp_supported` has the identical
shape and no such test today; do not copy that gap.

If driving the real `KmsBackend` proves impractical in a unit test, use
`RecordingBackend` (`recording.rs:354`) with an overridden
`glx_vendor_names`, assert the value reaches `ServerState`, and state
explicitly in the spec that the KMS path's wiring is covered **only** by
the manual tty2 run.

**Unit, `yserver` (`backend.rs`):**

- `glx_vendor_names_for_driver(NVIDIA_PROPRIETARY) == "nvidia mesa"`.
- A non-NVIDIA `DriverId` (e.g. `MESA_LLVMPIPE`, `INTEL_OPEN_SOURCE_MESA`)
  returns `VENDOR_NAMES`, pinning that the mapping stays binary.

**Unit, `yserver-core` (`resolve_glx_vendor_names`):**

- The parse function takes the raw value as a **parameter**, not from
  `std::env`, so the accepted spellings are testable without mutating
  process environment — env mutation races under a parallel test runner.
- Empty and whitespace-only input falls back to the derived value.
- A well-formed value overrides the derived value.

**Protocol (`glx.rs`):**

- The existing `vendor_names_query_reports_mesa` (`glx.rs:942`) keeps
  asserting `VENDOR_NAMES == "mesa"` — still the default — and gains a
  note that it no longer pins what goes on the wire.

  No `encode_string_reply` list test. `glx.rs:272-275` computes
  `n`/`padded`/`length_units` from `bytes.len()` with no space-aware
  branch, so a two-name list cannot regress independently of a one-name
  one. Rev 1 described such a test as a guard against "a length
  regression on the space"; there is no code path keyed on the space.

**Integration (`yserver-core`):**

- A `QUERY_SERVER_STRING`/`VENDOR_NAMES_EXT` request against a
  `ServerState` whose `glx_vendor_names` was set to a non-default value
  returns that value, pinning that the arm reads state rather than the
  constant. This complements the wiring test; it does not replace it.

**Hardware verification** (not automatable; the only checks that close
this defect):

1. Re-run `~/yserver-glx-probes/run-probe-session-0805.sh` from tty2 with
   this branch built. `vendor-default.txt` must report
   `GL_RENDERER: NVIDIA GeForce RTX 5060 Ti` with **no**
   `__GLX_VENDOR_LIBRARY_NAME` set, and `raw-wire.txt` must show
   `VENDOR_NAMES_EXT -> "nvidia mesa"`.
2. The server log must show the client issuing `VENDOR_NAMES_EXT` (D5).
   Without this the run cannot attribute the outcome to our reply.
3. A Plasma session — the merge precondition above.
4. A Steam launch, re-measuring the regression recorded in
   `sabado-nvidia-forzado/` now that defect C is fixed.

**Trap that cost a cycle on 2026-08-05:** that script runs `cargo build`
against **whatever branch is checked out**. Confirm the branch first.

## Documentation

`docs/status.md` is where env knobs are recorded in this repo —
`YSERVER_SCANOUT_MODIFIER` is at `docs/status.md:5306`, and no man page
exists. `YSERVER_GLX_VENDOR` gets an equivalent entry, including the
server-restart constraint from D3.

## Unverified here

The Xorg and libglvnd citations are carried from the 2026-08-03
investigation, conducted on the sandbox machine where `~/Projects/xserver`
is checked out. **This box has neither that checkout nor the `codex`
CLI**, so none were re-confirmed while writing this.

A reviewer with the checkout should re-check:

- `glxscreens.c:425` (advertisement gated on having a vendor) and
  `glxcmds.c:2433` (`BadValue` without one) — cited in Error handling.
- `glamor_egl.c:919`, `xwayland-glamor-gbm.c:1719`,
  `glamor_glx_provider.c:425` — cited in Divergence as evidence that Xorg
  resolves exactly one vendor.
- libglvnd's `__glXLookupVendorByScreen`, specifically: that the split is
  `strtok_r(..., " ", ...)`; that a failed `isScreenSupported` continues
  to the next name rather than aborting; and that `FALLBACK_VENDOR_NAME`
  is `"indirect"`.

**The divergence in this design rests entirely on that last item.** If
libglvnd does not honour a multi-name reply as described, the list is
unjustified and D1 must return `"nvidia"`.
