# GLX vendor names derived from the render driver — design

**Status:** rev 4, 2026-08-06. Rev 2 closed Opus adversarial review
round 1 (REJECT, all findings applied). Rev 3 restructured D2 so the
wiring defect rev 2 identified is closed by the type system rather than
documented, and re-verified every citation against source. Rev 4 applies
the adversarial round run against rev 3: overstated evidence withdrawn,
an unverified universal claim scoped to what was checked, and the
document's two separable changes declared as such.

**This spec contains two separable changes.** See "Severability".
**Base:** branch `glx-reply-xorg-alignment` @ `bd168cf`.
**All yserver line numbers in this document are against that base**
and were re-checked on 2026-08-06 (see "Citation audit").
**Scope:** `crates/yserver-protocol/src/x11/glx.rs`,
`crates/yserver-core/src/core_loop/process_request.rs`,
`crates/yserver-core/src/server.rs`,
`crates/yserver-core/src/backend/trait_def.rs`,
`crates/yserver-core/src/nested.rs`,
`crates/yserver/src/lib.rs`,
`crates/yserver/src/kms/render/backend.rs`,
`docs/status.md`.

## Severability

This document specifies two changes with independent justifications, and
the plan must order them as separate tasks with the refactor **first**:

1. **The `BackendCapabilities` refactor (D2).** Collapses two duplicated,
   untestable backend→state snapshot blocks into one type-enforced seam.
   It touches `dpms_capable` and `glx_tfp_supported`, **neither of which
   is defect A**, and it closes a pre-existing `glx_tfp_supported` wiring
   gap. Justified on its own; testable on its own; carries no hardware
   precondition.
2. **The GLX vendor derivation (D1, D3, D4, D5).** Defect A proper.
   Gated on the Plasma measurement under "Precondition to merge".

**Why this matters and is not bookkeeping.** The precondition in (2) can
fail — defect B is open and unmeasured, and the whole point of that gate
is that we do not yet know whether KWin survives. If (2) is held back,
(1) must be able to land without it. Entangling them would hold a change
that is independently correct behind a hardware result it does not
depend on.

It also keeps the record honest: AGENTS.md:18 squash-merges this branch,
and a squash titled for GLX vendor names that silently contains a core
wiring refactor misreports what landed.

## What changed in rev 4

1. **Overstated evidence withdrawn.** Rev 3 cited `glxscreens.h:150`
   (`char *glvnd`) as structural proof that Xorg emits one name. A
   `char *` proves nothing of the sort; the claim rests on the
   assignment sites alone, which rev 2 already had. Downgraded to
   context — in the one section whose purpose is rigor.
2. **An unverified universal negative scoped.** Rev 3's "no X client
   parses this string itself" is not something reading one library can
   establish, and it carried the AGENTS.md:19 burden. Narrowed to what
   was checked, with the residual exposure named.
3. **Severability declared** (above), with the refactor ordered first in
   the plan.
4. **`from_backend` moved** out of `trait_def.rs` — startup policy does
   not belong in a contract module — into `backend/mod.rs`.
5. **Module homes and test homes named**, which rev 3 left ambiguous.
6. **`docs/status.md` gained its second entry**, for the seam change.
7. The `with_*` naming question is **resolved as a non-issue**, with the
   reasoning recorded so it is not re-raised.

## What changed in rev 3

1. **D2 replaced.** Rev 2 carried the vendor across the backend seam as
   a field assigned at two call sites, and its own Testing section
   admitted the resulting hole: an implementer who forgets
   `lib.rs:337` passes every unit test and still ships `"mesa"` on
   NVIDIA. Rev 3 makes that omission a compile error
   (`BackendCapabilities`, below).
2. **D1 simplified.** No cached field on `KmsBackend`, so
   `for_tests_seed` is untouched.
3. **D4 gained the comment rewrite** it needs and rev 2 missed.
4. **"Unverified here" is gone.** All six Xorg citations and the
   libglvnd claim the divergence rests on were verified from source on
   this box. Rev 2 asserted the Xorg checkout was not available here;
   it is (`~/Projects/xserver` @ `5541a5c8`), and libglvnd's source
   ships in Gentoo's distfiles. Rev 2 was right about `codex`.

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
to `probe_dmabuf_export_support` (`backend.rs:698`). Two existing
precedents for a per-driver policy keyed on `vk::DriverId`:
`scanout_prefers_linear` (`crates/yserver/src/kms/vk/scanout.rs:922`),
and — closer in shape, a one-line binary `matches!` with the rationale
in its doc comment — `VkContext::supports_dri3_syncobj`
(`crates/yserver/src/kms/vk/device.rs:88`).

```rust
fn glx_vendor_names_for_driver(driver_id: ash::vk::DriverId) -> &'static str {
    if matches!(driver_id, ash::vk::DriverId::NVIDIA_PROPRIETARY) {
        "nvidia mesa"
    } else {
        x11glx::VENDOR_NAMES
    }
}
```

**No cached field.** `KmsBackend` overrides the trait getter and
computes there, beside `supports_dmabuf_export` (`backend.rs:12670`):

```rust
fn glx_vendor_names(&self) -> &'static str {
    self.platform
        .vk
        .as_ref()
        .map_or(x11glx::VENDOR_NAMES, |vk| glx_vendor_names_for_driver(vk.driver_id))
}
```

**`platform.vk` is an `Option`** (`kms/render/platform.rs:567`,
`Option<Arc<VkContext>>`, no `cfg` gate) — a `KmsBackend` with no Vulkan
context is representable — so the `None` arm is supplied here. The access
shape mirrors `dmabuf_export_supported` at `backend.rs:1164-1167`, which
reads `platform.vk` the same way; `platform` is moved intact into `Self`
at construction and stays reachable as `self.platform`.

Why compute rather than cache, unlike `dmabuf_export_supported`
(`backend.rs:602`): that field caches `probe_dmabuf_export_support`,
which does real Vulkan work. This is a `matches!` on an enum, and under
D2 the getter is called **exactly once per server lifetime**. Caching it
would buy nothing and would cost an initialiser in the second
constructor, `KmsBackend::for_tests_seed` (`backend.rs:2049`), which
this design therefore does not touch.

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

#### The wiring defect this replaces

Rev 2 assigned the field at two sites — `crates/yserver/src/lib.rs:337`
and `crates/yserver-core/src/nested.rs:417` — and its Testing section
named the resulting hole without closing it: an implementer who adds
every other piece but forgets `lib.rs:337` passes every unit test while
the server still ships `"mesa"` on NVIDIA.

That hole is not incidental to this change. It is the **shape of the
surrounding code**, measured 2026-08-06:

```
lib.rs:336-337     (pub fn run)   state.dpms = DpmsState::new(backend.dpms_capable());
                                  state.glx_tfp_supported = backend.supports_dmabuf_export();
nested.rs:416-417  (pub fn run)   state.dpms = DpmsState::new(backend.dpms_capable());
                                  state.glx_tfp_supported = backend.supports_dmabuf_export();
```

Two adjacent, duplicated backend→state snapshot blocks, inside two
`run()` functions no test can call: `yserver::run` needs a DRM device
and a VT, `yserver_core::nested::run` needs a host X server.
**`glx_tfp_supported` already carries this defect today** — rev 2 noted
as much and said "do not copy that gap", then copied it.

#### `BackendCapabilities`

In `server.rs`, a plain data struct with no dependency on `Backend`:

```rust
pub struct BackendCapabilities {
    pub dpms_capable: bool,
    pub glx_tfp_supported: bool,
    pub glx_vendor_names: String,
}
```

The constructor goes in **`crates/yserver-core/src/backend/mod.rs`**,
which already defines small backend-adjacent types (`OriginContext`) and
re-exports the trait:

```rust
impl BackendCapabilities {
    pub fn from_backend(backend: &dyn Backend) -> Self {
        Self {
            dpms_capable: backend.dpms_capable(),
            glx_tfp_supported: backend.supports_dmabuf_export(),
            glx_vendor_names: resolve_glx_vendor_names(
                backend.glx_vendor_names(),
                std::env::var("YSERVER_GLX_VENDOR").ok().as_deref(),
            ),
        }
    }
}
```

**The split of struct and constructor across two modules is
deliberate.** `server.rs` must not import `Backend`, because
`trait_def.rs` already imports `ServerState` and several trait methods
take `&mut ServerState` (`trait_def.rs:400,404,407`). Keeping the data
type free of the trait keeps that edge one-directional: `backend`
depends on `server`, not the reverse.

**Not `trait_def.rs`, which rev 3 chose.** That module defines the
contract and reads no environment today. Core does read `YSERVER_*`
elsewhere — `core_loop/run.rs:113`, `core_loop/client_reader.rs:124`,
`core_loop/damage_fanout.rs:123,250`,
`core_loop/process_request.rs:740,8985` — so an env read in this crate
is not itself unprecedented, but every one of those sits in a behaviour
module. Startup policy does not belong in the trait definition.

**Not `backend/params.rs`** either, despite the name: that module is
explicitly the other direction — "snapshots of state that are resolved
by yserver-core once per request and passed to the backend"
(`params.rs:1-3`). `BackendCapabilities` flows backend → core, once at
startup.

`ServerState::with_randr_outputs_and_modes` (`server.rs:1363`) and
`ServerState::with_randr_outputs` (`server.rs:1355`) take it as a
**required** parameter; the latter forwards its own to the former
(`server.rs:1357`).

**No rename, and the reason is worth recording** so a reviewer does not
raise it twice. The `with_*` family appears to name its parameters —
`with_geometry(width, height)`, `with_randr_outputs(width, height,
outputs)` — which would make a trailing unnamed `caps` a break. It does
not: `width`/`height` are already absent from two of the three names.
The convention names the **differentiator from the base constructor**,
and `capabilities`, like geometry, becomes common to both randr
constructors rather than distinguishing them. `with_randr_outputs` vs
`with_randr_outputs_and_modes` still differ by exactly what their names
say.

**This is cheap because of who calls those constructors.** Measured
2026-08-06, they have exactly one call site each, and they are the two
`run()` functions:

```
crates/yserver/src/lib.rs:324        ServerState::with_randr_outputs_and_modes(...)
crates/yserver-core/src/nested.rs:409  ServerState::with_randr_outputs(...)
crates/yserver-core/src/server.rs:1357 Self::with_randr_outputs_and_modes(...)   (internal)
```

The 617 call sites of `ServerState::new` — the test constructor — are
untouched.

#### What the entry points become

Both `run()` bodies lose lines. `lib.rs`, replacing 324 and 336-337:

```rust
let caps = BackendCapabilities::from_backend(&backend);
let mut state = ServerState::with_randr_outputs_and_modes(
    fb_w, fb_h, randr_outputs, randr_mode_table, caps);
crate::clock::init(state.start_instant);
install_backend_root_bindings(&mut state, &backend);
```

The two `state.dpms` / `state.glx_tfp_supported` assignments are
deleted; the constructor performs them, wrapping `dpms_capable` in
`DpmsState::new` as the entry points do today. Ordering holds:
`backend` is built at `lib.rs:318`, well before the constructor call,
and `crate::clock::init` stays after it, unchanged. `nested.rs:409` is
the same move.

#### Why this closes it

Omitting the wiring stops being expressible: a `ServerState` built by
either entry point cannot exist without `BackendCapabilities`, and a
`BackendCapabilities` cannot be built without going through
`from_backend`. A future capability added to the struct fails to compile
at the struct literal in `from_backend` — which is where it should fail.
Two drifting sites collapse to one, and the pre-existing
`glx_tfp_supported` gap closes with it.

`resolve_glx_vendor_names` lives in **`crates/yserver-core/src/backend/mod.rs`**,
module-private beside its only caller, and depends on nothing but `std`.
Its unit tests go in that module's `#[cfg(test)]` block, alongside the
`BackendCapabilities::from_backend` tests; the `ServerState` constructor
tests go in `server.rs`, where the constructors are.

### D3 — `YSERVER_GLX_VENDOR` override

Modelled on `YSERVER_SCANOUT_MODIFIER`'s pure parse function
(`crates/yserver/src/kms/vk/scanout.rs:1027`). The rationale recorded at
`scanout.rs:988-1003` transfers verbatim — a policy inferred from a
handful of machines, where "which vendor actually works on THIS card"
can only be answered by pointing the server somewhere other than where
the policy points, on hardware the maintainers may not own.

```rust
fn resolve_glx_vendor_names(derived: &str, raw_env: Option<&str>) -> String
```

The env value is a **parameter, not a `std::env` read inside the
function**, so the accepted spellings are testable without mutating
process environment — env mutation races under a parallel test runner.
`from_backend` is the one place that reads `std::env`.

That knob's `OnceLock`-cached reader (`scanout.rs:1051`) is **not**
copied. It exists because scanout modifiers are resolved repeatedly;
`from_backend` runs once per server lifetime, so the read is naturally
single and the log line it emits is naturally one line.

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

**The comment above that arm must be rewritten** — rev 2 changed the
code under it and left it standing. `process_request.rs:11658-11664`
currently reads, in part, *"returning `"mesa"` (matching Xorg) makes
libglvnd load libGLX_mesa"*, which stops being true the moment the arm
reads state. The rationale underneath it is still live, so it is
rewritten rather than dropped: the Asahi/cogl `SIGSEGV` is still averted,
now by the non-NVIDIA arm of D1 rather than by a constant.

```rust
// libglvnd vendor-neutral dispatch: tells the client which
// libGLX_<vendor>.so drives this screen. Resolved once at startup
// from the render driver (`BackendCapabilities::from_backend`);
// every non-NVIDIA driver keeps `VENDOR_NAMES` ("mesa"), which is
// what stops libglvnd from falling back to a vendor that resolves
// to nothing on Asahi → NULL glXQueryExtensionsString → cogl
// SIGSEGV. Only queried because we advertise GLX_EXT_libglvnd.
```

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
`FALLBACK_VENDOR_NAME = "indirect"` only when all fail. **Verified from
libglvnd 1.7.0 source on 2026-08-06** — see "Citation audit".

**The scope of that verification, stated precisely.** What was checked is
what libglvnd does with the reply. What was *not* checked — and cannot
be, by reading one library — is that libglvnd is the only consumer. Rev
3 claimed "no X client parses this string itself"; that is an unverified
universal negative, and it sat in the exact sentence carrying the
AGENTS.md:19 burden. Any client on raw xcb-glx can issue the query — our
own `rawglx` probe does — so the honest claim is narrower: **libglvnd is
the only consumer we know of, and it handles a list as described.** The
residual exposure is a client that both queries `GLX_VENDOR_NAMES_EXT`
directly and assumes one name. None is known; none was ruled out.

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
(`2026-08-03-glx-reply-xorg-alignment-design.md:383`). The integration
test below is the designated one: against today's code the arm returns
`x11glx::VENDOR_NAMES` regardless of state, so it fails with `"mesa"`.
Rev 2 nominated the wiring test, which no longer exists.

**Wiring is no longer a testing problem.** Rev 2 needed a test to guard
an omission the compiler could not see, and could not write one that
reached `lib.rs:337`. Under D2 that omission does not compile, so the
tests below cover only what *can* be wrong while compiling.

**Unit, `yserver-core` (`BackendCapabilities`):**

- `from_backend` against a `Backend` double whose three getters all
  return non-default values yields those values — pinning that each
  field reads its own getter, the one mistake (a copy-paste crossing two
  fields) the struct literal cannot catch.
- `with_randr_outputs_and_modes` deposits all three into the
  `ServerState` it returns, `dpms_capable` arriving as a `DpmsState`.
- `with_randr_outputs` forwards its capabilities unchanged
  (`server.rs:1357`).

`RecordingBackend` (`recording.rs:354`) is the natural double, but any
of the three implementors works — the point is no longer *which* backend
but that the seam is exercised at all.

**Unit, `yserver` (`backend.rs`):**

- `glx_vendor_names_for_driver(NVIDIA_PROPRIETARY) == "nvidia mesa"`.
- A non-NVIDIA `DriverId` (e.g. `MESA_LLVMPIPE`, `INTEL_OPEN_SOURCE_MESA`)
  returns `VENDOR_NAMES`, pinning that the mapping stays binary.

**Unit, `yserver-core` (`resolve_glx_vendor_names`):**

- Empty and whitespace-only env input falls back to the derived value.
- A well-formed env value overrides the derived value.
- Absent env (`None`) yields the derived value.

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
  constant. **This is the test verified failing first** (see above).

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

AGENTS.md:6 requires `docs/status.md` to be kept current. Two entries,
matching the two severable changes:

1. **`YSERVER_GLX_VENDOR`.** `docs/status.md` is where env knobs are
   recorded in this repo — `YSERVER_SCANOUT_MODIFIER` is at
   `docs/status.md:5306`, and no man page exists. The new knob gets an
   equivalent entry, including the server-restart constraint from D3
   (the value is read by the server, unlike
   `__GLX_VENDOR_LIBRARY_NAME`, which takes effect per client launch).
2. **The `BackendCapabilities` seam.** Despite its "KMS render backend"
   title, `docs/status.md` covers core-protocol and input work too, and
   D2 changes how every backend capability reaches `ServerState` and
   closes a pre-existing `glx_tfp_supported` wiring gap. This entry
   belongs to task 1 under "Severability" and lands with it, not with
   the vendor work.

Recorded because it has been missed before on a branch whose plan
omitted the step.

## Citation audit — 2026-08-06

Rev 2 declared its Xorg and libglvnd citations unverifiable, on the
grounds that this box has neither the Xorg checkout nor the `codex` CLI.
**The first half of that was false.** `~/Projects/xserver` is checked out
here at `5541a5c8` — the same clone the Present Task 0 verification used
on 2026-08-01 — and libglvnd 1.7.0's source ships in Gentoo's distfiles
at `/var/cache/distfiles/libglvnd-1.7.0.tar.bz2`. Every citation below
was read from source on 2026-08-06. (`codex` is still unavailable; that
part stands.)

**Xorg, all six citations read and exact** — four locations below, plus
`glxscreens.h:150` discussed after them:

- `glx/glxscreens.c:425` — `if (pGlxScreen->glvnd)
  __glXEnableExtension(..., "GLX_EXT_libglvnd")`. Advertisement is gated
  on having a vendor.
- `glx/glxcmds.c:2433` — `case GLX_VENDOR_NAMES_EXT:` falls through to
  `default: return BadValue` when `pGlxScreen->glvnd` is NULL.
- `glamor/glamor_egl.c:919` and `hw/xwayland/xwayland-glamor-gbm.c:1719`
  — `gbm_device_get_backend_name`, skipped when the name is `"drm"`.
- `glamor/glamor_glx_provider.c:425` — `strdup("mesa")` as the fallback.

**What carries "Xorg never emits more than one name": the assignment
sites, and only those.** `glx/glxscreens.h:150` declares the field as
`char *glvnd`, and rev 3 cited that as structural confirmation. It is
not: a `char *` holds `"nvidia mesa"` as readily as `"mesa"`, so the
field type is compatible with one name and with many. The claim rests
entirely on the assignments at the three `glamor`/`xwayland` locations
above — `glamor_set_glvnd_vendor`, `xwl_screen->glvnd_vendor =`, and
`glamor_glx_provider.c`'s two `strdup`s — each of which stores exactly
one name. The field declaration is context, not evidence, and is
recorded here only so a later reader does not mistake it for proof a
second time.

**libglvnd 1.7.0, exact.** `src/GLX/libglxmapping.c:519-600`,
`__glXLookupVendorByScreen`:

```c
for (name = strtok_r(queriedVendorNames, " ", &saveptr); name != NULL;
     name = strtok_r(NULL, " ", &saveptr)) {
    vendor = __glXLookupVendorByName(name);
    if (vendor != NULL && !vendor->glxvc->isScreenSupported(dpy, screen))
        vendor = NULL;
    if (vendor != NULL) break;
}
...
if (!vendor) vendor = __glXLookupVendorByName(FALLBACK_VENDOR_NAME);
```

`FALLBACK_VENDOR_NAME` is `"indirect"` (`libglxmapping.c:63`). The split
is `strtok_r(..., " ", ...)`; a failed `isScreenSupported` nulls the
candidate and the loop continues; the fallback engages only after every
name fails.

**Consequence: the divergence is no longer reasoned, it is sourced.**
Rev 2's escape hatch — *"if libglvnd does not honour a multi-name reply
as described, the list is unjustified and D1 must return `"nvidia"`"* —
is closed in favour of the list.

**One finding not in rev 2**, from the same read: `libglxmapping.c:556`
consults `__GLX_VENDOR_LIBRARY_NAME` **before** querying the server, so
the client-side override keeps winning over whatever we send. The
existing workaround is unaffected by this change.

**yserver, 25 of 26 exact.** The one correction, applied in rev 3:
`for_tests_seed` initialises `dmabuf_export_supported: false` at
`backend.rs:2104`, not 2107. (D1 no longer touches that constructor
regardless.)
