//! The single raw DRM event-stream parser.
//!
//! All DRM event types share one byte stream on the device fd, so C.0 owns
//! the whole drain rather than racing a second reader or raw-parsing only
//! selected events. Every header length is validated before the cursor
//! advances: a zero, undersized, over-buffer, truncated or overflowing length
//! is malformed input, never a reason to read out of bounds or to stop making
//! progress.

// Task 3 (crates/yserver/src/drm/page_flip.rs's `drain_events` cutover) adds
// the real-fd reading half and gives every item below a non-test caller.
// Until then, `cargo clippy --all-targets` sees these as unreachable outside
// `#[cfg(test)]`, so each is allowed narrowly rather than blanket-allowed at
// the module level.
#[allow(dead_code)]
pub(crate) const DRM_EVENT_VBLANK: u32 = 0x01;
#[allow(dead_code)]
pub(crate) const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
#[allow(dead_code)]
pub(crate) const DRM_EVENT_CRTC_SEQUENCE: u32 = 0x03;

#[allow(dead_code)]
const HEADER_LEN: usize = 8;
#[allow(dead_code)]
const VBLANK_LEN: usize = 32;
#[allow(dead_code)]
const CRTC_SEQUENCE_LEN: usize = 32;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) enum EventParseError {
    Malformed(&'static str),
}

#[allow(dead_code)]
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_ne_bytes(raw)
}

#[allow(dead_code)]
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_ne_bytes(raw)
}

#[allow(dead_code)]
pub(crate) fn parse_event_buffer(bytes: &[u8]) -> Result<Vec<DrmEventRecord>, EventParseError> {
    let mut records = Vec::new();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        if bytes.len() - cursor < HEADER_LEN {
            return Err(EventParseError::Malformed(
                "trailing bytes cannot hold an event header",
            ));
        }
        let kind = u32_at(bytes, cursor);
        let length = u32_at(bytes, cursor + 4) as usize;

        // The cursor must always advance by at least a header, or a hostile
        // or corrupt stream would spin here forever.
        if length < HEADER_LEN {
            return Err(EventParseError::Malformed(
                "event length is shorter than its header",
            ));
        }
        if length > bytes.len() - cursor {
            return Err(EventParseError::Malformed(
                "event length overruns the buffer",
            ));
        }
        let body = &bytes[cursor..cursor + length];

        match kind {
            DRM_EVENT_VBLANK | DRM_EVENT_FLIP_COMPLETE => {
                if length != VBLANK_LEN {
                    return Err(EventParseError::Malformed(
                        "vblank event has the wrong length",
                    ));
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
                    return Err(EventParseError::Malformed(
                        "sequence event has the wrong length",
                    ));
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
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 5, 1, 0));
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
        buf.extend_from_slice(&vblank_bytes(DRM_EVENT_FLIP_COMPLETE, 1, 1, 0));
        buf.extend_from_slice(&[0u8; 3]);
        assert!(
            parse_event_buffer(&buf).is_err(),
            "3 trailing bytes cannot hold a header"
        );
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
