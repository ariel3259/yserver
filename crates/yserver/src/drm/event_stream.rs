//! The single raw DRM event-stream parser and its fd-draining half.
//!
//! All DRM event types share one byte stream on the device fd, so C.0 owns
//! the whole drain rather than racing a second reader or raw-parsing only
//! selected events. Every header length is validated before the cursor
//! advances: a zero, undersized, over-buffer, truncated or overflowing length
//! is malformed input, never a reason to read out of bounds or to stop making
//! progress.
//!
//! [`drain_device_events`] drains to `EAGAIN` unconditionally rather than
//! stopping at a short read: `present/event_loop.rs` registers the DRM fd
//! with raw epoll (level-triggered), but the KMS backend fd goes through the
//! core poller's mio registration (edge-triggered), where a residue left
//! behind by a single short read is never re-reported and would surface as a
//! permanently missing completion. Draining to `EAGAIN` is correct under
//! both trigger modes. This requires the fd to be non-blocking — see
//! [`drain_fd_events`]'s doc comment.

use std::{io, os::fd::AsFd};

pub(crate) const DRM_EVENT_VBLANK: u32 = 0x01;
pub(crate) const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
pub(crate) const DRM_EVENT_CRTC_SEQUENCE: u32 = 0x03;

const HEADER_LEN: usize = 8;
const VBLANK_LEN: usize = 32;
const CRTC_SEQUENCE_LEN: usize = 32;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DrmEventRecord {
    PageFlip {
        crtc_id: u32,
        sequence: u32,
        tv_sec: u32,
        tv_usec: u32,
        user_data: u64,
    },
    Vblank {
        crtc_id: u32,
        sequence: u32,
        tv_sec: u32,
        tv_usec: u32,
        user_data: u64,
    },
    CrtcSequence {
        user_data: u64,
        time_ns: i64,
        sequence: u64,
    },
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

/// Parse as far as the buffer allows, returning what decoded plus the
/// failure that stopped it, if any. Callers dispatch the good records
/// first: the bytes are already consumed by `read(2)` and cannot be read
/// again, so discarding them on a later malformed event would lose real
/// completions. The error still reaches the caller, which routes it to the
/// poison boundary rather than swallowing it.
pub(crate) fn parse_event_buffer_partial(
    bytes: &[u8],
) -> (Vec<DrmEventRecord>, Option<EventParseError>) {
    let mut records = Vec::new();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        if bytes.len() - cursor < HEADER_LEN {
            return (
                records,
                Some(EventParseError::Malformed(
                    "trailing bytes cannot hold an event header",
                )),
            );
        }
        let kind = u32_at(bytes, cursor);
        let length = u32_at(bytes, cursor + 4) as usize;

        // The cursor must always advance by at least a header, or a hostile
        // or corrupt stream would spin here forever.
        if length < HEADER_LEN {
            return (
                records,
                Some(EventParseError::Malformed(
                    "event length is shorter than its header",
                )),
            );
        }
        if length > bytes.len() - cursor {
            return (
                records,
                Some(EventParseError::Malformed(
                    "event length overruns the buffer",
                )),
            );
        }
        let body = &bytes[cursor..cursor + length];

        match kind {
            DRM_EVENT_VBLANK | DRM_EVENT_FLIP_COMPLETE => {
                if length != VBLANK_LEN {
                    return (
                        records,
                        Some(EventParseError::Malformed(
                            "vblank event has the wrong length",
                        )),
                    );
                }
                let user_data = u64_at(body, 8);
                let tv_sec = u32_at(body, 16);
                let tv_usec = u32_at(body, 20);
                let sequence = u32_at(body, 24);
                let crtc_id = u32_at(body, 28);
                records.push(if kind == DRM_EVENT_FLIP_COMPLETE {
                    DrmEventRecord::PageFlip {
                        crtc_id,
                        sequence,
                        tv_sec,
                        tv_usec,
                        user_data,
                    }
                } else {
                    DrmEventRecord::Vblank {
                        crtc_id,
                        sequence,
                        tv_sec,
                        tv_usec,
                        user_data,
                    }
                });
            }
            DRM_EVENT_CRTC_SEQUENCE => {
                if length != CRTC_SEQUENCE_LEN {
                    return (
                        records,
                        Some(EventParseError::Malformed(
                            "sequence event has the wrong length",
                        )),
                    );
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

    (records, None)
}

#[cfg(test)]
pub(crate) fn parse_event_buffer(bytes: &[u8]) -> Result<Vec<DrmEventRecord>, EventParseError> {
    match parse_event_buffer_partial(bytes) {
        (records, None) => Ok(records),
        (_, Some(error)) => Err(error),
    }
}

/// One read is one drain step: the kernel returns whole events, and the
/// parser rejects any buffer that does not decompose into whole events.
const DRAIN_BUFFER_LEN: usize = 1024;

/// Read `fd` and dispatch every decoded [`DrmEventRecord`] to `on_record`,
/// looping until the kernel reports `EAGAIN`/`EWOULDBLOCK` (or EOF).
///
/// This requires `fd` to be non-blocking. A single short read is not a
/// terminating condition here — see the module doc comment for why the
/// drain must not stop early — so on a blocking fd the final iteration
/// (the one that is supposed to observe "no more data") instead blocks
/// waiting for the next event, which can stall the caller's poll loop
/// indefinitely. The caller is responsible for registering a non-blocking
/// fd; see this task's report for the current state of that guarantee.
pub(crate) fn drain_fd_events<F>(fd: &impl AsFd, mut on_record: F) -> io::Result<()>
where
    F: FnMut(DrmEventRecord),
{
    let mut buffer = [0u8; DRAIN_BUFFER_LEN];
    loop {
        let read = match raw_read(fd, &mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let (records, error) = parse_event_buffer_partial(&buffer[..read]);
        for record in records {
            on_record(record);
        }
        if let Some(EventParseError::Malformed(why)) = error {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed DRM event stream: {why}"),
            ));
        }
        // Deliberately no `read < DRAIN_BUFFER_LEN` early return: under
        // mio's edge-triggered registration a residue is never
        // re-reported, so the loop continues until the kernel says the
        // queue is empty.
    }
}

fn raw_read(fd: &impl AsFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buffer` is a valid mutable slice for its full length and
    // `fd` is a live descriptor for the duration of the call; `read`
    // writes at most `buffer.len()` bytes into it and reports the count.
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

/// Drain every pending DRM event on `device`'s fd. All DRM event types
/// share one byte stream, so this is the only reader: no call site may
/// subscribe to a subset of event types (see the module doc comment).
pub(crate) fn drain_device_events<F>(device: &crate::drm::Device, on_record: F) -> io::Result<()>
where
    F: FnMut(DrmEventRecord),
{
    drain_fd_events(device, on_record)
}

/// Test-only pipe helper: a pipe whose read end is already non-blocking,
/// standing in for the DRM fd. `drain_fd_events` requires a non-blocking
/// fd (see its doc comment); std's `std::io::pipe` has no
/// `set_nonblocking`, so this goes straight to `pipe2(2)` with
/// `O_NONBLOCK` set atomically on creation.
#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        fs::File,
        io,
        os::fd::{FromRawFd, OwnedFd},
    };

    pub(crate) fn nonblocking_pipe() -> io::Result<(File, File)> {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a valid 2-element buffer for the kernel to fill;
        // pipe2 sets O_NONBLOCK on both ends atomically with creation.
        let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `pipe2` returned successfully, so `fds[0]` and `fds[1]`
        // are freshly opened, valid, and uniquely owned here.
        let reader = unsafe { File::from(OwnedFd::from_raw_fd(fds[0])) };
        let writer = unsafe { File::from(OwnedFd::from_raw_fd(fds[1])) };
        Ok((reader, writer))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DRM_EVENT_CRTC_SEQUENCE, DRM_EVENT_FLIP_COMPLETE, DRM_EVENT_VBLANK, DrmEventRecord,
        parse_event_buffer,
    };

    fn vblank_bytes(
        kind: u32,
        crtc_id: u32,
        sequence: u32,
        tv_sec: u32,
        tv_usec: u32,
        user_data: u64,
    ) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&kind.to_ne_bytes());
        b[4..8].copy_from_slice(&32u32.to_ne_bytes());
        b[8..16].copy_from_slice(&user_data.to_ne_bytes());
        b[16..20].copy_from_slice(&tv_sec.to_ne_bytes());
        b[20..24].copy_from_slice(&tv_usec.to_ne_bytes());
        b[24..28].copy_from_slice(&sequence.to_ne_bytes());
        b[28..32].copy_from_slice(&crtc_id.to_ne_bytes());
        b
    }

    #[test]
    fn decodes_a_flip_complete_with_its_crtc_id_preserved() {
        // tv_sec and tv_usec are distinct and non-zero so a transposed read
        // of the two offsets (20 vs 24 relative to the event base) fails
        // this assertion instead of passing by coincidence. user_data does
        // not fit in 32 bits, so truncating the read to `u32_at(.., 8) as
        // u64` instead of a real `u64_at` also fails here.
        let bytes = vblank_bytes(
            DRM_EVENT_FLIP_COMPLETE,
            0x42,
            7,
            11,
            22,
            0x1122_3344_5566_7788,
        );
        let records = parse_event_buffer(&bytes).expect("well-formed buffer");
        assert_eq!(
            records,
            vec![DrmEventRecord::PageFlip {
                crtc_id: 0x42,
                sequence: 7,
                tv_sec: 11,
                tv_usec: 22,
                user_data: 0x1122_3344_5566_7788,
            }]
        );
    }

    #[test]
    fn decodes_a_vblank_event_distinct_from_a_page_flip() {
        // Type 0x01 (DRM_EVENT_VBLANK) must decode into the Vblank variant,
        // never PageFlip -- a vblank mistagged as a flip completion would
        // later retire a swapchain image for a flip that never completed.
        let bytes = vblank_bytes(DRM_EVENT_VBLANK, 0x7, 3, 5, 6, 0xFEED_FACE);
        let records = parse_event_buffer(&bytes).expect("well-formed buffer");
        assert_eq!(
            records,
            vec![DrmEventRecord::Vblank {
                crtc_id: 0x7,
                sequence: 3,
                tv_sec: 5,
                tv_usec: 6,
                user_data: 0xFEED_FACE,
            }]
        );
    }

    #[test]
    fn empty_buffer_decodes_to_no_records() {
        assert_eq!(parse_event_buffer(&[]), Ok(vec![]));
    }

    #[test]
    fn decodes_two_concatenated_events_in_order() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 10, 0, 0, 100));
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 2, 20, 0, 0, 200));
        let records = parse_event_buffer(&buf).expect("well-formed buffer");
        assert_eq!(records.len(), 2);
        assert!(matches!(
            records[0],
            DrmEventRecord::PageFlip { crtc_id: 1, .. }
        ));
        assert!(matches!(
            records[1],
            DrmEventRecord::PageFlip { crtc_id: 2, .. }
        ));
    }

    #[test]
    fn skips_an_unknown_but_well_formed_event_by_its_declared_length() {
        let mut buf = Vec::new();
        let mut unknown = [0u8; 16];
        unknown[0..4].copy_from_slice(&99u32.to_ne_bytes());
        unknown[4..8].copy_from_slice(&16u32.to_ne_bytes());
        buf.extend_from_slice(&unknown);
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 5, 1, 0, 0, 0));
        let records = parse_event_buffer(&buf).expect("unknown well-formed event is skippable");
        assert_eq!(records.len(), 1, "only the known event decodes");
        assert!(matches!(
            records[0],
            DrmEventRecord::PageFlip { crtc_id: 5, .. }
        ));
    }

    #[test]
    fn rejects_a_zero_length_event_instead_of_looping() {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        buf[4..8].copy_from_slice(&0u32.to_ne_bytes());
        assert!(
            parse_event_buffer(&buf).is_err(),
            "zero length must not advance by zero"
        );
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
        assert!(
            parse_event_buffer(&buf).is_err(),
            "declared length overruns the buffer"
        );
    }

    #[test]
    fn rejects_a_truncated_trailing_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 1, 0, 0, 0));
        buf.extend_from_slice(&[0u8; 3]);
        assert!(
            parse_event_buffer(&buf).is_err(),
            "3 trailing bytes cannot hold a header"
        );
    }

    #[test]
    fn rejects_a_known_type_carrying_the_wrong_length() {
        let mut bytes = vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 1, 0, 0, 0);
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

    #[test]
    fn a_malformed_tail_does_not_discard_the_records_before_it() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 7, 1, 0, 0, 0));
        buf.extend_from_slice(&[0u8; 3]); // cannot hold a header
        let (records, error) = super::parse_event_buffer_partial(&buf);
        assert_eq!(
            records.len(),
            1,
            "the good record must survive the bad tail"
        );
        assert!(error.is_some(), "and the failure must still be reported");
    }

    #[test]
    fn drain_continues_past_a_full_buffer_until_the_queue_is_empty() {
        // More events than one DRAIN_BUFFER_LEN read can return. A drain that
        // stopped at the first short read would strand the remainder, which is
        // unrecoverable on the edge-triggered path.
        let (reader, mut writer) = super::test_support::nonblocking_pipe().expect("pipe");
        let count = (super::DRAIN_BUFFER_LEN / 32) + 5;
        let mut buf = Vec::new();
        for index in 0..count {
            buf.extend_from_slice(&vblank_bytes(
                DRM_EVENT_FLIP_COMPLETE,
                index as u32 + 1,
                0,
                0,
                0,
                0,
            ));
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
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 10, 0, 0, 0));
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 2, 20, 0, 0, 0));
        writer.write_all(&buf).expect("write");
        drop(writer);

        let mut seen = Vec::new();
        super::drain_fd_events(&reader, |record| seen.push(record)).expect("drain");
        assert_eq!(seen.len(), 2);
        assert!(matches!(
            seen[0],
            DrmEventRecord::PageFlip { crtc_id: 1, .. }
        ));
        assert!(matches!(
            seen[1],
            DrmEventRecord::PageFlip { crtc_id: 2, .. }
        ));
    }
}
