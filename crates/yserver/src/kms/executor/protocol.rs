//! Private wire protocol for process-isolated executor host calls.

use crate::kms::owner::identity::{ClockEpochId, CommitId, EventToken, IncarnationId};

pub(crate) const PROTOCOL_MAGIC: [u8; 4] = *b"YSKX";
pub(crate) const PROTOCOL_VERSION: u16 = 1;
const KIND_ATOMIC_REQUEST: u16 = 1;
const KIND_CLOCK_PROBE_REQUEST: u16 = 2;
const KIND_REPLY: u16 = 3;

const HEADER_LEN: usize = 12;
const REQUEST_PAYLOAD_LEN: usize = 64;
const REPLY_PAYLOAD_LEN: usize = 32;

#[allow(dead_code)] // Will be consumed in Task 8
pub(crate) const REQUEST_FRAME_LEN: usize = HEADER_LEN + REQUEST_PAYLOAD_LEN;

#[allow(dead_code)] // Will be consumed in Task 8
pub(crate) const REPLY_FRAME_LEN: usize = HEADER_LEN + REPLY_PAYLOAD_LEN;

const REPLY_TAG_ACCEPTED: u16 = 1;
const REPLY_TAG_REJECTED: u16 = 2;
const REPLY_TAG_CLOCK_PROBE: u16 = 3;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProtocolError {
    Magic,
    Version(u16),
    Kind(u16),
    Length,
    Field(&'static str),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct RequestSeq(pub(crate) u64);

impl RequestSeq {
    #[doc(hidden)]
    #[allow(dead_code)] // Will be consumed in Task 8, 9, 10
    pub(crate) const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    #[allow(dead_code)] // Will be consumed in Task 8, 9, 10
    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    #[allow(dead_code)] // Will be consumed in Task 9, 10
    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[allow(dead_code)] // Will be consumed in Task 9, 10
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum HostCallRequest {
    Atomic(AtomicRequest),
    ClockProbe(ClockProbeRequest),
}

#[allow(dead_code)] // Will be consumed in Task 9, 10
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct AtomicRequest {
    pub(crate) seq: RequestSeq,
    pub(crate) incarnation: IncarnationId,
    pub(crate) epoch: ClockEpochId,
    pub(crate) transition: Option<u64>,
    pub(crate) commit: CommitId,
    pub(crate) event_token: EventToken,
    pub(crate) flags: u32,
    pub(crate) payload_len: u32,
}

#[allow(dead_code)] // Will be consumed in Task 9, 10
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ClockProbeRequest {
    pub(crate) seq: RequestSeq,
    pub(crate) incarnation: IncarnationId,
    pub(crate) epoch: ClockEpochId,
    pub(crate) topology_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) clock_epoch: ClockEpochId,
    pub(crate) probe: u64,
}

#[allow(dead_code)] // Will be consumed in Task 8, 9, 10
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum HostCallReply {
    Accepted {
        seq: RequestSeq,
        helper_duration_ns: u64,
        out_fence_count: u8,
    },
    Rejected {
        seq: RequestSeq,
        errno: i32,
        helper_duration_ns: u64,
    },
    ClockProbe {
        seq: RequestSeq,
        sequence: u64,
        helper_duration_ns: u64,
    },
}

fn encode_header(frame: &mut [u8], kind: u16, payload_len: usize) {
    frame[..4].copy_from_slice(&PROTOCOL_MAGIC);
    frame[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    frame[6..8].copy_from_slice(&kind.to_le_bytes());
    frame[8..12].copy_from_slice(&(payload_len as u32).to_le_bytes());
}

fn decode_request_header(frame: &[u8]) -> Result<u16, ProtocolError> {
    if frame.len() != REQUEST_FRAME_LEN {
        return Err(ProtocolError::Length);
    }
    if frame[..4] != PROTOCOL_MAGIC {
        return Err(ProtocolError::Magic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::Version(version));
    }
    let kind = u16::from_le_bytes([frame[6], frame[7]]);
    if kind != KIND_ATOMIC_REQUEST && kind != KIND_CLOCK_PROBE_REQUEST {
        return Err(ProtocolError::Kind(kind));
    }
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
    if payload_len as usize != REQUEST_PAYLOAD_LEN {
        return Err(ProtocolError::Length);
    }
    Ok(kind)
}

fn decode_reply_header(frame: &[u8]) -> Result<(), ProtocolError> {
    if frame.len() != REPLY_FRAME_LEN {
        return Err(ProtocolError::Length);
    }
    if frame[..4] != PROTOCOL_MAGIC {
        return Err(ProtocolError::Magic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::Version(version));
    }
    let kind = u16::from_le_bytes([frame[6], frame[7]]);
    if kind != KIND_REPLY {
        return Err(ProtocolError::Kind(kind));
    }
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
    if payload_len as usize != REPLY_PAYLOAD_LEN {
        return Err(ProtocolError::Length);
    }
    Ok(())
}

fn put_u8(frame: &mut [u8], cursor: &mut usize, value: u8) {
    frame[*cursor] = value;
    *cursor += 1;
}

fn put_u16(frame: &mut [u8], cursor: &mut usize, value: u16) {
    frame[*cursor..*cursor + 2].copy_from_slice(&value.to_le_bytes());
    *cursor += 2;
}

fn put_u32(frame: &mut [u8], cursor: &mut usize, value: u32) {
    frame[*cursor..*cursor + 4].copy_from_slice(&value.to_le_bytes());
    *cursor += 4;
}

fn put_i32(frame: &mut [u8], cursor: &mut usize, value: i32) {
    frame[*cursor..*cursor + 4].copy_from_slice(&value.to_le_bytes());
    *cursor += 4;
}

fn put_u64(frame: &mut [u8], cursor: &mut usize, value: u64) {
    frame[*cursor..*cursor + 8].copy_from_slice(&value.to_le_bytes());
    *cursor += 8;
}

fn put_bytes(frame: &mut [u8], cursor: &mut usize, value: &[u8]) {
    frame[*cursor..*cursor + value.len()].copy_from_slice(value);
    *cursor += value.len();
}

fn take_u8(frame: &[u8], cursor: &mut usize) -> Result<u8, ProtocolError> {
    let byte = *frame.get(*cursor).ok_or(ProtocolError::Length)?;
    *cursor += 1;
    Ok(byte)
}

fn take_u16(frame: &[u8], cursor: &mut usize) -> Result<u16, ProtocolError> {
    let end = cursor.saturating_add(2);
    let bytes = frame.get(*cursor..end).ok_or(ProtocolError::Length)?;
    *cursor = end;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn take_u32(frame: &[u8], cursor: &mut usize) -> Result<u32, ProtocolError> {
    let end = cursor.saturating_add(4);
    let bytes = frame.get(*cursor..end).ok_or(ProtocolError::Length)?;
    *cursor = end;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_i32(frame: &[u8], cursor: &mut usize) -> Result<i32, ProtocolError> {
    let end = cursor.saturating_add(4);
    let bytes = frame.get(*cursor..end).ok_or(ProtocolError::Length)?;
    *cursor = end;
    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_u64(frame: &[u8], cursor: &mut usize) -> Result<u64, ProtocolError> {
    let end = cursor.saturating_add(8);
    let bytes = frame.get(*cursor..end).ok_or(ProtocolError::Length)?;
    *cursor = end;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn take_bytes<'a>(
    frame: &'a [u8],
    cursor: &mut usize,
    len: usize,
) -> Result<&'a [u8], ProtocolError> {
    let end = cursor.saturating_add(len);
    let slice = frame.get(*cursor..end).ok_or(ProtocolError::Length)?;
    *cursor = end;
    Ok(slice)
}

fn decode_commit_id(raw: u64) -> Result<CommitId, ProtocolError> {
    if raw == 0 {
        return Err(ProtocolError::Field("commit"));
    }
    Ok(CommitId::from_raw(raw))
}

fn decode_incarnation_id(raw: u64) -> Result<IncarnationId, ProtocolError> {
    if raw == 0 {
        return Err(ProtocolError::Field("incarnation"));
    }
    Ok(IncarnationId::from_raw(raw))
}

fn decode_clock_epoch_id(raw: u64, field: &'static str) -> Result<ClockEpochId, ProtocolError> {
    if raw == 0 {
        return Err(ProtocolError::Field(field));
    }
    Ok(ClockEpochId::from_raw(raw))
}

#[allow(dead_code)] // Will be consumed in Task 9, 10
pub(crate) fn encode_request(request: &HostCallRequest) -> [u8; REQUEST_FRAME_LEN] {
    let mut frame = [0u8; REQUEST_FRAME_LEN];
    match request {
        HostCallRequest::Atomic(req) => {
            encode_header(&mut frame, KIND_ATOMIC_REQUEST, REQUEST_PAYLOAD_LEN);
            let mut cursor = HEADER_LEN;
            put_u64(&mut frame, &mut cursor, req.seq.get());
            put_u64(&mut frame, &mut cursor, req.incarnation.get());
            put_u64(&mut frame, &mut cursor, req.epoch.get());
            match req.transition {
                Some(trans) => {
                    put_u64(&mut frame, &mut cursor, 1);
                    put_u64(&mut frame, &mut cursor, trans);
                }
                None => {
                    put_u64(&mut frame, &mut cursor, 0);
                    put_u64(&mut frame, &mut cursor, 0);
                }
            }
            put_u64(&mut frame, &mut cursor, req.commit.get());
            put_u64(&mut frame, &mut cursor, req.event_token.as_user_data());
            put_u32(&mut frame, &mut cursor, req.flags);
            put_u32(&mut frame, &mut cursor, req.payload_len);
            debug_assert_eq!(cursor, REQUEST_FRAME_LEN);
        }
        HostCallRequest::ClockProbe(req) => {
            encode_header(&mut frame, KIND_CLOCK_PROBE_REQUEST, REQUEST_PAYLOAD_LEN);
            let mut cursor = HEADER_LEN;
            put_u64(&mut frame, &mut cursor, req.seq.get());
            put_u64(&mut frame, &mut cursor, req.incarnation.get());
            put_u64(&mut frame, &mut cursor, req.epoch.get());
            put_u64(&mut frame, &mut cursor, req.topology_generation);
            put_u32(&mut frame, &mut cursor, req.crtc_id);
            put_u32(&mut frame, &mut cursor, 0); // reserved
            put_u64(&mut frame, &mut cursor, req.clock_epoch.get());
            put_u64(&mut frame, &mut cursor, req.probe);
            put_u64(&mut frame, &mut cursor, 0); // reserved
            debug_assert_eq!(cursor, REQUEST_FRAME_LEN);
        }
    }
    frame
}

#[allow(dead_code)] // Will be consumed in Task 10
pub(crate) fn decode_request(frame: &[u8]) -> Result<HostCallRequest, ProtocolError> {
    let kind = decode_request_header(frame)?;
    let mut cursor = HEADER_LEN;
    let seq_raw = take_u64(frame, &mut cursor)?;
    let seq = RequestSeq::from_raw(seq_raw);
    match kind {
        KIND_ATOMIC_REQUEST => {
            let incarnation_raw = take_u64(frame, &mut cursor)?;
            let incarnation = decode_incarnation_id(incarnation_raw)?;
            let epoch_raw = take_u64(frame, &mut cursor)?;
            let epoch = decode_clock_epoch_id(epoch_raw, "epoch")?;
            let has_transition = take_u64(frame, &mut cursor)?;
            let transition_val = take_u64(frame, &mut cursor)?;
            let transition = match has_transition {
                0 if transition_val == 0 => None,
                0 => return Err(ProtocolError::Field("transition")),
                1 => Some(transition_val),
                _ => return Err(ProtocolError::Field("transition")),
            };
            let commit_raw = take_u64(frame, &mut cursor)?;
            let commit = decode_commit_id(commit_raw)?;
            let event_token_raw = take_u64(frame, &mut cursor)?;
            let event_token = EventToken::from_user_data(event_token_raw)
                .ok_or(ProtocolError::Field("event_token"))?;
            let flags = take_u32(frame, &mut cursor)?;
            let payload_len = take_u32(frame, &mut cursor)?;
            debug_assert_eq!(cursor, REQUEST_FRAME_LEN);
            Ok(HostCallRequest::Atomic(AtomicRequest {
                seq,
                incarnation,
                epoch,
                transition,
                commit,
                event_token,
                flags,
                payload_len,
            }))
        }
        KIND_CLOCK_PROBE_REQUEST => {
            let incarnation_raw = take_u64(frame, &mut cursor)?;
            let incarnation = decode_incarnation_id(incarnation_raw)?;
            let epoch_raw = take_u64(frame, &mut cursor)?;
            let epoch = decode_clock_epoch_id(epoch_raw, "epoch")?;
            let topology_generation = take_u64(frame, &mut cursor)?;
            let crtc_id = take_u32(frame, &mut cursor)?;
            if crtc_id == 0 {
                return Err(ProtocolError::Field("crtc_id"));
            }
            let reserved_32 = take_u32(frame, &mut cursor)?;
            if reserved_32 != 0 {
                return Err(ProtocolError::Field("reserved"));
            }
            let clock_epoch_raw = take_u64(frame, &mut cursor)?;
            let clock_epoch = decode_clock_epoch_id(clock_epoch_raw, "clock_epoch")?;
            let probe = take_u64(frame, &mut cursor)?;
            let reserved_64 = take_u64(frame, &mut cursor)?;
            if reserved_64 != 0 {
                return Err(ProtocolError::Field("reserved"));
            }
            debug_assert_eq!(cursor, REQUEST_FRAME_LEN);
            Ok(HostCallRequest::ClockProbe(ClockProbeRequest {
                seq,
                incarnation,
                epoch,
                topology_generation,
                crtc_id,
                clock_epoch,
                probe,
            }))
        }
        other => Err(ProtocolError::Kind(other)),
    }
}

#[allow(dead_code)] // Will be consumed in Task 10
pub(crate) fn encode_reply(reply: &HostCallReply) -> [u8; REPLY_FRAME_LEN] {
    let mut frame = [0u8; REPLY_FRAME_LEN];
    encode_header(&mut frame, KIND_REPLY, REPLY_PAYLOAD_LEN);
    let mut cursor = HEADER_LEN;
    match reply {
        HostCallReply::Accepted {
            seq,
            helper_duration_ns,
            out_fence_count,
        } => {
            put_u16(&mut frame, &mut cursor, REPLY_TAG_ACCEPTED);
            put_u16(&mut frame, &mut cursor, 0); // reserved
            put_u64(&mut frame, &mut cursor, seq.get());
            put_u64(&mut frame, &mut cursor, *helper_duration_ns);
            put_u8(&mut frame, &mut cursor, *out_fence_count);
            put_bytes(&mut frame, &mut cursor, &[0u8; 11]);
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
        }
        HostCallReply::Rejected {
            seq,
            errno,
            helper_duration_ns,
        } => {
            put_u16(&mut frame, &mut cursor, REPLY_TAG_REJECTED);
            put_u16(&mut frame, &mut cursor, 0); // reserved
            put_u64(&mut frame, &mut cursor, seq.get());
            put_i32(&mut frame, &mut cursor, *errno);
            put_u32(&mut frame, &mut cursor, 0); // reserved
            put_u64(&mut frame, &mut cursor, *helper_duration_ns);
            put_u32(&mut frame, &mut cursor, 0); // reserved
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
        }
        HostCallReply::ClockProbe {
            seq,
            sequence,
            helper_duration_ns,
        } => {
            put_u16(&mut frame, &mut cursor, REPLY_TAG_CLOCK_PROBE);
            put_u16(&mut frame, &mut cursor, 0); // reserved
            put_u64(&mut frame, &mut cursor, seq.get());
            put_u64(&mut frame, &mut cursor, *sequence);
            put_u64(&mut frame, &mut cursor, *helper_duration_ns);
            put_u32(&mut frame, &mut cursor, 0); // reserved
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
        }
    }
    frame
}

#[allow(dead_code)] // Will be consumed in Task 8, 9
pub(crate) fn decode_reply(frame: &[u8]) -> Result<HostCallReply, ProtocolError> {
    decode_reply_header(frame)?;
    let mut cursor = HEADER_LEN;
    let tag = take_u16(frame, &mut cursor)?;
    let reserved = take_u16(frame, &mut cursor)?;
    if reserved != 0 {
        return Err(ProtocolError::Field("reserved"));
    }
    let seq_raw = take_u64(frame, &mut cursor)?;
    let seq = RequestSeq::from_raw(seq_raw);
    match tag {
        REPLY_TAG_ACCEPTED => {
            let helper_duration_ns = take_u64(frame, &mut cursor)?;
            let out_fence_count = take_u8(frame, &mut cursor)?;
            let pad = take_bytes(frame, &mut cursor, 11)?;
            if pad.iter().any(|&b| b != 0) {
                return Err(ProtocolError::Field("reserved"));
            }
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
            Ok(HostCallReply::Accepted {
                seq,
                helper_duration_ns,
                out_fence_count,
            })
        }
        REPLY_TAG_REJECTED => {
            let errno = take_i32(frame, &mut cursor)?;
            let res1 = take_u32(frame, &mut cursor)?;
            if res1 != 0 {
                return Err(ProtocolError::Field("reserved"));
            }
            let helper_duration_ns = take_u64(frame, &mut cursor)?;
            let res2 = take_u32(frame, &mut cursor)?;
            if res2 != 0 {
                return Err(ProtocolError::Field("reserved"));
            }
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
            Ok(HostCallReply::Rejected {
                seq,
                errno,
                helper_duration_ns,
            })
        }
        REPLY_TAG_CLOCK_PROBE => {
            let sequence = take_u64(frame, &mut cursor)?;
            let helper_duration_ns = take_u64(frame, &mut cursor)?;
            let res = take_u32(frame, &mut cursor)?;
            if res != 0 {
                return Err(ProtocolError::Field("reserved"));
            }
            debug_assert_eq!(cursor, REPLY_FRAME_LEN);
            Ok(HostCallReply::ClockProbe {
                seq,
                sequence,
                helper_duration_ns,
            })
        }
        _ => Err(ProtocolError::Field("reply_tag")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_atomic() -> HostCallRequest {
        HostCallRequest::Atomic(AtomicRequest {
            seq: RequestSeq::for_tests(1),
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
        assert!(matches!(
            decode_request(&frame),
            Err(ProtocolError::Version(_))
        ));
    }

    #[test]
    fn a_short_frame_is_rejected() {
        let frame = encode_request(&sample_atomic());
        assert!(matches!(
            decode_request(&frame[..REQUEST_FRAME_LEN - 1]),
            Err(ProtocolError::Length)
        ));
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        let mut frame = encode_request(&sample_atomic());
        frame[6..8].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(
            decode_request(&frame),
            Err(ProtocolError::Kind(99))
        ));
    }

    #[test]
    fn a_rejected_reply_keeps_its_errno_and_helper_duration() {
        let reply = HostCallReply::Rejected {
            seq: RequestSeq::for_tests(1),
            errno: libc::EBUSY,
            helper_duration_ns: 12_345,
        };
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
        let reply = HostCallReply::Accepted {
            seq: RequestSeq::for_tests(2),
            helper_duration_ns: 900,
            out_fence_count: 2,
        };
        let frame = encode_reply(&reply);
        match decode_reply(&frame).expect("decodes") {
            HostCallReply::Accepted {
                out_fence_count, ..
            } => assert_eq!(out_fence_count, 2),
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn a_zero_event_token_is_rejected() {
        let mut frame = encode_request(&sample_atomic());
        // Header: 12 bytes; seq: 8, incarnation: 8, epoch: 8, transition: 16, commit: 8 -> event_token starts at 12 + 48 = 60
        frame[60..68].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(
            decode_request(&frame),
            Err(ProtocolError::Field("event_token"))
        );
    }

    #[test]
    fn an_atomic_request_without_transition_round_trips() {
        let request = HostCallRequest::Atomic(AtomicRequest {
            seq: RequestSeq::for_tests(2),
            incarnation: IncarnationId::first(),
            epoch: ClockEpochId::first(),
            transition: None,
            commit: CommitId::for_tests(5),
            event_token: EventToken::from_user_data(0x4000_0000_0000_0002).expect("valid"),
            flags: 0,
            payload_len: 256,
        });
        let frame = encode_request(&request);
        assert_eq!(decode_request(&frame).expect("decodes"), request);
    }

    #[test]
    fn a_clock_probe_request_round_trips() {
        let request = HostCallRequest::ClockProbe(ClockProbeRequest {
            seq: RequestSeq::for_tests(3),
            incarnation: IncarnationId::first(),
            epoch: ClockEpochId::first(),
            topology_generation: 12,
            crtc_id: 42,
            clock_epoch: ClockEpochId::first(),
            probe: 999,
        });
        let frame = encode_request(&request);
        assert_eq!(decode_request(&frame).expect("decodes"), request);
    }

    #[test]
    fn a_clock_probe_reply_round_trips() {
        let reply = HostCallReply::ClockProbe {
            seq: RequestSeq::for_tests(7),
            sequence: 123_456,
            helper_duration_ns: 50_000,
        };
        let frame = encode_reply(&reply);
        assert_eq!(decode_reply(&frame).expect("decodes"), reply);
    }
}
