# GLX vendor names from the render driver — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop yserver reporting a hardcoded `"mesa"` for
`GLX_VENDOR_NAMES_EXT`, so libglvnd loads `libGLX_nvidia.so` on NVIDIA
hardware instead of falling back to llvmpipe.

**Architecture:** The vendor list is derived from `vk.driver_id` inside
the `yserver` crate, crosses the `Backend` trait seam as a
`&'static str`, and is snapshotted into `ServerState` at startup through
a new `BackendCapabilities` value that both entry-point constructors
require. The `GLX_VENDOR_NAMES_EXT` reply arm then reads state instead
of a constant.

**Tech Stack:** Rust 2024, `ash` (Vulkan, `yserver` crate only), `log`.

**Spec:** `docs/superpowers/specs/2026-08-05-glx-vendor-names-from-driver-design.md`
rev 4 (`e542e49`). Read it before starting. Every design decision here
is justified there; this document does not repeat the reasoning.

**Base branch:** `glx-vendor-names-from-driver` @ `e542e49`, itself cut
from `glx-reply-xorg-alignment` @ `bd168cf`. **Do not branch from
master** — this work depends on the defect-C fix in the base branch.

## Model assignment

- **Sonnet implements** each task.
- **Opus reviews** Sonnet's work between tasks. Tasks 1, 3 and 4 are the
  delicate ones and get a review round each; 2 and 5 can be reviewed
  together with their neighbours.
- Reviews happen **after** each task lands, never batched to the end.

## Global Constraints

- **Clippy exactly as CI runs it**, before every commit:
  `cargo clippy --all-targets -- -D warnings`. CI fails on any warning,
  and `--all-targets` lints test code too (AGENTS.md:12).
- **Formatting:** `cargo +nightly fmt` (AGENTS.md:13). Verified
  available on this box.
- **`ash` is a `yserver`-crate dependency only.** `yserver-core` and
  `yserver-protocol` have none. Nothing that mentions `ash::` or
  `vk::DriverId` may leave the `yserver` crate.
- **Commits must be signed** (the branch's existing commits are; `%G?`
  must report `G`).
- **Never put a Claude session URL in a commit message** (CLAUDE.md).
- **`docs/status.md` must be kept current** (AGENTS.md:6). Two entries
  are required, in Tasks 1 and 4 respectively.
- **The default stays `"mesa"` on every non-NVIDIA driver.** The mapping
  is deliberately binary; do not add `DriverId` arms nobody can measure.

## Severability

Per the spec's "Severability" section, this plan contains two
independently justified changes:

- **Task 1** is the `BackendCapabilities` refactor. It touches
  `dpms_capable` and `glx_tfp_supported`, neither of which is defect A,
  and closes a pre-existing wiring gap. It has **no hardware
  precondition** and may be merged on its own.
- **Tasks 2-6** are the GLX vendor derivation, gated on the hardware
  measurement in Task 6.

**Task 1 therefore does NOT add a `glx_vendor_names` field.** Task 2
adds it. That ordering is deliberate: it exercises the mechanism the
refactor exists for — adding a capability fails to compile at the struct
literal in `from_backend` until it is handled.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/yserver-core/src/server.rs` | `BackendCapabilities` type; `ServerState` fields; both randr constructors take capabilities | 1, 2 |
| `crates/yserver-core/src/backend/mod.rs` | `BackendCapabilities::from_backend`; `resolve_glx_vendor_names`; their tests | 1, 2 |
| `crates/yserver-core/src/backend/trait_def.rs` | `Backend::glx_vendor_names` contract + default | 2 |
| `crates/yserver-core/src/nested.rs` | ynest entry point call site | 1 |
| `crates/yserver/src/lib.rs` | KMS entry point call site | 1 |
| `crates/yserver/src/kms/render/backend.rs` | `glx_vendor_names_for_driver`; `KmsBackend` getter override | 3 |
| `crates/yserver-core/src/core_loop/process_request.rs` | `GLX_VENDOR_NAMES_EXT` reply arm; debug log; integration test | 4 |
| `crates/yserver-protocol/src/x11/glx.rs` | `VENDOR_NAMES` constant (unchanged) + test comment | 4 |
| `docs/status.md` | seam entry (Task 1); `YSERVER_GLX_VENDOR` knob entry (Task 4) | 1, 4 |

---

## Task 1: `BackendCapabilities` — make the backend→state snapshot unforgettable

**Severable. This task may be reviewed, merged and shipped without any
of the tasks that follow.**

**Why this exists:** `crates/yserver/src/lib.rs:336-337` and
`crates/yserver-core/src/nested.rs:416-417` are duplicated backend→state
snapshot blocks inside two `run()` functions no test can call —
`yserver::run` needs a DRM device and a VT, `yserver_core::nested::run`
needs a host X server. Nothing catches an omission at either site.

**Files:**
- Modify: `crates/yserver-core/src/server.rs` — add type near `DpmsState`
  (`server.rs:587`); change `with_randr_outputs` (`server.rs:1355`) and
  `with_randr_outputs_and_modes` (`server.rs:1363`)
- Modify: `crates/yserver-core/src/backend/mod.rs` — add `from_backend`
  and its tests
- Modify: `crates/yserver-core/src/nested.rs:409,416-417`
- Modify: `crates/yserver/src/lib.rs:324,336-337`
- Modify: `docs/status.md`

**Interfaces:**
- Produces: `yserver_core::server::BackendCapabilities { dpms_capable: bool, glx_tfp_supported: bool }`,
  `BackendCapabilities::from_backend(&dyn Backend) -> BackendCapabilities`,
  and the two constructors' new trailing `capabilities: BackendCapabilities`
  parameter. Task 2 adds a third field to the struct.

- [ ] **Step 1: Write the failing test**

In `crates/yserver-core/src/backend/mod.rs`, at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::BackendCapabilities;
    use crate::backend::recording::RecordingBackend;

    #[test]
    fn from_backend_reads_each_capability_from_its_own_getter() {
        // RecordingBackend's two capabilities differ by default —
        // `dpms_capable()` returns true (recording.rs:1526, a test
        // default so DPMS transition tests have something to drive)
        // while `supports_dmabuf_export()` is not overridden and
        // inherits the trait default, false. That asymmetry is what
        // makes this test able to catch a crossed assignment: swapping
        // the two lines in `from_backend` flips both asserts.
        let backend = RecordingBackend::new();
        let caps = BackendCapabilities::from_backend(&backend);
        assert!(caps.dpms_capable, "must come from dpms_capable()");
        assert!(
            !caps.glx_tfp_supported,
            "must come from supports_dmabuf_export()"
        );
    }

    #[test]
    fn randr_constructors_deposit_capabilities_into_server_state() {
        use crate::server::ServerState;

        let caps = BackendCapabilities {
            dpms_capable: true,
            glx_tfp_supported: true,
        };
        let state = ServerState::with_randr_outputs(800, 600, Vec::new(), caps.clone());
        assert!(state.dpms.kms_capable, "dpms_capable must reach DpmsState");
        assert!(state.glx_tfp_supported);

        // `with_randr_outputs` forwards to `with_randr_outputs_and_modes`
        // (server.rs:1357); pin that the forward does not drop them.
        let direct =
            ServerState::with_randr_outputs_and_modes(800, 600, Vec::new(), Vec::new(), caps);
        assert!(direct.dpms.kms_capable);
        assert!(direct.glx_tfp_supported);
    }
}
```

`DpmsState.kms_capable` is `server.rs:588`, set from `DpmsState::new`'s
argument (`server.rs:606-608`); `enabled` mirrors it, so either field
distinguishes `new(true)` from `new(false)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver-core --lib backend::tests -- --nocapture`

Expected: FAIL to **compile** — `BackendCapabilities` does not exist yet.
A compile failure is the correct "red" here.

- [ ] **Step 3: Add the type**

In `crates/yserver-core/src/server.rs`, immediately before
`pub struct DpmsState` (`server.rs:587`):

```rust
/// Backend-derived facts snapshotted into `ServerState` once at startup.
///
/// This type exists so the snapshot cannot be forgotten. Both
/// entry-point constructors take it by value, so a `ServerState` built
/// by `yserver::run` or `yserver_core::nested::run` cannot exist
/// without one. Before it, each `run()` assigned these fields by hand
/// and an omission at either site compiled, passed every unit test, and
/// silently shipped the defaults.
///
/// Deliberately free of any `Backend` dependency: `backend` depends on
/// `server`, not the reverse. The constructor that reads a `Backend`
/// lives in `crate::backend`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// From `Backend::dpms_capable`. Wrapped in `DpmsState::new`.
    pub dpms_capable: bool,
    /// From `Backend::supports_dmabuf_export`. Gates whether
    /// `GLX_EXT_texture_from_pixmap` is advertised.
    pub glx_tfp_supported: bool,
}
```

- [ ] **Step 4: Add the constructor**

In `crates/yserver-core/src/backend/mod.rs`, after the `pub use`
block:

```rust
use crate::server::BackendCapabilities;

impl BackendCapabilities {
    /// Snapshot every backend-derived fact `ServerState` needs, at
    /// startup, in one place.
    ///
    /// Adding a capability here is the whole point of the type: the
    /// struct literal below fails to compile until the new field is
    /// filled, which is where that mistake should surface.
    #[must_use]
    pub fn from_backend(backend: &dyn Backend) -> Self {
        Self {
            dpms_capable: backend.dpms_capable(),
            glx_tfp_supported: backend.supports_dmabuf_export(),
        }
    }
}
```

- [ ] **Step 5: Change both constructors**

In `crates/yserver-core/src/server.rs`, replace `with_randr_outputs`
(`server.rs:1355-1358`) and the signature + head of
`with_randr_outputs_and_modes` (`server.rs:1363-1370`):

```rust
    pub fn with_randr_outputs(
        width: u16,
        height: u16,
        outputs: Vec<RandrOutput>,
        capabilities: BackendCapabilities,
    ) -> Self {
        let mode_table = RandrState::from_outputs(0, outputs.clone()).mode_table;
        Self::with_randr_outputs_and_modes(width, height, outputs, mode_table, capabilities)
    }

    /// Build a `ServerState` seeded with a caller-supplied set of
    /// RANDR outputs and explicit mode table.
    #[must_use]
    pub fn with_randr_outputs_and_modes(
        width: u16,
        height: u16,
        outputs: Vec<RandrOutput>,
        mode_table: Vec<crate::randr::RandrMode>,
        capabilities: BackendCapabilities,
    ) -> Self {
        let mut s = Self::with_geometry(width, height);
        s.randr = RandrState::from_outputs_with_modes(0, outputs, mode_table);
        s.dpms = DpmsState::new(capabilities.dpms_capable);
        s.glx_tfp_supported = capabilities.glx_tfp_supported;
```

Leave the rest of the body (the root/overlay extent fixups,
`server.rs:1371-1382`) untouched. It reads neither `dpms` nor
`glx_tfp_supported`, verified 2026-08-06, so moving the assignments
earlier is semantically inert.

**Do not touch `ServerState::new`.** Its 617 call sites keep today's
defaults.

- [ ] **Step 6: Update the KMS entry point**

In `crates/yserver/src/lib.rs`, replace lines 323-337. `backend` is
already built at `lib.rs:318`, so it is available before the
constructor:

```rust
    let (randr_outputs, randr_mode_table) = backend.randr_outputs_and_modes();
    let capabilities = yserver_core::server::BackendCapabilities::from_backend(&backend);
    let mut state = ServerState::with_randr_outputs_and_modes(
        fb_w,
        fb_h,
        randr_outputs,
        randr_mode_table,
        capabilities,
    );
```

Keep the long `crate::clock::init` comment block and the
`crate::clock::init(state.start_instant);` call exactly where they are.
**Delete** the two lines `state.dpms = …` and
`state.glx_tfp_supported = …` — the constructor performs them now.
Leave `install_backend_root_bindings(&mut state, &backend);` in place.

- [ ] **Step 7: Update the ynest entry point**

In `crates/yserver-core/src/nested.rs`, replace line 409 and delete
lines 416-417:

```rust
    let capabilities = crate::backend::BackendCapabilities::from_backend(&backend);
    let mut state = ServerState::with_randr_outputs(width, height, vec![synthetic], capabilities);
```

The comment at `nested.rs:410-415` explaining why the DPMS snapshot is
kept for symmetry describes behaviour the constructor now owns —
rewrite it to say the capabilities snapshot is taken once, through
`from_backend`, for both entry points.

- [ ] **Step 8: Fix the rest of the workspace**

Run: `cargo build --workspace --all-targets 2>&1 | head -40`

Expected: errors only at call sites of the two changed constructors.
There should be very few — measured 2026-08-06, each has exactly one
non-test call site. Fix any test call sites by passing
`BackendCapabilities::default()`, which reproduces today's values
(`false`/`false`).

- [ ] **Step 9: Run the tests**

```bash
cargo test -p yserver-core --lib backend::tests
cargo test -p yserver-core
```

Expected: PASS.

- [ ] **Step 10: Record it in `docs/status.md`**

Add an entry describing the seam change: backend capabilities are now
snapshotted once via `BackendCapabilities::from_backend` and required by
`ServerState`'s randr constructors, replacing two hand-maintained
assignment blocks in the KMS and ynest entry points; this also closes a
pre-existing gap where a missed `glx_tfp_supported` assignment would
compile and pass tests. Follow the file's existing entry format and its
descending-date ordering.

- [ ] **Step 11: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -S -m "$(cat <<'EOF'
refactor(core): require BackendCapabilities to build a ServerState

lib.rs:336-337 and nested.rs:416-417 were duplicated backend->state
snapshot blocks inside two run() functions no test can call: yserver::run
needs a DRM device and a VT, nested::run needs a host X server. Nothing
caught an omission at either site, so a missed glx_tfp_supported
assignment compiled, passed every unit test, and silently shipped the
default.

Both randr constructors now require a BackendCapabilities, built only by
from_backend, so the omission does not compile. This is cheap because
those constructors have exactly one call site each -- the two run()
functions; the 617 call sites of ServerState::new are untouched.

The move is semantically inert: with_randr_outputs_and_modes' body reads
neither dpms nor glx_tfp_supported, so assigning them inside the
constructor rather than after it is indistinguishable.

Severable from the GLX vendor work it was written for.
EOF
)"
```

---

## Task 2: carry the vendor list across the `Backend` seam

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs:15` (import) and
  near `supports_dmabuf_export` (`trait_def.rs:999`)
- Modify: `crates/yserver-core/src/server.rs` — field near
  `glx_tfp_supported` (`server.rs:1079`), default near `server.rs:1334`,
  third field on `BackendCapabilities`
- Modify: `crates/yserver-core/src/backend/mod.rs` —
  `resolve_glx_vendor_names`, third field in `from_backend`, tests

**Interfaces:**
- Consumes: `BackendCapabilities` and `from_backend` from Task 1.
- Produces: `Backend::glx_vendor_names(&self) -> &'static str` (default
  returns `glx::VENDOR_NAMES`); `ServerState::glx_vendor_names: String`;
  `BackendCapabilities.glx_vendor_names: String`;
  `resolve_glx_vendor_names(derived: &str, raw_env: Option<&str>) -> String`
  (module-private).

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block created in Task 1
(`crates/yserver-core/src/backend/mod.rs`):

```rust
    #[test]
    fn resolve_prefers_env_over_derived() {
        assert_eq!(
            super::resolve_glx_vendor_names("nvidia mesa", Some("mesa")),
            "mesa"
        );
    }

    #[test]
    fn resolve_trims_env_value() {
        assert_eq!(
            super::resolve_glx_vendor_names("mesa", Some("  nvidia mesa  ")),
            "nvidia mesa"
        );
    }

    #[test]
    fn resolve_falls_back_when_env_absent_or_blank() {
        // A typo must never keep the display server from starting, so
        // blank input degrades to the derived value rather than erroring.
        assert_eq!(super::resolve_glx_vendor_names("nvidia mesa", None), "nvidia mesa");
        assert_eq!(super::resolve_glx_vendor_names("nvidia mesa", Some("")), "nvidia mesa");
        assert_eq!(
            super::resolve_glx_vendor_names("nvidia mesa", Some("   ")),
            "nvidia mesa"
        );
    }

    #[test]
    fn from_backend_takes_vendor_names_from_the_backend() {
        // RecordingBackend does not override glx_vendor_names, so this
        // pins the trait default reaching the struct.
        let backend = RecordingBackend::new();
        let caps = BackendCapabilities::from_backend(&backend);
        assert_eq!(caps.glx_vendor_names, "mesa");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p yserver-core --lib backend::tests`

Expected: FAIL to compile — `resolve_glx_vendor_names` and the
`glx_vendor_names` field do not exist.

- [ ] **Step 3: Add the trait method**

In `crates/yserver-core/src/backend/trait_def.rs`, add `glx` to the
import at line 15:

```rust
use yserver_protocol::x11::{AtomId, ClipRectangles, FontMetrics, glx, xfixes};
```

and add, beside `supports_dmabuf_export` (`trait_def.rs:999`):

```rust
    /// Client GLX vendor library names for `GLX_VENDOR_NAMES_EXT`, in
    /// libglvnd priority order, space-separated.
    ///
    /// libglvnd's `__glXLookupVendorByScreen` splits the reply with
    /// `strtok_r(..., " ", ...)` and tries each name in turn, moving to
    /// the next when a vendor fails to load or its `isScreenSupported`
    /// returns False, so a list is a real fallback chain rather than a
    /// wire curiosity (verified against libglvnd 1.7.0,
    /// `src/GLX/libglxmapping.c:519-600`).
    ///
    /// The default is `VENDOR_NAMES` ("mesa"), which is also what
    /// Xorg's glamor GLX provider falls back to
    /// (`glamor/glamor_glx_provider.c:425`). Only `KmsBackend`
    /// overrides it.
    fn glx_vendor_names(&self) -> &'static str {
        glx::VENDOR_NAMES
    }
```

- [ ] **Step 4: Add the `ServerState` field**

In `crates/yserver-core/src/server.rs`, beside `pub glx_tfp_supported: bool`
(`server.rs:1079`):

```rust
    /// Vendor-name list answered for `GLX_VENDOR_NAMES_EXT`, snapshotted
    /// from the backend at startup. Space-separated, libglvnd priority
    /// order.
    pub glx_vendor_names: String,
```

and beside `glx_tfp_supported: false` in the default block
(`server.rs:1334`):

```rust
            glx_vendor_names: yserver_protocol::x11::glx::VENDOR_NAMES.to_string(),
```

If `server.rs:13`'s `use yserver_protocol::x11::{…}` group already
carries other GLX items, add `glx` there and shorten the path.

- [ ] **Step 5: Add the third capability and the resolver**

In `crates/yserver-core/src/server.rs`, add to `BackendCapabilities`:

```rust
    /// From `Backend::glx_vendor_names`, after `YSERVER_GLX_VENDOR` is
    /// applied.
    pub glx_vendor_names: String,
```

`#[derive(Default)]` still holds — `String::default()` is empty, and the
only producer is `from_backend`.

In `crates/yserver-core/src/backend/mod.rs`, add the resolver and wire
the third field:

```rust
/// Resolve the vendor-name list actually sent to clients.
///
/// Precedence: `YSERVER_GLX_VENDOR` > the backend's derived value.
///
/// The env value arrives as a parameter rather than being read here so
/// the accepted spellings stay testable — mutating the process
/// environment races under a parallel test runner.
///
/// Validation is trim-and-reject-empty only. A name with no matching
/// `libGLX_<name>.so` needs no server-side check: the client fails to
/// load it and libglvnd falls through to the next entry, which is the
/// intended experimental behaviour. A typo must not keep the display
/// server from starting.
fn resolve_glx_vendor_names(derived: &str, raw_env: Option<&str>) -> String {
    match raw_env {
        Some(raw) if !raw.trim().is_empty() => {
            let chosen = raw.trim().to_string();
            log::info!("GLX vendor names overridden by YSERVER_GLX_VENDOR: {chosen}");
            chosen
        }
        Some(_) => {
            log::warn!(
                "YSERVER_GLX_VENDOR is set but empty; using derived value {derived}"
            );
            derived.to_string()
        }
        None => derived.to_string(),
    }
}
```

and in `from_backend`:

```rust
            glx_vendor_names: resolve_glx_vendor_names(
                backend.glx_vendor_names(),
                std::env::var("YSERVER_GLX_VENDOR").ok().as_deref(),
            ),
```

`from_backend` is the one place that reads `std::env`. It runs once per
server lifetime, so no `OnceLock` caching is needed — unlike
`YSERVER_SCANOUT_MODIFIER`'s reader (`kms/vk/scanout.rs:1051`), which is
consulted repeatedly.

- [ ] **Step 6: Deposit it in the constructor**

In `with_randr_outputs_and_modes`, beside the Task 1 assignments:

```rust
        s.glx_vendor_names = capabilities.glx_vendor_names;
```

Order matters only in that `capabilities` is consumed by value; take the
`String` last or destructure the struct.

- [ ] **Step 7: Run the tests**

```bash
cargo test -p yserver-core --lib backend::tests
cargo test -p yserver-core
```

Expected: PASS.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -S -m "$(cat <<'EOF'
feat(glx): carry the vendor-name list across the Backend seam

Adds Backend::glx_vendor_names (default "mesa", the same value Xorg's
glamor provider falls back to), the ServerState field it feeds, and
resolve_glx_vendor_names for the YSERVER_GLX_VENDOR override.

ash is a yserver-crate dependency only, so the value crosses the seam as
a &'static str the way glx_tfp_supported crosses as a bool.

The override takes the env value as a parameter rather than reading it
inside the resolver: env mutation races under a parallel test runner.
from_backend is the single place that reads std::env, and it runs once
per server lifetime, so no OnceLock caching is warranted.

Blank or whitespace-only input degrades to the derived value with a
warning -- a typo must not keep the display server from starting. An
unloadable name is deliberately not validated server-side: libglvnd
falls through to the next entry in the list.

No behaviour change yet; the reply arm still returns the constant.
EOF
)"
```

---

## Task 3: derive the list from `vk.driver_id`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs` — new function near
  `probe_dmabuf_export_support` (`backend.rs:698`); getter override near
  `supports_dmabuf_export` (`backend.rs:12670`); tests in the existing
  `mod tests` (`backend.rs:18938`)

**Interfaces:**
- Consumes: `Backend::glx_vendor_names` from Task 2.
- Produces: `glx_vendor_names_for_driver(ash::vk::DriverId) -> &'static str`
  (module-private) and the `KmsBackend` override.

- [ ] **Step 1: Write the failing test**

In `crates/yserver/src/kms/render/backend.rs`'s `mod tests`
(`backend.rs:18938`), add `glx_vendor_names_for_driver` to the
`use super::{…}` list and add:

```rust
    #[test]
    fn glx_vendor_names_are_nvidia_first_then_mesa_on_nvidia() {
        use ash::vk::DriverId;

        // libglvnd tries the names left to right and falls through when
        // one will not load, so "mesa" is insurance for the routine
        // package split where the NVIDIA Vulkan ICD is installed but
        // libGLX_nvidia.so is not. Without it libglvnd resolves no
        // vendor and lands on FALLBACK_VENDOR_NAME "indirect", which is
        // worse than today's llvmpipe.
        assert_eq!(
            glx_vendor_names_for_driver(DriverId::NVIDIA_PROPRIETARY),
            "nvidia mesa"
        );
    }

    #[test]
    fn glx_vendor_names_stay_mesa_on_every_other_driver() {
        use ash::vk::DriverId;
        use yserver_protocol::x11::glx as x11glx;

        // The mapping is deliberately binary. Entries for other drivers
        // are omitted because nobody working on this repo can measure
        // them, and an unmeasured mapping that redirects a working
        // configuration onto a nonexistent libGLX_*.so is worse than
        // the status quo.
        for driver in [
            DriverId::MESA_LLVMPIPE,
            DriverId::INTEL_OPEN_SOURCE_MESA,
            DriverId::MESA_RADV,
            DriverId::AMD_PROPRIETARY,
        ] {
            assert_eq!(glx_vendor_names_for_driver(driver), x11glx::VENDOR_NAMES);
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p yserver --lib glx_vendor_names`

Expected: FAIL to compile — the function does not exist.

- [ ] **Step 3: Add the function**

In `crates/yserver/src/kms/render/backend.rs`, beside
`probe_dmabuf_export_support` (`backend.rs:698`):

```rust
/// Client GLX vendor library names to advertise for a given render
/// driver, in libglvnd priority order.
///
/// Same shape as the other per-driver policies in this tree —
/// `scanout_prefers_linear` (`kms/vk/scanout.rs:922`) and
/// `VkContext::supports_dri3_syncobj` (`kms/vk/device.rs:88`).
///
/// Deliberately binary. Every non-NVIDIA driver keeps "mesa": an
/// unmeasured mapping that redirects a configuration working today onto
/// a nonexistent `libGLX_*.so` is worse than the status quo, and nobody
/// working on this repo can measure AMD proprietary, Imagination, or
/// the rest.
///
/// The second entry matters on NVIDIA. If the NVIDIA Vulkan ICD is
/// installed but `libGLX_nvidia.so` is not — a routine package split —
/// a bare "nvidia" leaves libglvnd with no vendor and it lands on
/// `FALLBACK_VENDOR_NAME` "indirect", worse than today's llvmpipe.
fn glx_vendor_names_for_driver(driver_id: ash::vk::DriverId) -> &'static str {
    if matches!(driver_id, ash::vk::DriverId::NVIDIA_PROPRIETARY) {
        "nvidia mesa"
    } else {
        yserver_protocol::x11::glx::VENDOR_NAMES
    }
}
```

- [ ] **Step 4: Add the getter override**

In `impl Backend for KmsBackend`, beside `supports_dmabuf_export`
(`backend.rs:12670`):

```rust
    fn glx_vendor_names(&self) -> &'static str {
        self.platform
            .vk
            .as_ref()
            .map_or(yserver_protocol::x11::glx::VENDOR_NAMES, |vk| {
                glx_vendor_names_for_driver(vk.driver_id)
            })
    }
```

**No cached field, and no change to `KmsBackend::for_tests_seed`
(`backend.rs:2049`).** Unlike `dmabuf_export_supported`
(`backend.rs:602`), which caches real Vulkan probing work, this is a
`matches!` on an enum and the getter is called exactly once per server
lifetime, from `BackendCapabilities::from_backend`.

`platform.vk` is `Option<Arc<VkContext>>` (`kms/render/platform.rs:567`,
no `cfg` gate) — a `KmsBackend` with no Vulkan context is
representable, hence the `None` arm. The access shape mirrors
`backend.rs:1164-1167`.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p yserver --lib glx_vendor_names
cargo test -p yserver
```

Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -S -m "$(cat <<'EOF'
feat(glx): derive the vendor-name list from vk.driver_id

KmsBackend now answers "nvidia mesa" on NVIDIA_PROPRIETARY and keeps
"mesa" everywhere else. Same shape as the tree's other per-driver
policies, scanout_prefers_linear and VkContext::supports_dri3_syncobj.

The mapping is deliberately binary: an unmeasured mapping that redirects
a configuration working today onto a nonexistent libGLX_*.so is worse
than the status quo.

The second entry is insurance for the package split where the NVIDIA
Vulkan ICD is present but libGLX_nvidia.so is not -- a bare "nvidia"
would leave libglvnd on FALLBACK_VENDOR_NAME "indirect", worse than
today's llvmpipe. libglvnd 1.7.0 honours a list
(src/GLX/libglxmapping.c:519-600).

No cached field: the getter runs once per server lifetime and the
computation is a matches! on an enum, so for_tests_seed is untouched.
EOF
)"
```

---

## Task 4: answer from state, and make the query observable

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs:11658-11665`
  (comment + arm), plus a `debug!` on the `VENDOR_NAMES_EXT` path
- Modify: `crates/yserver-protocol/src/x11/glx.rs:942` (test comment only)
- Modify: `docs/status.md`
- Test: `crates/yserver-core/src/core_loop/process_request.rs` `mod tests`

**Interfaces:**
- Consumes: `ServerState::glx_vendor_names` from Task 2.

- [ ] **Step 1: Write the failing test**

In `process_request.rs`'s test module, modelled on
`glx_get_drawable_attributes_naked_window_wire_reply`
(`process_request.rs:52495`):

```rust
    #[test]
    fn glx_vendor_names_query_answers_from_server_state() {
        use yserver_protocol::x11::glx as g;

        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 1);
        let mut backend = RecordingBackend::new();
        let client_id = ClientId(1);

        // The value the backend derived at startup, not the constant.
        state.glx_vendor_names = "nvidia mesa".to_string();

        // QueryServerString body: screen (u32), name (u32).
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&g::VENDOR_NAMES_EXT.to_le_bytes());
        let length_units = u32::try_from(1 + body.len().div_ceil(4)).expect("fits");

        process_request(
            &mut state,
            &mut backend,
            client_id,
            SequenceNumber(9),
            RequestHeader {
                opcode: 148,
                data: g::QUERY_SERVER_STRING,
                length_units,
            },
            &body,
            None,
        )
        .expect("process_request QUERY_SERVER_STRING");

        peer.set_nonblocking(true).unwrap();
        let mut header = [0u8; 32];
        peer.read_exact(&mut header).expect("reply header delivered");
        assert_eq!(header[0], 1, "byte 0 must be 1 (Reply)");
        assert_eq!(u16::from_le_bytes([header[2], header[3]]), 9, "sequence");

        // Reply layout, read off encode_string_reply (glx.rs:267-291):
        //   0      1 (Reply)      1      0 (pad)
        //   2..4   sequence       4..8   length_units = padded / 4
        //   8..12  pad1           12..16 n (string length INCLUDING NUL)
        //   16..32 pad3..pad6     then bytes + NUL + zero padding
        // Note n sits at 12, not 8 -- offset 8 is pad1. The
        // GetDrawableAttributes reply carries numAttribs at 8, which is a
        // different reply shape; do not copy that offset here.
        let n = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        assert_eq!(n as usize, "nvidia mesa".len() + 1, "n counts the NUL");

        let padded = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize * 4;
        assert_eq!(padded, 12, "11 bytes + NUL, already 4-aligned");
        let mut tail = vec![0u8; padded];
        peer.read_exact(&mut tail).expect("reply tail delivered");
        let s = String::from_utf8_lossy(&tail);
        assert!(
            s.starts_with("nvidia mesa"),
            "arm must read state, not the VENDOR_NAMES constant; got {s:?}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p yserver-core --lib glx_vendor_names_query_answers_from_server_state`

Expected: **FAIL with `"mesa"`, not a compile error.** This is the test
the spec designates as verified-failing-first: today the arm returns
`x11glx::VENDOR_NAMES` regardless of state. **Record the actual failure
output in the commit message.** If it fails to compile instead, the test
harness call is wrong — fix the test, not the assertion.

- [ ] **Step 3: Change the arm and rewrite its comment**

In `crates/yserver-core/src/core_loop/process_request.rs`, replace the
comment at 11658-11664 and the arm at 11665:

```rust
                // libglvnd vendor-neutral dispatch: tells the client which
                // libGLX_<vendor>.so drives this screen. Resolved once at
                // startup from the render driver
                // (`BackendCapabilities::from_backend`); every non-NVIDIA
                // driver keeps `VENDOR_NAMES` ("mesa"), which is what stops
                // libglvnd from falling back to a vendor that resolves to
                // nothing on Asahi → NULL glXQueryExtensionsString → cogl
                // SIGSEGV. Only queried because we advertise
                // GLX_EXT_libglvnd.
                x11glx::VENDOR_NAMES_EXT => &state.glx_vendor_names,
```

The borrow is fine: `&state.glx_vendor_names` and the later
`state.clients.get_mut(…)` (`process_request.rs:11669`) touch disjoint
fields of `*state`. **Do not replace either access with an accessor
method** — that would borrow all of `*state` and stop compiling.

- [ ] **Step 4: Add the debug log**

Still inside the `QUERY_SERVER_STRING` arm, after `s` is bound, matching
the convention of `QUERY_VERSION` (`process_request.rs:11636`):

```rust
            debug!(
                "client {} #{} GLX::QueryServerString name={name:#x} -> {s:?}",
                client_id.0, sequence.0
            );
```

This is not polish. Without it the hardware run in Task 6 cannot observe
the query at all, and cannot distinguish "libglvnd honoured our list"
from "libglvnd guessed nvidia for unrelated reasons".

- [ ] **Step 5: Annotate the protocol test**

`vendor_names_query_reports_mesa` (`glx.rs:942`) keeps asserting
`VENDOR_NAMES == "mesa"` — still the default. Add to its comment that
the constant is now the **default only**, and no longer pins what goes
on the wire; `ServerState::glx_vendor_names` does. Change no assertions.

No `encode_string_reply` list test is added: `glx.rs:272-275` computes
`n`/`padded`/`length_units` from `bytes.len()` with no space-aware
branch, so a two-name list cannot regress independently of a one-name
one.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p yserver-core --lib glx_vendor_names_query_answers_from_server_state
cargo test -p yserver-core
cargo test -p yserver-protocol
```

Expected: PASS.

- [ ] **Step 7: Document `YSERVER_GLX_VENDOR` in `docs/status.md`**

Follow the `YSERVER_SCANOUT_MODIFIER` entry at `docs/status.md:5306`.
Must state: accepted value is the vendor string verbatim (`nvidia`,
`mesa`, `nvidia mesa`); precedence is env > driver derivation > `mesa`;
and **the value is read by the server, so changing it requires
restarting the display server** — unlike `__GLX_VENDOR_LIBRARY_NAME`,
which takes effect on the next client launch.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -S -m "$(cat <<'EOF'
feat(glx): answer GLX_VENDOR_NAMES_EXT from server state

The arm returned the hardcoded VENDOR_NAMES ("mesa") for every screen,
so libglvnd loaded libGLX_mesa.so on NVIDIA hardware, Mesa found no DRI
driver for the PCI id, and GL fell back to llvmpipe on a discrete GPU.

It now reads ServerState::glx_vendor_names, derived from vk.driver_id at
startup. The regression test was verified failing against the previous
commit first.

Also adds a debug! on the QueryServerString path. That path had no
logging at all, which made the server log structurally incapable of
showing the query this whole change rests on -- without it a hardware
run cannot tell "libglvnd honoured our list" from "libglvnd guessed
nvidia for unrelated reasons".

The comment above the arm was rewritten rather than dropped: the
Asahi/cogl SIGSEGV rationale is still live, now carried by the
non-NVIDIA arm of the driver mapping instead of by a constant.
EOF
)"
```

---

## Task 5: workspace green

**Files:** none — this task only verifies.

- [ ] **Step 1: Full test run**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: all three crates pass. For reference, the last recorded
workspace figure was 2209 tests; this plan adds roughly nine.

- [ ] **Step 2: CI-exact clippy**

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: no output.

- [ ] **Step 3: CI-exact format check**

```bash
cargo +nightly fmt --check
```

Expected: no output. `rustup` with a nightly toolchain is installed on
this box, so this is verifiable locally.

- [ ] **Step 4: Confirm every commit is signed**

```bash
git log --format='%h %G? %s' bd168cf..HEAD
```

Expected: every row's second column is `G`.

- [ ] **Step 5: Fix anything that failed, then re-run from Step 1**

Do not proceed to Task 6 with a red workspace.

---

## Task 6: hardware verification — the merge precondition

**Not automatable. These are the only checks that close defect A, and
Check 3 is a gate on merging.**

**Trap that cost a cycle on 2026-08-05:**
`~/yserver-glx-probes/run-probe-session-0805.sh` runs `cargo build`
against **whatever branch is checked out**. It was once run against the
wrong branch and produced four output files byte-identical to the
baseline — a re-measurement of the bug, mistaken for a measurement of
the fix. Run `git branch --show-current` and confirm
`glx-vendor-names-from-driver` before anything else.

- [ ] **Step 1: Confirm the branch, then run the probe from tty2**

```bash
git branch --show-current   # must print glx-vendor-names-from-driver
~/yserver-glx-probes/run-probe-session-0805.sh
```

- [ ] **Step 2: Check the probe output**

`vendor-default.txt` must report
`GL_RENDERER: NVIDIA GeForce RTX 5060 Ti` with **no**
`__GLX_VENDOR_LIBRARY_NAME` set, and `raw-wire.txt` must show
`VENDOR_NAMES_EXT -> "nvidia mesa"`.

Both files were byte-identical to the 2026-08-03 baseline before this
change; if they still are, the build did not pick up the fix — go back
to Step 1.

- [ ] **Step 3: Check the server log shows the query**

The `GLX::QueryServerString` line from Task 4 Step 4 must appear with
`name=0x20f6`. Without it the run cannot attribute the outcome to our
reply.

- [ ] **Step 4: Run a Plasma session — THE MERGE GATE**

Pass/fail: does `kwin_x11` survive under the NVIDIA vendor with
`GLX_EXT_texture_from_pixmap` unadvertised?

- **Survives** → land, and defect B is downgraded for compositors.
- **Crashes** → the change is still correct and independently measured,
  but it must land together with a defect-B decision, because "Plasma
  crashes" then becomes the shipped state on the **default** path rather
  than an opt-in one. Stop and escalate to the user; do not merge.

Expect the crash to be reachable now in a way it was not before. KWin
previously never got as far as `glXBindTexImageEXT` under the NVIDIA
vendor because it could not create a context at all; defect C's fix
removed that blocker. Note that the status quo is **not** "Plasma runs
on llvmpipe" — Plasma already crashes there today, so this change moves
an existing crash rather than creating one.

- [ ] **Step 5: Re-measure Steam**

The 2026-08-01/02 session with `__GLX_VENDOR_LIBRARY_NAME=nvidia`
exported globally recorded Steam and games crashing on launch, with
defect C's exact signature (`BadAlloc` / major opcode 148 / minor 5 /
serial 0, `steam-juegos-nvidia/steam-console-linux.txt:63423-63426`).
Defect C is fixed, so that regression is **expected to be gone** — but
it was never re-run against the fixed branch and Steam is a 32-bit
client. Measure, do not assume.

- [ ] **Step 6: Preserve the logs**

Every yserver start truncates the logs at the repo root. Copy the run
aside immediately, following the convention of
`~/yserver-glx-logs/2026-08-03-investigation/`, and write a `README.md`
next to it recording the branch, commit, and what each file shows.

- [ ] **Step 7: Report results and stop**

Report all five outcomes to the user. **Do not squash-merge** —
AGENTS.md:18 requires asking for confirmation, and Step 4 may have
turned up a blocker.

---

## Notes for the reviewer

Things that look wrong and are not:

- **`with_randr_outputs_and_modes(w, h, outputs, modes, caps)` seems to
  break the `with_*` naming convention.** It does not:
  `width`/`height` are already absent from two of the three constructor
  names. The convention names the differentiator from the base
  constructor, and `capabilities`, like geometry, is common to both
  randr constructors rather than distinguishing them.
- **Task 1 does not add `glx_vendor_names`.** That is the severability
  boundary, and it deliberately exercises the mechanism: Task 2's new
  field fails to compile at `from_backend`'s struct literal until it is
  handled.
- **`from_backend` lives in `backend/mod.rs`, not `trait_def.rs`.**
  `trait_def.rs` defines the contract and reads no environment; startup
  policy does not belong there. Not `params.rs` either — that module is
  explicitly core → backend, per request (`params.rs:1-3`).
- **`GLX_EXT_libglvnd` stays advertised unconditionally**
  (`glx.rs:201-204`). Xorg gates the advertisement on having resolved a
  vendor (`glxscreens.c:425`) and returns `BadValue` without one
  (`glxcmds.c:2433`); yserver's floor is `"mesa"`, so it can never enter
  the vendorless state those paths exist to handle. The regression
  assertion at `glx.rs:938` stays as-is — it guards the Cinnamon/cogl
  SIGSEGV on Asahi.

Deliberately out of scope, recorded so it is not rediscovered: ynest
against an NVIDIA host still reports `"mesa"`; `vk.driver_id` is the
render device's driver, which on a PRIME/Optimus box need not match the
KMS node's vendor; fbconfig synthesis is untouched (2 FBConfigs against
XWayland's 168); `QUERY_CONTEXT` still answers 0 attributes.
