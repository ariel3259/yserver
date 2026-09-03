# Phase C.0 Stage 1 — Executor host-call, raw-event, identity and evidence substrate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the process-isolated `KmsIoExecutor`, the single raw DRM event-stream parser, the commit/event/sequence identities and the evidence primitives that every later C.0 stage depends on.

**Architecture:** Every KMS host call moves out of the X11 core into a per-device-incarnation helper process reached over a framed Unix socket with fd passing, modelled closely on the existing `internal_probe` PRIME helper. The compatibility `drm` crate event drain is replaced by one buffered raw parser that validates every header before advancing. Commit, event, sequence-arm and clock-epoch identities become typed and incarnation-monotonic so later stages can prove provenance instead of inferring it.

**Tech Stack:** Rust (stable toolchain), `drm` 0.15 / `drm-ffi` 0.9, `libc`, `std::os::unix` sockets and `pre_exec`. No serialization crate: framing is hand-rolled fixed-length frames, matching `internal_probe.rs`.

**Spec:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` (Approved 2026-09-02). This plan implements section 18 stage 1 only. Stages 2 to 4 are planned separately once this stage settles the interfaces they consume.

**Review:** `docs/superpowers/findings/2026-09-02-phase-c0-stage-1-plan-adversarial-review.md` — two blocking, three major and two minor findings, all applied before execution.

## Global Constraints

Copied verbatim from the spec. Every task's requirements implicitly include this section.

- The X11 core never executes or waits synchronously for a potentially blocking KMS ioctl (`COMMIT-5`).
- The owner installs `Submitting` or `CoordinateSubmitting` and the applicable fd lease **before** IPC dispatch (`COMMIT-6`).
- After dispatch, explicit rejection, success and acceptance-unknown remain distinct. IPC loss, helper exit or watchdog expiry can never be rewritten as rejection (`COMMIT-6`).
- Host-call watchdog: 2 seconds for seat-active `NONBLOCK` work, 30 seconds for a permitted cold-start/final-offline blocking ioctl.
- Sending a termination signal, closing the IPC channel, `PR_SET_PDEATHSIG`, or a watchdog expiry is a request, **not** reap proof (`COMMIT-7`).
- Executors are created by reexecuting the yserver helper mode through `posix_spawn` or fork-immediately-followed-by-exec using only async-signal-safe child setup. They never run the Rust allocator, locks, Vulkan, GBM or backend code in a forked multithreaded child before exec.
- Message transport is message-boundary-preserving: `SOCK_SEQPACKET` or an equivalent framed transport. Atomic success returns every out-fence through fd passing.
- The single raw event parser validates every header length before advancing. Zero, undersized, over-buffer, truncated or overflowing lengths are malformed input, never an excuse for an out-of-bounds or non-progressing parse.
- The latency/concurrency recorder uses a checked fixed-size, single-writer preallocated buffer. It performs no filesystem write, allocation, buffer flush or additional supervisor IPC on the measured path, never wraps, and exports only after the arm ends. Exhaustion makes the affected evidence row `EvidenceInsufficient`.
- Transport criteria for later evidence: `ExecutorTransportP99Max = 50 us`, `ExecutorTransportP999Max = 100 us`, `ExecutorTransportExcursionCeiling = 500 us`, all measured with the class's own helper ioctl duration excluded.
- Portable builds must compile on glibc, musl and FreeBSD. These gates plus the synthetic malformed/concatenated raw-event tests are required before this stage is considered reviewable.
- Format check is `cargo +nightly fmt --check`. Tests are `cargo test -p yserver`.

---

## File Structure

**New:**
- `crates/yserver/src/platform/ioctl.rs` — the portable raw-ioctl ABI boundary: the `IoctlReq` type and request-code construction, in one place.
- `crates/yserver/src/drm/event_stream.rs` — the single buffered raw DRM event parser and its typed records.
- `crates/yserver/src/kms/owner/mod.rs` — owner-side module root for stage 1 types.
- `crates/yserver/src/kms/owner/identity.rs` — `CommitId`, `EventToken`, `SequenceArmToken`, `ClockEpochId`.
- `crates/yserver/src/kms/executor/mod.rs` — `KmsIoExecutor`, supervisor, lease registry, reap.
- `crates/yserver/src/kms/executor/protocol.rs` — framed request/response encoding.
- `crates/yserver/src/kms/executor/transport.rs` — send/receive with `SCM_RIGHTS` fd passing.
- `crates/yserver/src/kms/executor/helper.rs` — the reexec helper entry point and host-call loop.
- `crates/yserver/src/kms/executor/device_lock.rs` — the `COMMIT-7` device-scoped lock and its start-time check.
- `crates/yserver/src/kms/evidence.rs` — the fixed-size single-writer latency/concurrency recorder.
- `crates/yserver/tests/executor_substrate.rs` — integration tests that spawn real helper processes.

**Modified:**
- `crates/yserver/src/drm/page_flip.rs` — remove the local `IoctlReq` alias, the compatibility `Event::PageFlip` drain and the manual `Event::Unknown` sequence parser.
- `crates/yserver/src/present/event_loop.rs:99,247` and `crates/yserver/src/kms/render/platform.rs:4160,4190` — the four `drain_events` call sites move to the new parser.
- `crates/yserver/src/kms/render/backend.rs:1039,3697,4640,9159,9211,9252-9279` — remove `crtc_queue_sequence_unsupported_devices`; sequence classification becomes epoch-local.
- `crates/yserver/src/lib.rs` and `crates/yserver/src/platform/mod.rs` — module declarations.
- `crates/yserver/src/bin/yserver.rs:6` — dispatch the new helper reexec mode alongside the probe helper.

`crates/yserver/src/kms/console.rs` also declares an `IoctlReq` alias. It is VT plumbing, not KMS, and is deliberately **out of scope**: do not migrate it in this stage.

---

### Task 1: Portable raw-ioctl ABI boundary

**Files:**
- Create: `crates/yserver/src/platform/ioctl.rs`
- Modify: `crates/yserver/src/platform/mod.rs`
- Modify: `crates/yserver/src/drm/page_flip.rs:25-28` (remove the local alias)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) type IoctlReq`, `pub(crate) const fn iowr(kind: u8, nr: u8, size: usize) -> IoctlReq`, `pub(crate) const DRM_IOCTL_BASE: u8 = b'd'`.

- [x] **Step 1: Write the failing test**

Create `crates/yserver/src/platform/ioctl.rs` containing only the test module:

```rust
//! Portable raw-ioctl ABI boundary.

#[cfg(test)]
mod tests {
    use super::{DRM_IOCTL_BASE, IoctlReq, iowr};

    #[test]
    fn iowr_reproduces_the_queue_sequence_request_code() {
        // _IOWR('d' /*0x64*/, 0x3C, drm_crtc_queue_sequence /*24 bytes*/):
        //   (3 << 30) | (24 << 16) | (0x64 << 8) | 0x3C = 0xC018643C
        assert_eq!(iowr(DRM_IOCTL_BASE, 0x3C, 24), 0xC018_643C_u32 as IoctlReq);
    }

    #[test]
    fn iowr_reproduces_the_atomic_request_code() {
        // _IOWR('d', 0xBC, drm_mode_atomic /*40 bytes*/):
        //   (3 << 30) | (40 << 16) | (0x64 << 8) | 0xBC = 0xC02864BC
        assert_eq!(iowr(DRM_IOCTL_BASE, 0xBC, 40), 0xC028_64BC_u32 as IoctlReq);
    }
}
```

Add `pub(crate) mod ioctl;` to `crates/yserver/src/platform/mod.rs`.

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --lib platform::ioctl`
Expected: FAIL to compile — `cannot find function iowr in this scope`.

- [x] **Step 3: Write minimal implementation**

Above the test module in `crates/yserver/src/platform/ioctl.rs`:

```rust
#[cfg(target_os = "linux")]
pub(crate) type IoctlReq = libc::Ioctl;
#[cfg(not(target_os = "linux"))]
pub(crate) type IoctlReq = libc::c_ulong;

/// DRM ioctl type letter, `<drm/drm.h>` `DRM_IOCTL_BASE`.
pub(crate) const DRM_IOCTL_BASE: u8 = b'd';

const DIRECTION_READ_WRITE: u32 = 3;
const SIZE_SHIFT: u32 = 16;
const TYPE_SHIFT: u32 = 8;
const DIRECTION_SHIFT: u32 = 30;
const SIZE_MASK: u32 = 0x3FFF;

/// Build an `_IOWR` request code.
///
/// `size` is the payload struct size in bytes and must fit the 14-bit
/// size field; a larger struct is a programming error, not a runtime
/// condition, so this panics in a const context at build time.
pub(crate) const fn iowr(kind: u8, nr: u8, size: usize) -> IoctlReq {
    assert!(size <= SIZE_MASK as usize, "ioctl payload exceeds the 14-bit size field");
    let code = (DIRECTION_READ_WRITE << DIRECTION_SHIFT)
        | ((size as u32) << SIZE_SHIFT)
        | ((kind as u32) << TYPE_SHIFT)
        | (nr as u32);
    code as IoctlReq
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test -p yserver --lib platform::ioctl`
Expected: PASS, 2 tests.

- [x] **Step 5: Migrate the page_flip alias**

In `crates/yserver/src/drm/page_flip.rs`, delete lines 25-28 (the `#[cfg(target_os = "linux")] type IoctlReq = libc::Ioctl;` block and its `not(linux)` twin) and import the shared one instead:

```rust
use crate::platform::ioctl::IoctlReq;
```

Leave `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` where it is for now; task 2 moves the raw event structs.

- [x] **Step 6: Run the existing page_flip tests to prove no behaviour changed**

Run: `cargo test -p yserver --lib drm::page_flip`
Expected: PASS, including `drm_crtc_queue_sequence_ioctl_request_code`.

- [x] **Step 7: Commit**

```bash
git add crates/yserver/src/platform/ioctl.rs crates/yserver/src/platform/mod.rs crates/yserver/src/drm/page_flip.rs
git commit -m "refactor(kms): add the portable raw-ioctl ABI boundary"
```

---

### Task 2: The single raw DRM event-stream parser

**Files:**
- Create: `crates/yserver/src/drm/event_stream.rs`
- Modify: `crates/yserver/src/drm/mod.rs`

**Interfaces:**
- Consumes: `crate::platform::ioctl::IoctlReq` (task 1).
- Produces:
  - `pub(crate) enum DrmEventRecord { PageFlip { crtc_id: u32, sequence: u32, tv_sec: u32, tv_usec: u32, user_data: u64 }, Vblank { .. same fields .. }, CrtcSequence { user_data: u64, time_ns: i64, sequence: u64 } }`
  - `pub(crate) enum EventParseError { Malformed(&'static str) }`
  - `pub(crate) fn parse_event_buffer(bytes: &[u8]) -> Result<Vec<DrmEventRecord>, EventParseError>`
  - `pub(crate) const DRM_EVENT_VBLANK: u32 = 0x01;`
  - `pub(crate) const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;`
  - `pub(crate) const DRM_EVENT_CRTC_SEQUENCE: u32 = 0x03;`

The parser takes a byte buffer rather than a device so it is unit-testable without a DRM fd, exactly as `dispatch_event` was. Task 3 supplies the buffer from a real read.

- [x] **Step 1: Write the failing tests**

Create `crates/yserver/src/drm/event_stream.rs` with the test module. These are the synthetic malformed and concatenated cases the spec names as a stage gate:

```rust
#[cfg(test)]
mod tests {
    use super::{
        DRM_EVENT_CRTC_SEQUENCE, DRM_EVENT_FLIP_COMPLETE, DrmEventRecord, parse_event_buffer,
    };

    fn vblank_bytes(kind: u32, crtc_id: u32, sequence: u32, user_data: u64) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&kind.to_ne_bytes());
        b[4..8].copy_from_slice(&32u32.to_ne_bytes());
        b[8..16].copy_from_slice(&user_data.to_ne_bytes());
        b[16..20].copy_from_slice(&0u32.to_ne_bytes()); // tv_sec
        b[20..24].copy_from_slice(&0u32.to_ne_bytes()); // tv_usec
        b[24..28].copy_from_slice(&sequence.to_ne_bytes());
        b[28..32].copy_from_slice(&crtc_id.to_ne_bytes());
        b
    }

    #[test]
    fn decodes_a_flip_complete_with_its_crtc_id_preserved() {
        let bytes = vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 0x42, 7, 0xDEAD_BEEF);
        let records = parse_event_buffer(&bytes).expect("well-formed buffer");
        assert_eq!(
            records,
            vec![DrmEventRecord::PageFlip {
                crtc_id: 0x42,
                sequence: 7,
                tv_sec: 0,
                tv_usec: 0,
                user_data: 0xDEAD_BEEF,
            }]
        );
    }

    #[test]
    fn decodes_two_concatenated_events_in_order() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 10, 100));
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 2, 20, 200));
        let records = parse_event_buffer(&buf).expect("well-formed buffer");
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0], DrmEventRecord::PageFlip { crtc_id: 1, .. }));
        assert!(matches!(records[1], DrmEventRecord::PageFlip { crtc_id: 2, .. }));
    }

    #[test]
    fn skips_an_unknown_but_well_formed_event_by_its_declared_length() {
        let mut buf = Vec::new();
        let mut unknown = [0u8; 16];
        unknown[0..4].copy_from_slice(&99u32.to_ne_bytes());
        unknown[4..8].copy_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(&unknown);
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 5, 1, 0));
        let records = parse_event_buffer(&buf).expect("unknown well-formed event is skippable");
        assert_eq!(records.len(), 1, "only the known event decodes");
        assert!(matches!(records[0], DrmEventRecord::PageFlip { crtc_id: 5, .. }));
    }

    #[test]
    fn rejects_a_zero_length_event_instead_of_looping() {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        buf[4..8].copy_from_slice(&0u32.to_ne_bytes());
        assert!(parse_event_buffer(&buf).is_err(), "zero length must not advance by zero");
    }

    #[test]
    fn rejects_a_length_shorter_than_the_header() {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        buf[4..8].copy_from_slice(&4u32.to_ne_bytes());
        assert!(parse_event_buffer(&buf).is_err());
    }

    #[test]
    fn rejects_a_length_beyond_the_buffer() {
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        buf[4..8].copy_from_slice(&64u32.to_ne_bytes());
        assert!(parse_event_buffer(&buf).is_err(), "declared length overruns the buffer");
    }

    #[test]
    fn rejects_a_truncated_trailing_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 1, 0));
        buf.extend_from_slice(&[0u8; 3]);
        assert!(parse_event_buffer(&buf).is_err(), "3 trailing bytes cannot hold a header");
    }

    #[test]
    fn rejects_a_known_type_carrying_the_wrong_length() {
        let mut bytes = vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 1, 0);
        bytes[4..8].copy_from_slice(&24u32.to_ne_bytes());
        assert!(parse_event_buffer(&bytes).is_err());
    }

    #[test]
    fn decodes_a_crtc_sequence_event_with_its_raw_fields() {
        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&DRM_EVENT_CRTC_SEQUENCE.to_ne_bytes());
        b[4..8].copy_from_slice(&32u32.to_ne_bytes());
        b[8..16].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_ne_bytes());
        b[16..24].copy_from_slice(&(-5i64).to_ne_bytes());
        b[24..32].copy_from_slice(&9_999u64.to_ne_bytes());
        let records = parse_event_buffer(&b).expect("well-formed sequence event");
        assert_eq!(
            records,
            vec![DrmEventRecord::CrtcSequence {
                user_data: 0x1234_5678_9ABC_DEF0,
                time_ns: -5,
                sequence: 9_999,
            }]
        );
    }
}
```

Add `pub mod event_stream;` to `crates/yserver/src/drm/mod.rs`.

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib drm::event_stream`
Expected: FAIL to compile — `parse_event_buffer` not found.

- [x] **Step 3: Write minimal implementation**

Above the test module:

```rust
//! The single raw DRM event-stream parser.
//!
//! All DRM event types share one byte stream on the device fd, so C.0 owns
//! the whole drain rather than racing a second reader or raw-parsing only
//! selected events. Every header length is validated before the cursor
//! advances: a zero, undersized, over-buffer, truncated or overflowing length
//! is malformed input, never a reason to read out of bounds or to stop making
//! progress.

pub(crate) const DRM_EVENT_VBLANK: u32 = 0x01;
pub(crate) const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
pub(crate) const DRM_EVENT_CRTC_SEQUENCE: u32 = 0x03;

const HEADER_LEN: usize = 8;
const VBLANK_LEN: usize = 32;
const CRTC_SEQUENCE_LEN: usize = 32;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DrmEventRecord {
    PageFlip { crtc_id: u32, sequence: u32, tv_sec: u32, tv_usec: u32, user_data: u64 },
    Vblank { crtc_id: u32, sequence: u32, tv_sec: u32, tv_usec: u32, user_data: u64 },
    CrtcSequence { user_data: u64, time_ns: i64, sequence: u64 },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum EventParseError {
    Malformed(&'static str),
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(raw)
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_ne_bytes(raw)
}

pub(crate) fn parse_event_buffer(bytes: &[u8]) -> Result<Vec<DrmEventRecord>, EventParseError> {
    let mut records = Vec::new();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        if bytes.len() - cursor < HEADER_LEN {
            return Err(EventParseError::Malformed("trailing bytes cannot hold an event header"));
        }
        let kind = u32_at(bytes, cursor);
        let length = u32_at(bytes, cursor + 4) as usize;

        // The cursor must always advance by at least a header, or a hostile
        // or corrupt stream would spin here forever.
        if length < HEADER_LEN {
            return Err(EventParseError::Malformed("event length is shorter than its header"));
        }
        if length > bytes.len() - cursor {
            return Err(EventParseError::Malformed("event length overruns the buffer"));
        }
        let body = &bytes[cursor..cursor + length];

        match kind {
            DRM_EVENT_VBLANK | DRM_EVENT_FLIP_COMPLETE => {
                if length != VBLANK_LEN {
                    return Err(EventParseError::Malformed("vblank event has the wrong length"));
                }
                let user_data = u64_at(body, 8);
                let tv_sec = u32_at(body, 16);
                let tv_usec = u32_at(body, 20);
                let sequence = u32_at(body, 24);
                let crtc_id = u32_at(body, 28);
                records.push(if kind == DRM_EVENT_FLIP_COMPLETE {
                    DrmEventRecord::PageFlip { crtc_id, sequence, tv_sec, tv_usec, user_data }
                } else {
                    DrmEventRecord::Vblank { crtc_id, sequence, tv_sec, tv_usec, user_data }
                });
            }
            DRM_EVENT_CRTC_SEQUENCE => {
                if length != CRTC_SEQUENCE_LEN {
                    return Err(EventParseError::Malformed("sequence event has the wrong length"));
                }
                records.push(DrmEventRecord::CrtcSequence {
                    user_data: u64_at(body, 8),
                    time_ns: u64_at(body, 16) as i64,
                    sequence: u64_at(body, 24),
                });
            }
            // Unknown but well formed: skip exactly its declared length.
            _ => {}
        }

        cursor += length;
    }

    Ok(records)
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib drm::event_stream`
Expected: PASS, 9 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/drm/event_stream.rs crates/yserver/src/drm/mod.rs
git commit -m "feat(kms): add the single raw DRM event-stream parser"
```

---

### Task 3: Cut the four drain sites over and delete the compatibility drain

**Files:**
- Modify: `crates/yserver/src/drm/event_stream.rs` (add the reading half)
- Modify: `crates/yserver/src/drm/page_flip.rs` (delete `drain_events`, `dispatch_event`, `drm_event_header`, `drm_event_crtc_sequence` and their tests)
- Modify: `crates/yserver/src/present/event_loop.rs:99,247`
- Modify: `crates/yserver/src/kms/render/platform.rs:4160,4190`

**Interfaces:**
- Consumes: `parse_event_buffer`, `DrmEventRecord` (task 2).
- Produces: `pub(crate) fn drain_device_events<F>(device: &Device, on_record: F) -> io::Result<()> where F: FnMut(DrmEventRecord)`.

The four existing call sites pass two closures, `on_advance(crtc, msc, ust)` and `on_sequence(user_data, time_ns, sequence)`. They now receive one `DrmEventRecord` and match on it. Keeping one closure is what makes the drain single-reader by construction: no call site can subscribe to a subset.

**Termination condition: drain until `EAGAIN`, never a single read.** The two
call-site families are on different pollers and different trigger modes, and the
stricter one governs:

- `present/event_loop.rs:58` registers the DRM fd with raw epoll and
  `EpollFlags::EPOLLIN` alone, with no `EPOLLET` — **level-triggered**, so a
  residue would be re-reported on the next wait.
- The KMS backend fd is registered through the core poller at
  `crates/yserver-core/src/core_loop/run.rs:1058`, which is **mio 1.x**
  (`Cargo.toml:29`). mio registers epoll sources **edge-triggered**, so a
  residue is *not* re-reported: the remaining events sit in the queue until some
  unrelated activity wakes the fd, and that presents as a permanently missing
  completion.

Draining to `EAGAIN` is correct under both modes, so the drain does that
unconditionally rather than depending on which poller a given call site uses.
The DRM fd must therefore be non-blocking; assert that at registration rather
than assuming it.

- [x] **Step 1: Write the failing test**

Append to the test module in `crates/yserver/src/drm/event_stream.rs`:

```rust
    #[test]
    fn drain_continues_past_a_full_buffer_until_the_queue_is_empty() {
        // More events than one DRAIN_BUFFER_LEN read can return. A drain that
        // stopped at the first short read would strand the remainder, which is
        // unrecoverable on the edge-triggered path.
        let (reader, mut writer) = super::test_support::nonblocking_pipe().expect("pipe");
        let count = (super::DRAIN_BUFFER_LEN / 32) + 5;
        let mut buf = Vec::new();
        for index in 0..count {
            buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, index as u32 + 1, 0, 0));
        }
        std::io::Write::write_all(&mut writer, &buf).expect("write");
        let mut seen = 0usize;
        super::drain_fd_events(&reader, |_| seen += 1).expect("drain");
        assert_eq!(seen, count, "the drain must continue until EAGAIN");
    }

    #[test]
    fn drain_reads_one_buffer_and_yields_every_record_in_order() {
        // A pipe stands in for the DRM fd: the drain is a read plus a parse,
        // and the parse is already covered above.
        use std::io::Write;
        let (reader, mut writer) = std::io::pipe().expect("pipe");
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 10, 0));
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 2, 20, 0));
        writer.write_all(&buf).expect("write");
        drop(writer);

        let mut seen = Vec::new();
        super::drain_fd_events(&reader, |record| seen.push(record)).expect("drain");
        assert_eq!(seen.len(), 2);
        assert!(matches!(seen[0], DrmEventRecord::PageFlip { crtc_id: 1, .. }));
        assert!(matches!(seen[1], DrmEventRecord::PageFlip { crtc_id: 2, .. }));
    }
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --lib drm::event_stream::tests::drain_reads_one_buffer`
Expected: FAIL to compile — `drain_fd_events` not found.

- [x] **Step 3: Write minimal implementation**

First, stop one malformed event from destroying the whole batch. The task 2
review established that the mandated `Result<Vec<_>, _>` shape discards every
good record already decoded from the same read, that the bytes are gone once
`read(2)` has consumed them, and that all four call sites `?`-propagate — so a
single bad event loses completions the baseline `dispatch_event` would have kept,
because it dropped one event and continued. Add the partial form and keep the
existing one as a wrapper, so task 2's signature and its tests stand unchanged:

```rust
/// Parse as far as the buffer allows, returning what decoded plus the failure
/// that stopped it. Callers dispatch the good records first: the bytes are
/// already consumed and cannot be read again, so discarding them would lose
/// real completions. The error still reaches the caller, which routes it to the
/// poison boundary rather than swallowing it.
pub(crate) fn parse_event_buffer_partial(
    bytes: &[u8],
) -> (Vec<DrmEventRecord>, Option<EventParseError>) {
    // ... same loop as parse_event_buffer, but each malformed branch breaks
    // with the error instead of returning and dropping `records` ...
}

pub(crate) fn parse_event_buffer(bytes: &[u8]) -> Result<Vec<DrmEventRecord>, EventParseError> {
    match parse_event_buffer_partial(bytes) {
        (records, None) => Ok(records),
        (_, Some(error)) => Err(error),
    }
}
```

Add one test proving the difference, since it is the whole point of the change:

```rust
    #[test]
    fn a_malformed_tail_does_not_discard_the_records_before_it() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 7, 1, 0, 0, 0));
        buf.extend_from_slice(&[0u8; 3]); // cannot hold a header
        let (records, error) = super::parse_event_buffer_partial(&buf);
        assert_eq!(records.len(), 1, "the good record must survive the bad tail");
        assert!(error.is_some(), "and the failure must still be reported");
    }
```

Adjust that test's `vblank_bytes` call to whatever signature task 2's fix round
settled on.

Then add the drain itself:

```rust
use std::{io, os::fd::AsFd};

/// One read is one drain: the kernel returns whole events, and the parser
/// rejects any buffer that does not decompose into whole events.
const DRAIN_BUFFER_LEN: usize = 1024;

pub(crate) fn drain_fd_events<F>(fd: &impl AsFd, mut on_record: F) -> io::Result<()>
where
    F: FnMut(DrmEventRecord),
{
    let mut buffer = [0u8; DRAIN_BUFFER_LEN];
    loop {
        let read = match rustix_read(fd, &mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let records = parse_event_buffer(&buffer[..read]).map_err(|EventParseError::Malformed(why)| {
            io::Error::new(io::ErrorKind::InvalidData, format!("malformed DRM event stream: {why}"))
        })?;
        for record in records {
            on_record(record);
        }
        // Deliberately no `read < DRAIN_BUFFER_LEN` early return: under mio's
        // edge-triggered registration a residue is never re-reported, so the
        // loop continues until the kernel says the queue is empty.
    }
}

fn rustix_read(fd: &impl AsFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `read` writes at most `buffer.len()` bytes into a valid
    // mutable slice and reports the count.
    let read = unsafe {
        libc::read(
            std::os::fd::AsRawFd::as_raw_fd(&fd.as_fd()),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(read as usize)
}

pub(crate) fn drain_device_events<F>(device: &crate::drm::Device, on_record: F) -> io::Result<()>
where
    F: FnMut(DrmEventRecord),
{
    drain_fd_events(device, on_record)
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test -p yserver --lib drm::event_stream`
Expected: PASS, 10 tests.

- [x] **Step 5: Move the four call sites**

At each of `present/event_loop.rs:99`, `present/event_loop.rs:247`, `kms/render/platform.rs:4160` and `kms/render/platform.rs:4190`, replace the `drm::page_flip::drain_events(device, on_advance, on_sequence)` call with the single-closure form. The shape at every site becomes:

```rust
crate::drm::event_stream::drain_device_events(device, |record| match record {
    DrmEventRecord::PageFlip { crtc_id, sequence, tv_sec, tv_usec, .. } => {
        // Previously the `on_advance` closure body. The drm crate handed us a
        // `crtc::Handle`; construct it from the raw id the kernel actually sent.
        let Some(handle) = drm::control::from_u32::<crtc::Handle>(crtc_id) else {
            return;
        };
        let ust = std::time::Duration::new(u64::from(tv_sec), tv_usec * 1_000);
        // ... existing on_advance body, with `sequence` in place of `frame` ...
    }
    DrmEventRecord::CrtcSequence { user_data, time_ns, sequence } => {
        // ... existing on_sequence body, unchanged ...
    }
    DrmEventRecord::Vblank { .. } => {}
})?;
```

- [x] **Step 6: Give the task 1 boundary its first caller and retire the duplicate**

The task 1 review found that without this step the stage ends with three
independent copies of the same `_IOWR` bit math — `platform/ioctl.rs`,
`page_flip.rs:85-88` and `imported_syncobj.rs:54-57` — and the new boundary
retires none of them, leaving its blanket `#![allow(dead_code)]` permanent. In
`crates/yserver/src/drm/page_flip.rs`, replace the hand-expanded constant with
the boundary:

```rust
pub(crate) const DRM_IOCTL_CRTC_QUEUE_SEQUENCE: IoctlReq = iowr(
    DRM_IOCTL_BASE,
    0x3C,
    std::mem::size_of::<drm_crtc_queue_sequence>(),
);
```

Keep the existing `drm_crtc_queue_sequence_ioctl_request_code` test unchanged:
it now proves the boundary reproduces the value the hand-expansion produced.

Then in `crates/yserver/src/platform/ioctl.rs`:

- Narrow `#![allow(dead_code)]` to the items that still lack a caller, and name
  the task that retires each. `iowr` and `DRM_IOCTL_BASE` now have one.
- Move the cfg-split rationale here from `page_flip.rs:78-84`, since this module
  is now the one place a reader goes to understand the boundary. State it
  accurately: `libc::Ioctl` is `c_ulong` on glibc and `c_int` on musl, FreeBSD
  does not export the alias at all, and the code is built in `u32` so the
  read-write direction bits survive. A reader who "simplifies" this to a single
  `libc::Ioctl` breaks FreeBSD; one who picks a single `c_ulong` breaks musl by
  mismatching `libc::ioctl`'s own signature.
- Give `SIZE_MASK` a cfg: Linux's `_IOC_SIZEBITS` is 14 (`0x3FFF`), but
  FreeBSD's `IOCPARM_MASK` is 13 (`0x1FFF`). The guard's doc comment claims to
  catch a programming error, and on one of the three named targets it currently
  does not.

Finally delete the comment above `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` that still says
FreeBSD's libc "does not export that alias … so use a tiny local alias": the
local alias was removed in task 1 and the sentence is now false.

- [x] **Step 7: Delete the compatibility drain**

From `crates/yserver/src/drm/page_flip.rs` delete `drain_events`, `dispatch_event`, `drm_event_header`, `drm_event_crtc_sequence`, `DRM_EVENT_CRTC_SEQUENCE` and the five tests that cover them (`dispatch_event_passes_crtc_handle_for_page_flip`, `dispatch_event_ignores_unknown`, `dispatch_event_decodes_crtc_sequence`, `dispatch_event_ignores_wrong_length_sequence_event`, `dispatch_event_ignores_unknown_event_type`). Keep `submit_flip`, `drm_crtc_queue_sequence`, the two struct-layout tests and the ioctl request-code test. Remove the now-unused `Event` import.

- [x] **Step 8: Retire the task 2 dead-code allowances**

Task 2 added eleven narrowly scoped `#[allow(dead_code)]` attributes in
`event_stream.rs`, each commented as retiring in this task. Every item now has a
real caller through the drain and the migrated call sites, so delete all eleven
and let `cargo clippy -p yserver --all-targets -- -D warnings` prove it. An
allowance that outlives its reason becomes a permanent mask over genuinely dead
code added later.

- [x] **Step 9: Prove no second reader survives**

Run: `grep -rn 'receive_events' crates/yserver/src/`
Expected: no matches outside doc comments. If any remain, they are a second reader on the same stream and must move to `drain_device_events`.

- [x] **Step 10: Run the full suite**

Run: `cargo test -p yserver`
Expected: PASS.

- [x] **Step 11: Commit**

```bash
git add crates/yserver/src/drm/ crates/yserver/src/platform/ioctl.rs crates/yserver/src/present/event_loop.rs crates/yserver/src/kms/render/platform.rs
git commit -m "refactor(kms): drain DRM events through the single raw parser"
```

---

### Task 4: Commit, event, sequence-arm and clock-epoch identities

**Files:**
- Create: `crates/yserver/src/kms/owner/mod.rs`
- Create: `crates/yserver/src/kms/owner/identity.rs`
- Modify: `crates/yserver/src/kms/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub(crate) struct IncarnationId(u64)`, `pub(crate) struct CommitId(u64)`, `pub(crate) struct EventToken(u64)`, `pub(crate) struct SequenceArmToken(u64)`, `pub(crate) struct ClockEpochId(u64)`
  - `pub(crate) struct IdentityAllocator { .. }` with `fn new(incarnation: IncarnationId) -> Self`, `fn next_commit(&mut self) -> CommitId`, `fn next_event_token(&mut self) -> EventToken`, `fn next_sequence_arm(&mut self) -> SequenceArmToken`, `fn incarnation(&self) -> IncarnationId`
  - `impl EventToken { pub(crate) fn as_user_data(self) -> u64; pub(crate) fn from_user_data(raw: u64) -> Option<Self>; }`

Zero is never a valid token. The spec requires an injected zero or unknown `EventToken` on a current tagged event to poison rather than to be silently accepted, so the type must be able to reject it at the boundary.

- [x] **Step 1: Write the failing tests**

Create `crates/yserver/src/kms/owner/identity.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::{EventToken, IdentityAllocator, IncarnationId};

    #[test]
    fn commit_ids_are_monotonic_within_an_incarnation() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let a = ids.next_commit();
        let b = ids.next_commit();
        assert!(b > a, "commit ids must increase");
    }

    #[test]
    fn event_tokens_never_repeat_within_an_incarnation() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(ids.next_event_token()), "event token reused");
        }
    }

    #[test]
    fn a_zero_user_data_is_not_a_valid_event_token() {
        assert!(EventToken::from_user_data(0).is_none());
    }

    #[test]
    fn an_event_token_round_trips_through_user_data() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let token = ids.next_event_token();
        assert_eq!(EventToken::from_user_data(token.as_user_data()), Some(token));
    }

    #[test]
    fn a_new_incarnation_does_not_reissue_the_previous_incarnations_tokens() {
        let mut first = IdentityAllocator::new(IncarnationId::first());
        let stale = first.next_event_token();
        let mut second = IdentityAllocator::new(IncarnationId::first().next());
        let fresh = second.next_event_token();
        assert_ne!(stale, fresh, "a fresh incarnation must not collide with the old one");
    }

    #[test]
    fn sequence_arm_tokens_are_distinct_from_event_tokens() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let event = ids.next_event_token();
        let arm = ids.next_sequence_arm();
        assert_ne!(event.as_user_data(), arm.as_user_data());
    }
}
```

Create `crates/yserver/src/kms/owner/mod.rs` with `pub(crate) mod identity;` and add `pub(crate) mod owner;` to `crates/yserver/src/kms/mod.rs`.

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::owner::identity`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

```rust
//! Typed C.0 identities.
//!
//! Every identity is incarnation-scoped and monotonic. The kernel echoes
//! `user_data` verbatim, so the owner must be able to tell its own live token
//! from a stale one, from another purpose's token, and from zero — the spec
//! requires a zero or unknown token on a current tagged event to poison the
//! incarnation rather than to be accepted.

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct IncarnationId(u64);

impl IncarnationId {
    pub(crate) const fn first() -> Self {
        Self(1)
    }
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct CommitId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct EventToken(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SequenceArmToken(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ClockEpochId(u64);

impl EventToken {
    pub(crate) const fn as_user_data(self) -> u64 {
        self.0
    }
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }
}

impl SequenceArmToken {
    pub(crate) const fn as_user_data(self) -> u64 {
        self.0
    }
    pub(crate) const fn from_user_data(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }
}

/// Purpose tags occupy the top bits so an event token and a sequence-arm
/// token can never be mistaken for each other in an echoed `user_data`.
///
/// Both purposes draw from ONE counter, not one per purpose. Section 6.1
/// allocates from a single monotonic namespace across the complete device
/// incarnation; two independent counters would still pass a per-type
/// uniqueness test while quietly breaking that property, and nothing
/// downstream would notice until two tokens of different purposes shared a
/// counter value in a log.
const PURPOSE_SHIFT: u32 = 62;
const PURPOSE_EVENT: u64 = 1;
const PURPOSE_SEQUENCE_ARM: u64 = 2;
const COUNTER_MASK: u64 = (1 << PURPOSE_SHIFT) - 1;

pub(crate) struct IdentityAllocator {
    incarnation: IncarnationId,
    next_commit: u64,
    next_counter: u64,
}

impl IdentityAllocator {
    pub(crate) fn new(incarnation: IncarnationId) -> Self {
        // Seeding the counter from the incarnation keeps a fresh incarnation
        // from reissuing a token the previous one may still see echoed.
        Self { incarnation, next_commit: 1, next_counter: incarnation.get() << 32 | 1 }
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn next_commit(&mut self) -> CommitId {
        let id = CommitId(self.next_commit);
        self.next_commit += 1;
        id
    }

    fn next_tagged(&mut self, purpose: u64) -> u64 {
        let counter = self.next_counter & COUNTER_MASK;
        self.next_counter += 1;
        (purpose << PURPOSE_SHIFT) | counter
    }

    pub(crate) fn next_event_token(&mut self) -> EventToken {
        EventToken(self.next_tagged(PURPOSE_EVENT))
    }

    pub(crate) fn next_sequence_arm(&mut self) -> SequenceArmToken {
        SequenceArmToken(self.next_tagged(PURPOSE_SEQUENCE_ARM))
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::owner::identity`
Expected: PASS, 6 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/owner/ crates/yserver/src/kms/mod.rs
git commit -m "feat(kms): add typed incarnation-scoped C.0 identities"
```

---

### Task 5: Replace the raw-CRTC / high-bit sequence encoding with the typed arm token

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:1508-1521` (delete `ABSOLUTE_SEQ_TAG` and `absolute_seq_user_data`)
- Modify: `crates/yserver/src/kms/render/backend.rs:9062-9075` (`on_crtc_sequence_event` decode)
- Modify: `crates/yserver/src/kms/render/backend.rs:9260` (the relative arm's `user_data`)

**Interfaces:**
- Consumes: `SequenceArmToken`, `IdentityAllocator` (task 4); `DrmEventRecord::CrtcSequence` (task 2).
- Produces: `SequenceArm { token: SequenceArmToken, crtc_id: u32, epoch: ClockEpochId, purpose: SequenceArmPurpose, target: u64 }` and `pub(crate) enum SequenceArmPurpose { Relative, AbsolutePerTarget }`, plus `fn take_matching_arm(&mut self, token: SequenceArmToken) -> Option<SequenceArm>`.

Today the arm kind is a high bit and the CRTC is the low 32 bits of `user_data`, so any stale or foreign echo that happens to carry a live CRTC id decodes as a valid arm. The spec requires that only a fresh arm token matching CRTC, epoch, purpose, target and event type can advance the reference. The token becomes the whole `user_data`, and the arm record holds the rest.

- [x] **Step 1: Write the failing tests**

Add to the test module in `crates/yserver/src/kms/render/backend.rs`:

```rust
    #[test]
    fn a_stale_arm_token_does_not_advance_the_reference() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let mut arms = SequenceArmTable::default();
        let live = ids.next_sequence_arm();
        arms.insert(SequenceArm {
            token: live,
            crtc_id: 0x42,
            epoch: ClockEpochId::first(),
            purpose: SequenceArmPurpose::AbsolutePerTarget,
            target: 1_000,
        });
        let stale = ids.next_sequence_arm(); // never armed
        assert!(arms.take_matching_arm(stale).is_none());
        assert!(arms.take_matching_arm(live).is_some());
    }

    #[test]
    fn an_arm_token_is_consumed_exactly_once() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let mut arms = SequenceArmTable::default();
        let token = ids.next_sequence_arm();
        arms.insert(SequenceArm {
            token,
            crtc_id: 1,
            epoch: ClockEpochId::first(),
            purpose: SequenceArmPurpose::Relative,
            target: 0,
        });
        assert!(arms.take_matching_arm(token).is_some());
        assert!(arms.take_matching_arm(token).is_none(), "a consumed arm must not fire twice");
    }

    #[test]
    fn an_event_token_echoed_into_the_sequence_path_is_rejected() {
        let mut ids = IdentityAllocator::new(IncarnationId::first());
        let mut arms = SequenceArmTable::default();
        let event = ids.next_event_token();
        // A page-flip token must never decode as a sequence arm, even if the
        // counter happens to match: the purpose tag differs.
        assert!(SequenceArmToken::from_user_data(event.as_user_data())
            .is_none_or(|token| arms.take_matching_arm(token).is_none()));
    }

    #[test]
    fn a_zero_user_data_never_decodes_as_an_arm() {
        assert!(SequenceArmToken::from_user_data(0).is_none());
    }
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::render::backend::tests::a_stale_arm_token`
Expected: FAIL to compile — `SequenceArmTable` not found.

- [x] **Step 3: Write minimal implementation**

Add near the other backend types:

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SequenceArmPurpose {
    Relative,
    AbsolutePerTarget,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct SequenceArm {
    pub(crate) token: SequenceArmToken,
    pub(crate) crtc_id: u32,
    pub(crate) epoch: ClockEpochId,
    pub(crate) purpose: SequenceArmPurpose,
    pub(crate) target: u64,
}

#[derive(Debug, Default)]
pub(crate) struct SequenceArmTable {
    live: HashMap<SequenceArmToken, SequenceArm>,
}

impl SequenceArmTable {
    pub(crate) fn insert(&mut self, arm: SequenceArm) {
        self.live.insert(arm.token, arm);
    }

    /// Consume the arm this token names, if it is still live. An unknown,
    /// already-consumed, or foreign-purpose token yields `None`; the caller
    /// treats that as telemetry, never as a reference advance.
    pub(crate) fn take_matching_arm(&mut self, token: SequenceArmToken) -> Option<SequenceArm> {
        self.live.remove(&token)
    }
}
```

Delete `ABSOLUTE_SEQ_TAG` (`backend.rs:1514`) and `absolute_seq_user_data` (`backend.rs:1521`). At the arm site (`backend.rs:9260`) pass `token.as_user_data()` instead of `u64::from(crtc_id)`. In `on_crtc_sequence_event` (`backend.rs:9062`) replace the `tagged`/`crtc_id_raw` decode with:

```rust
let Some(token) = SequenceArmToken::from_user_data(user_data) else {
    self.record_unknown_sequence_echo(device_key, user_data);
    return;
};
let Some(arm) = self.sequence_arms.take_matching_arm(token) else {
    self.record_unknown_sequence_echo(device_key, user_data);
    return;
};
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: PASS.

- [x] **Step 5: Prove the old encoding is gone**

Run: `grep -rn 'ABSOLUTE_SEQ_TAG\|absolute_seq_user_data' crates/yserver/src/`
Expected: no matches.

- [x] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "refactor(kms): key sequence arms by typed token instead of a high bit"
```

---

### Task 6: Epoch-local sequence classification, without the process-lifetime cache

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:1039` (field), `:3697`, `:4640` (constructors), `:9159`, `:9211`, `:9252-9279` (readers and writer)

**Interfaces:**
- Consumes: `ClockEpochId` (task 4).
- Produces: `fn sequence_support(&self, device: DrmDeviceKey, epoch: ClockEpochId) -> SequenceSupport` and `pub(crate) enum SequenceSupport { Unknown, Supported, UnsupportedForEpoch }`.

`crtc_queue_sequence_unsupported_devices` is a `HashSet<DrmDeviceKey>` that lives for the whole process. A device that returns `EOPNOTSUPP` under one topology epoch stays classified unsupported across VT switches, hotplug and fd reopen, which the spec forbids: structural capability is a property of the incarnation, and only a new qualified incarnation reopens it.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn an_unsupported_result_does_not_survive_a_new_epoch() {
        let mut backend = KmsBackend::for_tests();
        let device = DrmDeviceKey::for_tests(0);
        backend.record_sequence_unsupported(device, ClockEpochId::first());
        assert_eq!(
            backend.sequence_support(device, ClockEpochId::first()),
            SequenceSupport::UnsupportedForEpoch
        );
        assert_eq!(
            backend.sequence_support(device, ClockEpochId::first().next()),
            SequenceSupport::Unknown,
            "a fresh epoch must requalify rather than inherit"
        );
    }

    #[test]
    fn one_devices_unsupported_result_does_not_classify_another() {
        let mut backend = KmsBackend::for_tests();
        let epoch = ClockEpochId::first();
        backend.record_sequence_unsupported(DrmDeviceKey::for_tests(0), epoch);
        assert_eq!(
            backend.sequence_support(DrmDeviceKey::for_tests(1), epoch),
            SequenceSupport::Unknown
        );
    }
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::render::backend::tests::an_unsupported_result`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Replace the field at `backend.rs:1039`:

```rust
    /// Sequence-capability classification, keyed by device *and* epoch.
    /// A topology epoch change discards the classification: capability is a
    /// property of the incarnation, and only a fresh qualified incarnation
    /// reopens it.
    pub(crate) sequence_support: HashMap<(crate::platform::drm::DrmDeviceKey, ClockEpochId), SequenceSupport>,
```

Replace both constructor initialisers (`:3697`, `:4640`) with `sequence_support: HashMap::new(),`. Replace the two readers (`:9159`, `:9211`) with `self.sequence_support(device_key, epoch)` and the writer (`:9252-9279`) with:

```rust
    pub(crate) fn record_sequence_unsupported(&mut self, device: DrmDeviceKey, epoch: ClockEpochId) {
        let newly = self
            .sequence_support
            .insert((device, epoch), SequenceSupport::UnsupportedForEpoch)
            .is_none();
        if newly {
            log::info!("crtc queue-sequence unsupported on {device:?} for epoch {epoch:?}; flip-driven MSC only");
        }
    }

    pub(crate) fn sequence_support(&self, device: DrmDeviceKey, epoch: ClockEpochId) -> SequenceSupport {
        self.sequence_support
            .get(&(device, epoch))
            .copied()
            .unwrap_or(SequenceSupport::Unknown)
    }

    /// Drop every classification for an epoch that has ended.
    pub(crate) fn forget_sequence_support_for_epoch(&mut self, epoch: ClockEpochId) {
        self.sequence_support.retain(|(_, recorded), _| *recorded != epoch);
    }
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::render::backend`
Expected: PASS.

- [x] **Step 5: Prove the process-lifetime cache is gone**

Run: `grep -rn 'crtc_queue_sequence_unsupported_devices' crates/yserver/src/`
Expected: no matches.

- [x] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "fix(kms): make sequence capability epoch-local instead of process-lifetime"
```

---

### Task 7: Executor wire protocol

**Files:**
- Create: `crates/yserver/src/kms/executor/mod.rs` (module root only in this task)
- Create: `crates/yserver/src/kms/executor/protocol.rs`
- Modify: `crates/yserver/src/kms/mod.rs`

**Interfaces:**
- Consumes: `IncarnationId`, `CommitId`, `EventToken`, `ClockEpochId` (task 4).
- Produces:
  - `pub(crate) enum HostCallRequest { Atomic(AtomicRequest), ClockProbe(ClockProbeRequest) }`
  - `pub(crate) struct AtomicRequest { incarnation: IncarnationId, epoch: ClockEpochId, transition: Option<u64>, commit: CommitId, event_token: EventToken, flags: u32, payload_len: u32 }`
  - `pub(crate) struct ClockProbeRequest { incarnation: IncarnationId, epoch: ClockEpochId, topology_generation: u64, crtc_id: u32, clock_epoch: ClockEpochId, probe: u64 }`
  - `pub(crate) struct RequestSeq(u64)` — a monotonic per-executor request number carried by every request **and echoed in every reply**, covering the clock-probe class as well as the atomic one
  - `pub(crate) enum HostCallReply { Accepted { seq: RequestSeq, helper_duration_ns: u64, out_fence_count: u8 }, Rejected { seq: RequestSeq, errno: i32, helper_duration_ns: u64 }, ClockProbe { seq: RequestSeq, .. } }`

A reply whose `seq` does not match the outstanding request is
`UnknownReason::MalformedReply`, never a result. Without the echo the parent can
only assume a reply answers whatever is outstanding, and section 6.3's rules for
recognising a stale result presuppose that it can actually tell. The one-in-flight
discipline hides this today; stage 2's recovery paths respawn helpers and reuse
sockets, so it is far cheaper to close in the frame layout than after.
  - `fn encode_request(&HostCallRequest) -> [u8; REQUEST_FRAME_LEN]`, `fn decode_request(&[u8]) -> Result<HostCallRequest, ProtocolError>`, and the reply pair.

Framing follows `internal_probe.rs` exactly: a 12-byte header of magic, version, kind and payload length, then a fixed-length payload. Do not introduce serde or bincode; this codebase hand-rolls its frames so parent and helper cannot silently cross protocol versions.

- [x] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_atomic() -> HostCallRequest {
        HostCallRequest::Atomic(AtomicRequest {
            incarnation: IncarnationId::first(),
            epoch: ClockEpochId::first(),
            transition: Some(7),
            commit: CommitId::for_tests(3),
            event_token: EventToken::from_user_data(0x4000_0000_0000_0001).expect("valid"),
            flags: 0x0000_0201,
            payload_len: 512,
        })
    }

    #[test]
    fn an_atomic_request_round_trips() {
        let request = sample_atomic();
        let frame = encode_request(&request);
        assert_eq!(decode_request(&frame).expect("decodes"), request);
    }

    #[test]
    fn a_frame_with_a_foreign_magic_is_rejected() {
        let mut frame = encode_request(&sample_atomic());
        frame[0] = b'X';
        assert!(matches!(decode_request(&frame), Err(ProtocolError::Magic)));
    }

    #[test]
    fn a_frame_with_a_future_version_is_rejected_rather_than_guessed() {
        let mut frame = encode_request(&sample_atomic());
        frame[4..6].copy_from_slice(&(PROTOCOL_VERSION + 1).to_le_bytes());
        assert!(matches!(decode_request(&frame), Err(ProtocolError::Version(_))));
    }

    #[test]
    fn a_short_frame_is_rejected() {
        let frame = encode_request(&sample_atomic());
        assert!(matches!(decode_request(&frame[..REQUEST_FRAME_LEN - 1]), Err(ProtocolError::Length)));
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        let mut frame = encode_request(&sample_atomic());
        frame[6..8].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(decode_request(&frame), Err(ProtocolError::Kind(99))));
    }

    #[test]
    fn a_rejected_reply_keeps_its_errno_and_helper_duration() {
        let reply = HostCallReply::Rejected { errno: libc::EBUSY, helper_duration_ns: 12_345 };
        let frame = encode_reply(&reply);
        assert_eq!(decode_reply(&frame).expect("decodes"), reply);
    }

    #[test]
    fn a_reply_echoes_the_request_sequence_it_answers() {
        let reply = HostCallReply::Accepted {
            seq: RequestSeq::for_tests(41),
            helper_duration_ns: 10,
            out_fence_count: 0,
        };
        let frame = encode_reply(&reply);
        match decode_reply(&frame).expect("decodes") {
            HostCallReply::Accepted { seq, .. } => assert_eq!(seq, RequestSeq::for_tests(41)),
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn an_accepted_reply_declares_its_out_fence_count() {
        let reply = HostCallReply::Accepted { helper_duration_ns: 900, out_fence_count: 2 };
        let frame = encode_reply(&reply);
        match decode_reply(&frame).expect("decodes") {
            HostCallReply::Accepted { out_fence_count, .. } => assert_eq!(out_fence_count, 2),
            other => panic!("expected Accepted, got {other:?}"),
        }
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::executor::protocol`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Mirror `internal_probe.rs:44-58`:

```rust
pub(crate) const PROTOCOL_MAGIC: [u8; 4] = *b"YSKX";
pub(crate) const PROTOCOL_VERSION: u16 = 1;
const KIND_ATOMIC_REQUEST: u16 = 1;
const KIND_CLOCK_PROBE_REQUEST: u16 = 2;
const KIND_REPLY: u16 = 3;
const HEADER_LEN: usize = 12;
const REQUEST_PAYLOAD_LEN: usize = 64;
const REPLY_PAYLOAD_LEN: usize = 32;
pub(crate) const REQUEST_FRAME_LEN: usize = HEADER_LEN + REQUEST_PAYLOAD_LEN;
pub(crate) const REPLY_FRAME_LEN: usize = HEADER_LEN + REPLY_PAYLOAD_LEN;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProtocolError {
    Magic,
    Version(u16),
    Kind(u16),
    Length,
    Field(&'static str),
}
```

Write `encode_request` / `decode_request` / `encode_reply` / `decode_reply` with the same `put_u16`/`put_u32`/`put_u64`/`take_*` helper style as `internal_probe.rs:137-190`. Every multi-byte field is little-endian. The decoder validates magic, then version, then kind, then payload length, and only then reads fields; a zero `event_token` is `ProtocolError::Field("event_token")`.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::executor::protocol`
Expected: PASS, 7 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/ crates/yserver/src/kms/mod.rs
git commit -m "feat(kms): add the executor wire protocol"
```

---

### Task 8: Framed transport with out-fence fd passing

**Files:**
- Create: `crates/yserver/src/kms/executor/transport.rs`

**Interfaces:**
- Consumes: task 7's frames.
- Produces:
  - `pub(crate) fn send_frame(socket: &UnixStream, frame: &[u8]) -> io::Result<()>`
  - `pub(crate) fn send_reply_with_fences(socket: &UnixStream, frame: &[u8], fences: &[BorrowedFd<'_>]) -> io::Result<()>`
  - `pub(crate) fn recv_frame(socket: &UnixStream, frame: &mut [u8]) -> io::Result<ReceivedFrame>`
  - `pub(crate) struct ReceivedFrame { pub(crate) len: usize, pub(crate) fds: Vec<OwnedFd> }`

This is the only genuinely new mechanism in stage 1: `internal_probe` passes no fds, and atomic success must return every out-fence. A truncated control message must be an error, never a silently short fence list — an adopted-then-dropped fence would look like a missing completion later.

- [x] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use std::os::{fd::AsFd, unix::net::UnixStream};
    use super::{recv_frame, send_frame, send_reply_with_fences};

    #[test]
    fn a_frame_round_trips_with_no_fds() {
        let (a, b) = UnixStream::pair().expect("pair");
        send_frame(&a, &[1, 2, 3, 4]).expect("send");
        let mut buf = [0u8; 4];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(received.len, 4);
        assert_eq!(buf, [1, 2, 3, 4]);
        assert!(received.fds.is_empty());
    }

    #[test]
    fn two_out_fences_arrive_with_their_reply() {
        let (a, b) = UnixStream::pair().expect("pair");
        let first = std::fs::File::open("/dev/null").expect("open");
        let second = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &[9; 8], &[first.as_fd(), second.as_fd()]).expect("send");
        let mut buf = [0u8; 8];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(received.fds.len(), 2, "both out-fences must arrive");
    }

    #[test]
    fn a_reply_declaring_more_fences_than_it_carries_is_rejected() {
        // No MSG_CTRUNC is set in this case: the control message is intact and
        // simply carries fewer descriptors than the payload declares. Without
        // an explicit equality check this becomes a missing completion later,
        // which is the symptom hardest to trace back to its cause.
        let (a, b) = seqpacket_pair().expect("pair");
        let only = std::fs::File::open("/dev/null").expect("open");
        send_reply_with_fences(&a, &reply_frame_declaring(2), &[only.as_fd()]).expect("send");
        let mut buf = [0u8; super::REPLY_FRAME_LEN];
        let received = recv_frame(&b, &mut buf).expect("recv");
        assert!(
            super::adopt_reply(&buf, received.fds).is_err(),
            "declared out_fence_count must equal the fds received"
        );
    }

    #[test]
    fn a_message_boundary_is_preserved_between_two_sends() {
        let (a, b) = UnixStream::pair().expect("pair");
        send_frame(&a, &[1; 8]).expect("send");
        send_frame(&a, &[2; 8]).expect("send");
        let mut buf = [0u8; 8];
        recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(buf, [1; 8], "the first message must not absorb the second");
        recv_frame(&b, &mut buf).expect("recv");
        assert_eq!(buf, [2; 8]);
    }

    #[test]
    fn a_closed_peer_reports_eof_rather_than_an_empty_success() {
        let (a, b) = UnixStream::pair().expect("pair");
        drop(a);
        let mut buf = [0u8; 8];
        let result = recv_frame(&b, &mut buf);
        assert!(result.is_err() || result.expect("ok").len == 0);
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::executor::transport`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Use `sendmsg`/`recvmsg` with a `SCM_RIGHTS` control buffer sized for `MAX_OUT_FENCES` descriptors. On receive, check `MSG_CTRUNC` in `msg_flags` and return `io::ErrorKind::InvalidData` if set: a truncated control message means fences were dropped by the kernel, and treating that as a short-but-valid list would surface later as a missing completion. Adopt each received fd into an `OwnedFd` immediately so no path can leak one.

`MSG_CTRUNC` is necessary but not sufficient. A helper that miscounts, or a
reply path that builds the control message from a different slice than the
count, produces a short fence list with the control message intact and no flag
set. `adopt_reply(frame, fds)` therefore decodes the reply and requires
`fds.len() == out_fence_count` before yielding anything; a mismatch is
`UnknownReason::MalformedReply`.

Use `SOCK_SEQPACKET` for the pair rather than `UnixStream::pair()`'s default `SOCK_STREAM`, so message boundaries are a property of the socket and not of the framing discipline:

```rust
pub(crate) fn seqpacket_pair() -> io::Result<(UnixStream, UnixStream)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: socketpair writes exactly two descriptors into `fds`.
    let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors are freshly created and owned here.
    unsafe { Ok((UnixStream::from_raw_fd(fds[0]), UnixStream::from_raw_fd(fds[1]))) }
}
```

Change the tests to build their pairs with `seqpacket_pair()`.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::executor::transport`
Expected: PASS, 4 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/transport.rs
git commit -m "feat(kms): add framed executor transport with out-fence fd passing"
```

---

### Task 9: Executor supervisor, spawn and watchdog

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs`
- Create: `crates/yserver/tests/executor_substrate.rs`

**Interfaces:**
- Consumes: tasks 7 and 8.
- Produces:
  - `pub(crate) struct KmsIoExecutor { .. }` with `fn spawn(kms_fd: BorrowedFd<'_>, incarnation: IncarnationId) -> io::Result<Self>`, `fn dispatch(&mut self, request: &HostCallRequest, proof: SubmittingProof) -> HostCallOutcome`, `fn request_termination(&mut self)`, `fn try_reap(&mut self) -> ReapState`
  - `pub(crate) enum HostCallClass { SeatActiveNonblock, ColdStartOrOfflineBlocking }` with `const fn watchdog(self) -> Duration` returning 2 s and 30 s respectively
  - `pub(crate) struct SubmittingProof(())` — see M-1 below

The watchdog is **not** a `dispatch` parameter. Both values are normative and
class-derived, so the executor derives them from the request rather than letting
each call site pick: a call site that picks wrong produces a wrong bound
silently, and the failure then looks like a driver problem rather than a
plumbing one.

`SubmittingProof` is constructible only by the owner's `Submitting` /
`CoordinateSubmitting` installation, which arrives in stage 2. `COMMIT-6`'s
ordering — record and lease installed *before* IPC dispatch — otherwise lives
only in prose for the whole of stage 1 and lands in stage 2, which is exactly
where it is easiest to get wrong because that is where the call sites move.
Stage 1 defines the type with a `#[doc(hidden)] fn for_tests()` constructor;
stage 2 adds the real producer and removes nothing.
  - `pub(crate) enum HostCallOutcome { Accepted { .. }, Rejected { errno: i32, .. }, Unknown(UnknownReason) }`
  - `pub(crate) enum UnknownReason { WatchdogExpired, HelperExited, IpcFailure, MalformedReply }`
  - `pub(crate) enum ReapState { Running, Reaped(ExitStatus), Stalled }`

`HostCallOutcome` has exactly three arms on purpose. `COMMIT-6` forbids collapsing `Unknown` into `Rejected`, so no constructor, `From`, or `unwrap_or` may produce `Rejected` from a timeout, an exit, or an IPC failure.

Model the spawn on `internal_probe.rs:795-845`: `for_current_exe`, a private `REEXEC_ARG`, fixed inherited fd slots, `pre_exec` doing only `PR_SET_PDEATHSIG` plus `dup2`, and `Command::spawn`.

- [x] **Step 1: Write the failing tests**

In `crates/yserver/tests/executor_substrate.rs`:

```rust
//! Integration coverage for the C.0 executor substrate. These tests spawn
//! real helper processes; they never touch a real DRM device.

#[test]
fn a_watchdog_expiry_is_unknown_and_never_rejection() {
    let executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::NeverReply,
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(
        matches!(outcome, HostCallOutcome::Unknown(UnknownReason::WatchdogExpired)),
        "a timeout must stay Unknown: got {outcome:?}"
    );
}

#[test]
fn a_helper_that_exits_before_replying_is_unknown_and_never_rejection() {
    let executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::ExitBeforeReply,
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(matches!(outcome, HostCallOutcome::Unknown(UnknownReason::HelperExited)));
}

#[test]
fn an_explicit_errno_reply_is_rejection() {
    let executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::RejectWith(libc::EBUSY),
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert!(matches!(outcome, HostCallOutcome::Rejected { errno, .. } if errno == libc::EBUSY));
}

#[test]
fn termination_alone_is_not_reap_proof() {
    let mut executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::IgnoreTermination,
    )
    .expect("spawn");
    executor.request_termination();
    assert!(
        matches!(executor.try_reap(), ReapState::Running | ReapState::Stalled),
        "a signal is a request, not proof"
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --test executor_substrate`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Implement `KmsIoExecutor` plus a `test_support` module gated behind `#[doc(hidden)]` that spawns the helper with a stub behaviour argument, so these tests never need a DRM device. `dispatch` installs the watchdog before `send_frame`, waits for the reply with a deadline, and maps every non-reply path to `Unknown`.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --test executor_substrate`
Expected: PASS, 4 tests.

- [x] **Step 5: Prove no path converts Unknown into Rejected**

Run: `grep -rn 'Unknown' crates/yserver/src/kms/executor/mod.rs | grep -i 'reject'`
Expected: no matches.

- [x] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs crates/yserver/tests/executor_substrate.rs
git commit -m "feat(kms): add the KmsIoExecutor supervisor with a three-outcome dispatch"
```

---

### Task 10: The helper-side host call

**Files:**
- Create: `crates/yserver/src/kms/executor/helper.rs`
- Modify: `crates/yserver/src/bin/yserver.rs:6`

**Interfaces:**
- Consumes: tasks 7 and 8; `Device::from_inherited_kms_fd` (`drm/device.rs`).
- Produces: `pub fn run_reexec_executor_if_requested() -> Option<io::Result<()>>`.

The helper receives one framed request at a time, performs the raw atomic ioctl on its inherited KMS fd alias, measures that ioctl independently of IPC, and replies. It never batches, never reorders, and **never reads the DRM event fd** — drain is owner-exclusive for the incarnation even though the helper holds an alias of the same open file description.

- [x] **Step 1: Write the failing test**

Add to `crates/yserver/tests/executor_substrate.rs`:

```rust
#[test]
fn the_helper_replies_with_its_own_measured_ioctl_duration() {
    // The stub performs a deliberate 5 ms sleep in place of an ioctl, so the
    // reply's helper duration must exceed it while the transport term stays
    // far below. This is the split the transport criteria depend on.
    let executor = yserver::kms::executor::test_support::spawn_stub_helper(
        yserver::kms::executor::test_support::StubBehaviour::AcceptAfter(
            std::time::Duration::from_millis(5),
        ),
    )
    .expect("spawn");
    let outcome = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    match outcome {
        HostCallOutcome::Accepted { helper_duration_ns, round_trip_ns, .. } => {
            assert!(helper_duration_ns >= 5_000_000, "helper must report its own ioctl time");
            assert!(
                round_trip_ns >= helper_duration_ns,
                "the round trip contains the ioctl, so it cannot be shorter"
            );
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[test]
fn the_helper_never_reads_the_event_fd() {
    // The owner writes a synthetic event into the shared pipe standing in for
    // the DRM event stream; after a full request/reply exchange the owner must
    // still be able to read every byte.
    let (owner_side, helper_side) = std::io::pipe().expect("pipe");
    let executor = yserver::kms::executor::test_support::spawn_stub_helper_with_event_fd(
        yserver::kms::executor::test_support::StubBehaviour::AcceptAfter(
            std::time::Duration::ZERO,
        ),
        helper_side,
    )
    .expect("spawn");
    yserver::kms::executor::test_support::write_synthetic_event(&owner_side, 32);
    let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
    assert_eq!(
        yserver::kms::executor::test_support::readable_bytes(&owner_side),
        32,
        "the helper consumed events the owner will now never see"
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --test executor_substrate`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

`run_reexec_executor_if_requested` mirrors `internal_probe::run_reexec_helper_if_requested` (`internal_probe.rs:855-870`): match the private marker argument, reject any extra arguments, then run the loop. Wire it in `crates/yserver/src/bin/yserver.rs` beside the existing probe hook:

```rust
    if let Some(result) = yserver::kms::executor::run_reexec_executor_if_requested() {
        return result.map_err(Into::into);
    }
    if let Some(result) = yserver::internal_probe::run_reexec_helper_if_requested() {
```

The loop body is: `recv_frame` → `decode_request` → take a monotonic timestamp → issue the ioctl → take a second timestamp → `send_reply_with_fences`. The two timestamps bracket only the ioctl, which is what makes `helper_duration_ns` independent of IPC.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --test executor_substrate`
Expected: PASS, 6 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/helper.rs crates/yserver/src/bin/yserver.rs
git commit -m "feat(kms): add the executor helper host-call loop"
```

---

### Task 11: Fd leases, asynchronous reap and the stalled states

**Files:**
- Modify: `crates/yserver/src/kms/executor/mod.rs`

**Interfaces:**
- Consumes: task 9.
- Produces:
  - `pub(crate) struct IncarnationFdSet { .. }` with `fn register_alias(&mut self, fd: OwnedFd) -> LeaseId`, `fn release(&mut self, lease: LeaseId) -> Result<(), LeaseError>`, `fn outstanding(&self) -> usize`
  - `pub(crate) enum ExecutorState { Live, Stalled, ShutdownStalled, Reaped }`

A lease is released only by proven reap. `request_termination`, IPC EOF and watchdog expiry all leave the lease outstanding.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_watchdog_expiry_does_not_release_the_lease() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let mut executor = stub_executor(StubBehaviour::NeverReply, lease);
        let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
        assert_eq!(fds.outstanding(), 1, "an unresolved call keeps its lease");
        assert_eq!(executor.state(), ExecutorState::Stalled);
    }

    #[test]
    fn only_a_wait_status_releases_the_lease() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        assert!(matches!(fds.release(lease), Err(LeaseError::NotReaped)));
    }

    #[test]
    fn no_fresh_incarnation_is_created_while_a_lease_is_outstanding() {
        let mut fds = IncarnationFdSet::default();
        let _lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        assert!(fds.may_open_fresh_incarnation().is_err());
    }
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::executor`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

`release` takes proof: its argument is a `ReapProof` newtype that only `try_reap` can construct from an actual `waitpid`/`waitid` status. That makes "a signal is not reap proof" a type error rather than a review comment.

`ReapProof` must be **neither `Copy` nor `Clone`**, and `release` takes it **by
value**. The spec releases a lease exactly once; a copyable proof would let a
single wait status release several leases, which is precisely the accounting
error the type exists to prevent. Add a compile-fail test under
`crates/yserver/tests/compile_fail/` asserting that `ReapProof` cannot be
cloned, so a later `#[derive(Clone)]` cannot reopen it silently.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::executor`
Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/mod.rs
git commit -m "feat(kms): gate executor lease release behind proven reap"
```

---

### Task 12: The COMMIT-7 device lock and its start-time check

**Files:**
- Create: `crates/yserver/src/kms/executor/device_lock.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub(crate) struct DeviceLock { .. }` with `fn acquire(device: &DrmDeviceKey) -> Result<Self, LockUnavailable>`
  - `pub(crate) fn may_install_state(device: &DrmDeviceKey) -> Result<DeviceLock, LockUnavailable>` — acquires and **returns the guard**
  - `pub(crate) struct LockUnavailable { pub(crate) recorded_holder: Option<HolderRecord> }`

Two naming and shape constraints. `flock` returning `EWOULDBLOCK` proves the
lock is held; it proves nothing about who holds it, whether that holder is a
yserver helper, or whether it is wedged. The state is therefore `Held`, never
`HeldByLiveHelper` — asserting provenance a mechanism cannot establish is the
same error section 7.1 spent several rounds removing from
`AuditedCursorExpansionHazard`, which now says in its own text that it is a
source-derived prediction and never an observation. Whatever the start-time
check actually knows travels separately in `HolderRecord`, read from the lock
file's own contents, and is explicitly advisory.

Second, a separate `check_available` would be check-then-act: the answer can
change between the question and the install. `may_install_state` acquires and
holds, so the only way to learn the lock is free is to be holding it.

The lock is held by the executor for as long as it lives and released only by its death. Every server start consults it before installing any state. It survives `SIGKILL` and reparenting to `init`, which is why it — and not either exit policy — is what closes the residual window described in `COMMIT-7`.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_lock_held_by_another_process_blocks_install() {
        let device = DrmDeviceKey::for_tests(0);
        let mut child = std::process::Command::new(std::env::current_exe().expect("exe"))
            .arg("--yserver-internal-kms-lock-holder-v1")
            .spawn()
            .expect("spawn");
        wait_for_lock_held(&device);
        assert!(super::may_install_state(&device).is_err());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn the_lock_is_released_when_the_holder_dies() {
        let device = DrmDeviceKey::for_tests(1);
        {
            let _lock = DeviceLock::acquire(&device).expect("acquire");
            assert!(super::may_install_state(&device).is_err(), "held while the guard lives");
        }
        assert!(super::may_install_state(&device).is_ok(), "released when the guard drops");
    }

    #[test]
    fn a_sigkilled_holder_still_releases_the_lock() {
        // This is the property COMMIT-7 rests on: the guarantee must survive a
        // service manager killing the parent, so it cannot depend on any
        // orderly release path running.
        let device = DrmDeviceKey::for_tests(2);
        let mut child = std::process::Command::new(std::env::current_exe().expect("exe"))
            .arg("--yserver-internal-kms-lock-holder-v1")
            .spawn()
            .expect("spawn");
        wait_for_lock_held(&device);
        child.kill().expect("kill");
        child.wait().expect("wait");
        assert!(super::may_install_state(&device).is_ok());
    }
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::executor::device_lock`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Use `flock(LOCK_EX | LOCK_NB)` on a per-device file under the runtime directory. `flock` is released by the kernel when the last descriptor closes, including on `SIGKILL`, which is the property `COMMIT-7` relies on. `may_install_state` attempts the non-blocking exclusive acquire and returns the guard on success, or `LockUnavailable` on `EWOULDBLOCK` with whatever advisory `HolderRecord` the lock file contains.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::executor::device_lock`
Expected: PASS, 3 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/executor/device_lock.rs
git commit -m "feat(kms): add the COMMIT-7 device lock and start-time check"
```

---

### Task 13: The evidence recorder

**Files:**
- Create: `crates/yserver/src/kms/evidence.rs`
- Modify: `crates/yserver/src/kms/mod.rs`

**Interfaces:**
- Consumes: `CommitId`, `IncarnationId` (task 4).
- Produces:
  - `pub(crate) struct LatencyRecorder { .. }` with `fn with_capacity(samples: usize) -> Self`, `fn record(&mut self, sample: HostCallSample) -> RecordOutcome`, `fn export(self) -> Result<Vec<HostCallSample>, EvidenceInsufficient>`
  - `pub(crate) enum RecordOutcome { Recorded, Exhausted }`

The buffer is preallocated for the whole declared arm, is single-writer, never wraps, and does no allocation, filesystem write or flush on the measured path. Exhaustion is not a wrap and not a silent drop: it marks the row `EvidenceInsufficient`.

- [x] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_recorder_never_wraps_and_reports_exhaustion() {
        let mut recorder = LatencyRecorder::with_capacity(2);
        assert_eq!(recorder.record(sample(1)), RecordOutcome::Recorded);
        assert_eq!(recorder.record(sample(2)), RecordOutcome::Recorded);
        assert_eq!(recorder.record(sample(3)), RecordOutcome::Exhausted);
        // The third sample must not have overwritten the first.
        assert_eq!(recorder.peek_for_tests(0).round_trip_ns, 1);
    }

    #[test]
    fn an_exhausted_arm_exports_as_evidence_insufficient() {
        let mut recorder = LatencyRecorder::with_capacity(1);
        recorder.record(sample(1));
        recorder.record(sample(2));
        assert!(matches!(recorder.export(), Err(EvidenceInsufficient::RecorderExhausted)));
    }

    #[test]
    fn a_complete_arm_exports_every_sample_in_order() {
        let mut recorder = LatencyRecorder::with_capacity(3);
        for value in 1..=3 {
            recorder.record(sample(value));
        }
        let exported = recorder.export().expect("complete arm");
        assert_eq!(exported.iter().map(|s| s.round_trip_ns).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn recording_does_not_allocate_after_construction() {
        // Capacity is fixed at construction; `record` only writes into it.
        let mut recorder = LatencyRecorder::with_capacity(64);
        let before = recorder.capacity_for_tests();
        for value in 0..64 {
            recorder.record(sample(value));
        }
        assert_eq!(recorder.capacity_for_tests(), before, "the buffer must not grow");
    }
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver --lib kms::evidence`
Expected: FAIL to compile.

- [x] **Step 3: Write minimal implementation**

Back the recorder with a `Box<[HostCallSample]>` allocated once in `with_capacity` plus a write cursor and an `exhausted` flag. `record` is a bounds check and a store.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver --lib kms::evidence`
Expected: PASS, 4 tests.

- [x] **Step 5: Commit**

```bash
git add crates/yserver/src/kms/evidence.rs crates/yserver/src/kms/mod.rs
git commit -m "feat(kms): add the fixed-capacity evidence recorder"
```

---

### Task 14: Portable compile gates — the stage reviewability gate

**Files:**
- Create: `.github/workflows/portable-build.yml` *(only if a workflow directory already exists; otherwise document the three commands in `CONTRIBUTING.md`)*

**Interfaces:**
- Consumes: every prior task.
- Produces: no code. This task is the gate the spec places on stage 1 being reviewable at all.

- [x] **Step 1: Add the musl target and build**

```bash
rustup target add x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-linux-musl
```
Expected: clean. `libc::Ioctl` does not exist on musl, which is exactly what the task 1 boundary exists to absorb; a failure here means a raw ioctl type leaked back into a call site.

- [x] **Step 2: Add the FreeBSD target and check**

```bash
rustup target add x86_64-unknown-freebsd
cargo check -p yserver --target x86_64-unknown-freebsd
```
Expected: clean.

- [x] **Step 3: Run the full suite on the host toolchain**

```bash
cargo test -p yserver
cargo +nightly fmt --check
```
Expected: PASS, no diff.

- [x] **Step 4: Confirm the stage's own removals landed**

```bash
grep -rn 'receive_events\|ABSOLUTE_SEQ_TAG\|absolute_seq_user_data\|crtc_queue_sequence_unsupported_devices' crates/yserver/src/
```
Expected: no matches.

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "ci(kms): gate stage 1 on the glibc, musl and FreeBSD builds"
```

---

## Stage exit criteria

Stage 1 is reviewable when all of the following hold. These come from section 18's own wording, not from this plan.

- The three portable builds pass and the full suite is green.
- `receive_events`, the `Event::PageFlip` compatibility drain, the manual `Event::Unknown` sequence parser, the `page_flip` `IoctlReq` alias, the raw-CRTC/high-bit sequence encodings and `crtc_queue_sequence_unsupported_devices` are all gone.
- The executor exists end to end and is exercised against the stub helper: protocol, transport with fd passing, spawn, class-derived watchdog, three-outcome dispatch, leases, reap and the device lock. **No production `atomic_commit` call site is converted in this stage.**
- No path converts an `Unknown` outcome into a `Rejected` one.
- No lease is released without a wait status.

## What stage 2 consumes

Stage 1 is not the conversion. These are the stage-1 products stage 2 spends,
listed so neither stage is judged against the other's outcome:

- The six real `atomic_commit` call sites (`page_flip.rs:217`,
  `modeset.rs:1144,1305,1562,1635,1690`) move behind `KmsIoExecutor` in stage 2,
  together with the device owner whose admission and completion model they need.
- `SubmittingProof` (task 9) gets its producer in stage 2 when the owner installs
  `Submitting`/`CoordinateSubmitting`. Until then no call site can dispatch
  without constructing it through the test-only constructor.
- `may_install_state` (task 12) gets its production caller at real device open,
  in `resolve_drm_device` (`crates/yserver/src/lib.rs:658`).

## Self-review notes

Checked against the spec after writing:

- **Coverage.** Every stage-1 item in section 18 maps to a task: executor and framing (7, 8, 9, 10), fd passing (8), leases and reap (11), the `COMMIT-7` device lock (12), `CommitId`/`EventToken`/sequence-arm/clock-epoch identities (4), the single raw parser (2, 3), the portable ioctl ABI boundary (1), epoch-local sequence classification (6), validation/watchdog/evidence primitives (9, 13), and the five removals (1, 3, 5, 6). The glibc/musl/FreeBSD gates and the synthetic malformed/concatenated event tests are task 14 and task 2.
- **Deliberate deferral.** Converting the six real `atomic_commit` call sites belongs to stage 2, because they need the device owner's admission and completion model to exist first — the spec's own ordering argument. Stage 1 therefore proves the executor against a stub helper. This is recorded in the exit criteria so it cannot be mistaken for an omission.
- **Type consistency.** `EventToken` and `SequenceArmToken` both expose `as_user_data`/`from_user_data` and both reject zero; the purpose tag in task 4 is what makes task 5's cross-purpose rejection test pass. `HostCallOutcome` is named identically in tasks 9, 10 and 11.
- **Known gap to close during execution.** Task 12's `may_install_state` needs a caller at real device open; that call site lives in `resolve_drm_device` (`crates/yserver/src/lib.rs:658`) and should be wired when stage 2 opens the incarnation. Task 12 delivers and tests the mechanism; the production call site is stage 2's.
