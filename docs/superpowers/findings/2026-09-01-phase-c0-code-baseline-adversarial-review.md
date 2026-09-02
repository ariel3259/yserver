# Phase C.0 merged-code-baseline adversarial review

**Date:** 2026-09-01  
**Reviewer:** Opus  
**Reviewed spec:** the coordinate-concurrency-revised Draft of
`2026-08-26-phase-c0-atomic-kms-migration-design.md`  
**Code basis:** Rust sources byte-identical to merged Phase A+B plus
`c09358a1`  
**Disposition:** Incorporated after maintainer/user approval

## Verification basis

The working branch HEAD was not itself `c09358a1`, but its Rust tree was
verified byte-identical to that baseline. The review's code observations
therefore apply. All named symbols were found at the described call sites, and
`crates/yserver/src/kms/render/backend.rs` contained 39,204 lines at review
time.

The local `drm 0.15.0` source confirmed that `Device::receive_events()` reads
the shared event byte stream and its `DRM_EVENT_FLIP_COMPLETE` parser constructs
`PageFlipEvent.crtc` from `crtc_id` when non-zero or from `user_data` otherwise.
It does not expose the two fields independently. The baseline yserver drain
uses that `Event::PageFlip` representation and manually parses only
`DRM_EVENT_CRTC_SEQUENCE` delivered as `Event::Unknown`.

## Accepted corrections

1. **Raw DRM event ownership.** C.0 replaces the complete compatibility event
   drain with one buffered raw parser for vblank, flip-complete, and CRTC-
   sequence events. It preserves `user_data`, `crtc_id`, `sequence`, `tv_sec`,
   and `tv_usec` independently, validates every declared length before
   advancing, and is the sole reader of the shared stream. This work belongs in
   PR 1 with the event identities, not as an incidental extension of the submit
   wrapper.
2. **Portable ioctl boundary.** The existing `IoctlReq` alias already uses
   `libc::Ioctl` on Linux and `libc::c_ulong` elsewhere. The spec now explicitly
   requires replacing or normalizing that pre-existing boundary rather than
   merely forbidding a new alias. One reviewed wrapper may retain correct
   target-specific types internally, with merge-blocking glibc, musl, and
   FreeBSD compile evidence.
3. **Clock-source granularity.** The process-lifetime
   `HashSet<DrmDeviceKey>` used for queue-sequence unsupported state and every
   constructor/read/write site are removed. The decision lives in the owner's
   `(device incarnation, hardware CRTC, clock epoch)` record and starts
   unresolved after reopen or any new epoch.
4. **DPMS projection.** X11's protocol contract remains one global power
   level; C.0 does not invent a per-output client API. Internally the global
   request is projected in one epoch to desired targets keyed by stable output
   identity. New outputs inherit the current level, removed outputs invalidate
   their projection, and one device applies compatible targets as an ordered
   atomic transition. The baseline all-output best-effort loop and global
   `kms_outputs_active` boolean are explicit conversion sites.
5. **Atomic gamma discovery.** The baseline legacy
   `get_crtc().gamma_length()` query, `u16::MAX` clamps, cache fallbacks, and
   resume/reapply consumers are inventoried. Atomic `GAMMA_LUT_SIZE` becomes
   the sole size source; an absent, zero, excessive, or unrepresentable value
   exposes gamma unavailable without clamping.

## Review-stack correction

The former PR 2 combined the owner, primary/successor conversion, modeset,
unflip, DPMS, VT, hotplug, topology, and recovery inside the repository's
largest Rust file. The approved stack has four independently bisectable PRs:

1. selected host-call path plus raw event/ABI/identity/clock substrate;
2. device owner plus primary/direct/successor integration;
3. modeset, unflip, DPMS, VT, hotplug, topology, and lifecycle integration;
4. atomic cursor/gamma conversion and final capability/hardware gates.

The first three keep C.0 capability false. This is an integration split, not a
runtime rollout or configuration mechanism. Each family has its own scoped
tests and physical gate, while only the final PR can advertise C.0 and must pass
the complete eight-hour soak.
