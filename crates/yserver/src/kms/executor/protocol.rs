//! Private wire protocol for process-isolated executor host calls.
//!
//! Two frame families share one transport. Host calls (`HostCallRequest` /
//! `HostCallReply`) carry a full `HostCallCorrelation`, because `ID-3`
//! requires every executor request and reply to carry the lifecycle epoch.
//! The startup handshake is its own family so the host-call accessors can be
//! total and bare — it still carries incarnation and lifecycle epoch. Each
//! decoder rejects the other family's kind.

use crate::kms::{
    executor::HostCallClass,
    owner::{
        identity::{ClockEpochId, CommitId, EventToken, IncarnationId},
        lifecycle::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId},
    },
};

pub(crate) const PROTOCOL_MAGIC: [u8; 4] = *b"YSKX";
pub(crate) const PROTOCOL_VERSION: u16 = 2;

const KIND_ATOMIC_REQUEST: u16 = 1;
const KIND_CLOCK_PROBE_REQUEST: u16 = 2;
const KIND_REPLY: u16 = 3;
#[allow(dead_code)] // Consumed by the handshake in task 6.
const KIND_HANDSHAKE_REQUEST: u16 = 4;
#[allow(dead_code)] // Consumed by the handshake in task 6.
const KIND_HANDSHAKE_REPLY: u16 = 5;

pub(crate) const HEADER_LEN: usize = 12;

/// Atomic head: six u64, then presence/class/pad, then four u32.
pub(crate) const ATOMIC_HEAD_LEN: usize = 68;
/// Probe head: six u64, then hardware CRTC and pad.
pub(crate) const PROBE_HEAD_LEN: usize = 56;
const _: () = assert!(ATOMIC_HEAD_LEN == 6 * 8 + 1 + 1 + 2 + 4 * 4);
const _: () = assert!(PROBE_HEAD_LEN == 6 * 8 + 4 + 4);

pub(crate) const MAX_ATOMIC_OBJECTS: usize = 256;
pub(crate) const MAX_ATOMIC_PROPS: usize = 1024;
pub(crate) const MAX_OUT_FENCES: usize = 16;
pub(crate) const MAX_REQUEST_FRAME_LEN: usize = 32 * 1024;

pub(crate) const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;
pub(crate) const DRM_MODE_ATOMIC_NONBLOCK: u32 = 0x0200;

/// Shared fixed-size encoding of a correlation tuple, used by replies so one
/// reply decoder serves both families.
const CORRELATION_LEN: usize = 56;
const CORRELATION_ATOMIC: u8 = 1;
const CORRELATION_PROBE: u8 = 2;

const REPLY_PAYLOAD_LEN: usize = 4 + CORRELATION_LEN + 16;
pub(crate) const REPLY_FRAME_LEN: usize = HEADER_LEN + REPLY_PAYLOAD_LEN;

#[allow(dead_code)] // Consumed by the handshake in task 6.
const HANDSHAKE_REQUEST_PAYLOAD_LEN: usize = 16;
#[allow(dead_code)] // Consumed by the handshake in task 6.
const HANDSHAKE_REPLY_PAYLOAD_LEN: usize = 20;

const REPLY_TAG_ACCEPTED: u8 = 1;
const REPLY_TAG_REJECTED: u8 = 2;
const REPLY_TAG_PROBE_ACCEPTED: u8 = 3;
const REPLY_TAG_PROBE_REJECTED: u8 = 4;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum ProtocolError {
    Magic,
    Version(u16),
    Kind(u16),
    Length,
    Field(&'static str),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[doc(hidden)]
pub struct RequestSeq(pub(crate) u64);

impl RequestSeq {
    #[doc(hidden)]
    pub const fn for_tests(raw: u64) -> Self {
        Self(raw)
    }

    #[doc(hidden)]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[doc(hidden)]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Which family a request or reply belongs to. A reply is current only when
/// its family matches the request's kind *and* its correlation is equal:
/// correlation equality alone would let a probe request accept an atomic
/// `Accepted` carrying the probe's own tuple.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum RequestKind {
    Atomic,
    ClockProbe,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostCallCorrelation {
    Atomic {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        transition: Option<LifecycleTransitionId>,
        commit: CommitId,
        event_token: EventToken,
    },
    ClockProbe {
        seq: RequestSeq,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        topology_generation: u64,
        hardware_crtc: u32,
        clock_epoch: ClockEpochId,
        probe: ClockProbeId,
    },
}

impl HostCallCorrelation {
    #[allow(dead_code)] // Consumed by the owner in 2b.
    #[doc(hidden)]
    pub const fn seq(self) -> RequestSeq {
        match self {
            Self::Atomic { seq, .. } | Self::ClockProbe { seq, .. } => seq,
        }
    }

    #[allow(dead_code)] // Consumed by poll_reply in task 4.
    #[doc(hidden)]
    pub const fn kind(self) -> RequestKind {
        match self {
            Self::Atomic { .. } => RequestKind::Atomic,
            Self::ClockProbe { .. } => RequestKind::ClockProbe,
        }
    }
}

/// The property arrays the kernel's atomic ioctl consumes. Counts must agree
/// with each other and stay inside the caps; `validate` is the single place
/// that decides, and both the encoder and the decoder call it.
#[derive(Debug, Clone, Eq, PartialEq)]
#[doc(hidden)]
pub struct AtomicPropertyList {
    pub objects: Vec<u32>,
    pub count_props: Vec<u32>,
    pub props: Vec<u32>,
    pub values: Vec<u64>,
}

impl AtomicPropertyList {
    #[doc(hidden)]
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.objects.len() > MAX_ATOMIC_OBJECTS {
            return Err(ProtocolError::Field("object count limit"));
        }
        if self.count_props.len() != self.objects.len() {
            return Err(ProtocolError::Field("count_props length"));
        }
        // Summed in u64 so no u32 overflow can make an oversized list look
        // small enough to pass the cap.
        let sum: u64 = self.count_props.iter().map(|c| u64::from(*c)).sum();
        if sum > MAX_ATOMIC_PROPS as u64 {
            return Err(ProtocolError::Field("prop count limit"));
        }
        if sum != self.props.len() as u64 {
            return Err(ProtocolError::Field("prop count sum"));
        }
        if self.values.len() != self.props.len() {
            return Err(ProtocolError::Field("value count"));
        }
        Ok(())
    }
}

/// One CRTC's `OUT_FENCE_PTR` slot: which value index the helper patches with
/// the address of that CRTC's holder.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub struct OutFenceSlot {
    pub crtc_id: u32,
    pub value_index: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[doc(hidden)]
pub struct AtomicRequest {
    pub correlation: HostCallCorrelation,
    pub class: HostCallClass,
    pub flags: u32,
    pub properties: AtomicPropertyList,
    pub out_fence_slots: Vec<OutFenceSlot>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub struct ClockProbeRequest {
    pub correlation: HostCallCorrelation,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostCallRequest {
    Atomic(AtomicRequest),
    ClockProbe(ClockProbeRequest),
}

impl HostCallRequest {
    #[doc(hidden)]
    pub fn correlation(&self) -> HostCallCorrelation {
        match self {
            Self::Atomic(req) => req.correlation,
            Self::ClockProbe(req) => req.correlation,
        }
    }

    /// Total and bare, which is what lets `send` pick a watchdog without an
    /// `Option`. A clock probe is seat-active work: the spec puts both
    /// message classes under the same watchdog rules, and a probe never
    /// blocks, so it takes the two-second class.
    #[doc(hidden)]
    pub fn class(&self) -> HostCallClass {
        match self {
            Self::Atomic(req) => req.class,
            Self::ClockProbe(_) => HostCallClass::SeatActiveNonblock,
        }
    }

    #[doc(hidden)]
    pub fn kind(&self) -> RequestKind {
        match self {
            Self::Atomic(_) => RequestKind::Atomic,
            Self::ClockProbe(_) => RequestKind::ClockProbe,
        }
    }

    #[allow(dead_code)] // Consumed by the owner in 2b.
    #[doc(hidden)]
    pub fn seq(&self) -> RequestSeq {
        self.correlation().seq()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostCallReply {
    Accepted {
        correlation: HostCallCorrelation,
        helper_duration_ns: u64,
        out_fence_mask: u32,
    },
    Rejected {
        correlation: HostCallCorrelation,
        errno: i32,
        helper_duration_ns: u64,
        unexpected_fence_output: bool,
    },
    ProbeAccepted {
        correlation: HostCallCorrelation,
        sequence: u64,
        helper_duration_ns: u64,
    },
    /// A probe's most important negative result. Without a probe-family
    /// rejection an `EOPNOTSUPP` could not be represented: the family check
    /// would classify every rejected probe as malformed, and 2b decides a
    /// CRTC is structurally incapable from exactly this errno.
    ProbeRejected {
        correlation: HostCallCorrelation,
        errno: i32,
        helper_duration_ns: u64,
    },
}

impl HostCallReply {
    #[doc(hidden)]
    pub const fn correlation(&self) -> HostCallCorrelation {
        match self {
            Self::Accepted { correlation, .. }
            | Self::Rejected { correlation, .. }
            | Self::ProbeAccepted { correlation, .. }
            | Self::ProbeRejected { correlation, .. } => *correlation,
        }
    }

    /// Derived from the *variant*, never from the correlation. The
    /// correlation is precisely what matches in the attack this guards
    /// against, so it cannot also be the discriminator.
    #[doc(hidden)]
    pub const fn family(&self) -> RequestKind {
        match self {
            Self::Accepted { .. } | Self::Rejected { .. } => RequestKind::Atomic,
            Self::ProbeAccepted { .. } | Self::ProbeRejected { .. } => RequestKind::ClockProbe,
        }
    }
}

/// Startup handshake. Its own frame family so `HostCallRequest`'s accessors
/// stay total, but it carries the epoch like everything else on this wire:
/// `spec:416-425` is unqualified, and both identities exist before the spawn.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub struct HandshakeRequest {
    pub incarnation: IncarnationId,
    pub lifecycle_epoch: LifecycleEpochId,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub struct HandshakeReply {
    pub incarnation: IncarnationId,
    pub lifecycle_epoch: LifecycleEpochId,
    pub helper_pid: u32,
}

fn encode_header(frame: &mut [u8], kind: u16, payload_len: usize) {
    frame[..4].copy_from_slice(&PROTOCOL_MAGIC);
    frame[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    frame[6..8].copy_from_slice(&kind.to_le_bytes());
    frame[8..12].copy_from_slice(&(payload_len as u32).to_le_bytes());
}

/// Validates magic, version and payload length against the frame's own size,
/// and returns the kind. Kind admissibility is the caller's decision, so each
/// family can reject the other's frames with `ProtocolError::Kind`.
fn decode_header(frame: &[u8]) -> Result<u16, ProtocolError> {
    if frame.len() < HEADER_LEN || frame.len() > MAX_REQUEST_FRAME_LEN {
        return Err(ProtocolError::Length);
    }
    if frame[..4] != PROTOCOL_MAGIC {
        return Err(ProtocolError::Magic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::Version(version));
    }
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;
    if payload_len != frame.len() - HEADER_LEN {
        return Err(ProtocolError::Length);
    }
    Ok(u16::from_le_bytes([frame[6], frame[7]]))
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

fn nonzero(raw: u64, field: &'static str) -> Result<u64, ProtocolError> {
    if raw == 0 {
        return Err(ProtocolError::Field(field));
    }
    Ok(raw)
}

fn put_correlation(frame: &mut [u8], cursor: &mut usize, correlation: HostCallCorrelation) {
    match correlation {
        HostCallCorrelation::Atomic {
            seq,
            incarnation,
            lifecycle_epoch,
            transition,
            commit,
            event_token,
        } => {
            put_u8(frame, cursor, CORRELATION_ATOMIC);
            put_u8(frame, cursor, u8::from(transition.is_some()));
            put_u16(frame, cursor, 0);
            put_u32(frame, cursor, 0);
            put_u64(frame, cursor, seq.get());
            put_u64(frame, cursor, incarnation.get());
            put_u64(frame, cursor, lifecycle_epoch.get());
            put_u64(frame, cursor, transition.map_or(0, |t| t.get()));
            put_u64(frame, cursor, commit.get());
            put_u64(frame, cursor, event_token.as_user_data());
        }
        HostCallCorrelation::ClockProbe {
            seq,
            incarnation,
            lifecycle_epoch,
            topology_generation,
            hardware_crtc,
            clock_epoch,
            probe,
        } => {
            put_u8(frame, cursor, CORRELATION_PROBE);
            put_u8(frame, cursor, 0);
            put_u16(frame, cursor, 0);
            put_u32(frame, cursor, hardware_crtc);
            put_u64(frame, cursor, seq.get());
            put_u64(frame, cursor, incarnation.get());
            put_u64(frame, cursor, lifecycle_epoch.get());
            put_u64(frame, cursor, topology_generation);
            put_u64(frame, cursor, clock_epoch.get());
            put_u64(frame, cursor, probe.get());
        }
    }
}

fn take_correlation(
    frame: &[u8],
    cursor: &mut usize,
) -> Result<HostCallCorrelation, ProtocolError> {
    let tag = take_u8(frame, cursor)?;
    let transition_present = take_u8(frame, cursor)?;
    let _pad = take_u16(frame, cursor)?;
    let hardware_crtc = take_u32(frame, cursor)?;
    let seq = RequestSeq::from_raw(take_u64(frame, cursor)?);
    let incarnation = IncarnationId::from_raw(nonzero(take_u64(frame, cursor)?, "incarnation")?);
    let lifecycle_epoch =
        LifecycleEpochId::from_raw(nonzero(take_u64(frame, cursor)?, "lifecycle epoch")?);
    let fourth = take_u64(frame, cursor)?;
    let fifth = take_u64(frame, cursor)?;
    let sixth = take_u64(frame, cursor)?;
    match tag {
        CORRELATION_ATOMIC => {
            let transition = match transition_present {
                0 => None,
                1 => Some(LifecycleTransitionId::from_raw(nonzero(
                    fourth,
                    "transition",
                )?)),
                _ => return Err(ProtocolError::Field("transition present")),
            };
            Ok(HostCallCorrelation::Atomic {
                seq,
                incarnation,
                lifecycle_epoch,
                transition,
                commit: CommitId::from_raw(nonzero(fifth, "commit")?),
                event_token: EventToken::from_user_data(sixth)
                    .ok_or(ProtocolError::Field("event token"))?,
            })
        }
        CORRELATION_PROBE => Ok(HostCallCorrelation::ClockProbe {
            seq,
            incarnation,
            lifecycle_epoch,
            topology_generation: fourth,
            hardware_crtc,
            clock_epoch: ClockEpochId::from_raw(nonzero(fifth, "clock epoch")?),
            probe: ClockProbeId::from_raw(nonzero(sixth, "probe")?),
        }),
        _ => Err(ProtocolError::Field("correlation tag")),
    }
}

/// Every rule that makes a request well-formed, in one place so the encoder
/// and the decoder cannot drift apart. Written against `is_validation()`
/// rather than named variants, so adding a class cannot leave a hole.
fn check_atomic_invariants(request: &AtomicRequest) -> Result<(), ProtocolError> {
    request.properties.validate()?;

    let nonblock = request.flags & DRM_MODE_ATOMIC_NONBLOCK != 0;
    let test_only = request.flags & DRM_MODE_ATOMIC_TEST_ONLY != 0;
    let agrees = match request.class {
        HostCallClass::SeatActiveNonblock => nonblock && !test_only,
        HostCallClass::SeatActiveValidation | HostCallClass::ColdStartOrOfflineValidation => {
            test_only && !nonblock
        }
        HostCallClass::ColdStartOrOfflineBlocking => !nonblock && !test_only,
    };
    if !agrees {
        return Err(ProtocolError::Field("class flag agreement"));
    }

    if request.class.is_validation() && !request.out_fence_slots.is_empty() {
        return Err(ProtocolError::Field("validation out fence slot"));
    }
    if request.out_fence_slots.len() > MAX_OUT_FENCES {
        return Err(ProtocolError::Field("slot count limit"));
    }
    for (i, slot) in request.out_fence_slots.iter().enumerate() {
        if slot.value_index as usize >= request.properties.values.len() {
            return Err(ProtocolError::Field("out fence slot index"));
        }
        for earlier in &request.out_fence_slots[..i] {
            if earlier.value_index == slot.value_index {
                return Err(ProtocolError::Field("duplicate out fence slot index"));
            }
            if earlier.crtc_id == slot.crtc_id {
                return Err(ProtocolError::Field("duplicate out fence slot crtc"));
            }
        }
    }
    Ok(())
}

fn atomic_body_len(request: &AtomicRequest) -> usize {
    let objects = request.properties.objects.len();
    let props = request.properties.props.len();
    2 * 4 * objects + 4 * props + 8 * props + 8 * request.out_fence_slots.len()
}

/// Panics on a malformed or mislabelled request: the owner must never put one
/// on the wire, and a panic in the parent beats handing a short array or a
/// mis-classed commit to a helper that passes it to the kernel.
#[doc(hidden)]
pub fn encode_request(request: &HostCallRequest) -> Vec<u8> {
    match request {
        HostCallRequest::Atomic(req) => {
            check_atomic_invariants(req).expect("owner built an invalid atomic request");
            encode_atomic_request(req)
        }
        HostCallRequest::ClockProbe(req) => encode_probe_request(req),
    }
}

/// Skips the invariant checks so the decoder can be exercised against frames
/// a correct encoder never emits.
#[cfg(test)]
pub(crate) fn encode_request_unchecked_for_tests(request: &HostCallRequest) -> Vec<u8> {
    match request {
        HostCallRequest::Atomic(req) => encode_atomic_request(req),
        HostCallRequest::ClockProbe(req) => encode_probe_request(req),
    }
}

fn encode_atomic_request(req: &AtomicRequest) -> Vec<u8> {
    let payload_len = ATOMIC_HEAD_LEN + atomic_body_len(req);
    let mut frame = vec![0u8; HEADER_LEN + payload_len];
    encode_header(&mut frame, KIND_ATOMIC_REQUEST, payload_len);
    let mut cursor = HEADER_LEN;
    let HostCallCorrelation::Atomic {
        seq,
        incarnation,
        lifecycle_epoch,
        transition,
        commit,
        event_token,
    } = req.correlation
    else {
        panic!("an atomic request carries an atomic correlation")
    };
    put_u64(&mut frame, &mut cursor, seq.get());
    put_u64(&mut frame, &mut cursor, incarnation.get());
    put_u64(&mut frame, &mut cursor, lifecycle_epoch.get());
    put_u64(&mut frame, &mut cursor, transition.map_or(0, |t| t.get()));
    put_u64(&mut frame, &mut cursor, commit.get());
    put_u64(&mut frame, &mut cursor, event_token.as_user_data());
    put_u8(&mut frame, &mut cursor, u8::from(transition.is_some()));
    put_u8(&mut frame, &mut cursor, req.class.wire_tag());
    put_u16(&mut frame, &mut cursor, 0);
    put_u32(&mut frame, &mut cursor, req.flags);
    put_u32(&mut frame, &mut cursor, req.properties.objects.len() as u32);
    put_u32(&mut frame, &mut cursor, req.properties.props.len() as u32);
    put_u32(&mut frame, &mut cursor, req.out_fence_slots.len() as u32);
    for object in &req.properties.objects {
        put_u32(&mut frame, &mut cursor, *object);
    }
    for count in &req.properties.count_props {
        put_u32(&mut frame, &mut cursor, *count);
    }
    for prop in &req.properties.props {
        put_u32(&mut frame, &mut cursor, *prop);
    }
    for value in &req.properties.values {
        put_u64(&mut frame, &mut cursor, *value);
    }
    for slot in &req.out_fence_slots {
        put_u32(&mut frame, &mut cursor, slot.crtc_id);
        put_u32(&mut frame, &mut cursor, slot.value_index);
    }
    frame
}

fn encode_probe_request(req: &ClockProbeRequest) -> Vec<u8> {
    let mut frame = vec![0u8; HEADER_LEN + PROBE_HEAD_LEN];
    encode_header(&mut frame, KIND_CLOCK_PROBE_REQUEST, PROBE_HEAD_LEN);
    let mut cursor = HEADER_LEN;
    let HostCallCorrelation::ClockProbe {
        seq,
        incarnation,
        lifecycle_epoch,
        topology_generation,
        hardware_crtc,
        clock_epoch,
        probe,
    } = req.correlation
    else {
        panic!("a clock probe carries a probe correlation")
    };
    put_u64(&mut frame, &mut cursor, seq.get());
    put_u64(&mut frame, &mut cursor, incarnation.get());
    put_u64(&mut frame, &mut cursor, lifecycle_epoch.get());
    put_u64(&mut frame, &mut cursor, topology_generation);
    put_u64(&mut frame, &mut cursor, clock_epoch.get());
    put_u64(&mut frame, &mut cursor, probe.get());
    put_u32(&mut frame, &mut cursor, hardware_crtc);
    put_u32(&mut frame, &mut cursor, 0);
    frame
}

#[doc(hidden)]
pub fn decode_request(frame: &[u8]) -> Result<HostCallRequest, ProtocolError> {
    match decode_header(frame)? {
        KIND_ATOMIC_REQUEST => decode_atomic_request(frame).map(HostCallRequest::Atomic),
        KIND_CLOCK_PROBE_REQUEST => decode_probe_request(frame).map(HostCallRequest::ClockProbe),
        other => Err(ProtocolError::Kind(other)),
    }
}

fn decode_atomic_request(frame: &[u8]) -> Result<AtomicRequest, ProtocolError> {
    if frame.len() < HEADER_LEN + ATOMIC_HEAD_LEN {
        return Err(ProtocolError::Length);
    }
    let mut cursor = HEADER_LEN;
    let seq = RequestSeq::from_raw(take_u64(frame, &mut cursor)?);
    let incarnation =
        IncarnationId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "incarnation")?);
    let lifecycle_epoch =
        LifecycleEpochId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "lifecycle epoch")?);
    let transition_raw = take_u64(frame, &mut cursor)?;
    let commit = CommitId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "commit")?);
    let event_token = EventToken::from_user_data(take_u64(frame, &mut cursor)?)
        .ok_or(ProtocolError::Field("event token"))?;
    let transition = match take_u8(frame, &mut cursor)? {
        0 => None,
        1 => Some(LifecycleTransitionId::from_raw(nonzero(
            transition_raw,
            "transition",
        )?)),
        _ => return Err(ProtocolError::Field("transition present")),
    };
    let class = HostCallClass::from_wire_tag(take_u8(frame, &mut cursor)?)
        .ok_or(ProtocolError::Field("class tag"))?;
    let _pad = take_u16(frame, &mut cursor)?;
    let flags = take_u32(frame, &mut cursor)?;

    // The counts are the only wire values that size an allocation, so they
    // are bounded before anything is allocated and before the body length is
    // computed from them.
    let object_count = take_u32(frame, &mut cursor)? as usize;
    let prop_count = take_u32(frame, &mut cursor)? as usize;
    let slot_count = take_u32(frame, &mut cursor)? as usize;
    if object_count > MAX_ATOMIC_OBJECTS {
        return Err(ProtocolError::Field("object count limit"));
    }
    if prop_count > MAX_ATOMIC_PROPS {
        return Err(ProtocolError::Field("prop count limit"));
    }
    if slot_count > MAX_OUT_FENCES {
        return Err(ProtocolError::Field("slot count limit"));
    }

    // Checked throughout so a future cap increase cannot silently wrap this.
    let body_len = 2usize
        .checked_mul(4)
        .and_then(|n| n.checked_mul(object_count))
        .and_then(|n| n.checked_add(prop_count.checked_mul(4)?))
        .and_then(|n| n.checked_add(prop_count.checked_mul(8)?))
        .and_then(|n| n.checked_add(slot_count.checked_mul(8)?))
        .ok_or(ProtocolError::Length)?;
    if frame.len() != HEADER_LEN + ATOMIC_HEAD_LEN + body_len {
        return Err(ProtocolError::Length);
    }

    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        objects.push(take_u32(frame, &mut cursor)?);
    }
    let mut count_props = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        count_props.push(take_u32(frame, &mut cursor)?);
    }
    let mut props = Vec::with_capacity(prop_count);
    for _ in 0..prop_count {
        props.push(take_u32(frame, &mut cursor)?);
    }
    let mut values = Vec::with_capacity(prop_count);
    for _ in 0..prop_count {
        values.push(take_u64(frame, &mut cursor)?);
    }
    let mut out_fence_slots = Vec::with_capacity(slot_count);
    for _ in 0..slot_count {
        let crtc_id = take_u32(frame, &mut cursor)?;
        let value_index = take_u32(frame, &mut cursor)?;
        out_fence_slots.push(OutFenceSlot {
            crtc_id,
            value_index,
        });
    }

    let request = AtomicRequest {
        correlation: HostCallCorrelation::Atomic {
            seq,
            incarnation,
            lifecycle_epoch,
            transition,
            commit,
            event_token,
        },
        class,
        flags,
        properties: AtomicPropertyList {
            objects,
            count_props,
            props,
            values,
        },
        out_fence_slots,
    };
    check_atomic_invariants(&request)?;
    Ok(request)
}

fn decode_probe_request(frame: &[u8]) -> Result<ClockProbeRequest, ProtocolError> {
    if frame.len() != HEADER_LEN + PROBE_HEAD_LEN {
        return Err(ProtocolError::Length);
    }
    let mut cursor = HEADER_LEN;
    let seq = RequestSeq::from_raw(take_u64(frame, &mut cursor)?);
    let incarnation =
        IncarnationId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "incarnation")?);
    let lifecycle_epoch =
        LifecycleEpochId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "lifecycle epoch")?);
    let topology_generation = take_u64(frame, &mut cursor)?;
    let clock_epoch =
        ClockEpochId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "clock epoch")?);
    let probe = ClockProbeId::from_raw(nonzero(take_u64(frame, &mut cursor)?, "probe")?);
    let hardware_crtc = take_u32(frame, &mut cursor)?;
    let _pad = take_u32(frame, &mut cursor)?;
    Ok(ClockProbeRequest {
        correlation: HostCallCorrelation::ClockProbe {
            seq,
            incarnation,
            lifecycle_epoch,
            topology_generation,
            hardware_crtc,
            clock_epoch,
            probe,
        },
    })
}

#[doc(hidden)]
pub fn encode_reply(reply: &HostCallReply) -> [u8; REPLY_FRAME_LEN] {
    let mut frame = [0u8; REPLY_FRAME_LEN];
    encode_header(&mut frame, KIND_REPLY, REPLY_PAYLOAD_LEN);
    let mut cursor = HEADER_LEN;
    let tag = match reply {
        HostCallReply::Accepted { .. } => REPLY_TAG_ACCEPTED,
        HostCallReply::Rejected { .. } => REPLY_TAG_REJECTED,
        HostCallReply::ProbeAccepted { .. } => REPLY_TAG_PROBE_ACCEPTED,
        HostCallReply::ProbeRejected { .. } => REPLY_TAG_PROBE_REJECTED,
    };
    put_u8(&mut frame, &mut cursor, tag);
    put_u8(&mut frame, &mut cursor, 0);
    put_u16(&mut frame, &mut cursor, 0);
    put_correlation(&mut frame, &mut cursor, reply.correlation());
    match *reply {
        HostCallReply::Accepted {
            helper_duration_ns,
            out_fence_mask,
            ..
        } => {
            put_u64(&mut frame, &mut cursor, helper_duration_ns);
            put_u32(&mut frame, &mut cursor, out_fence_mask);
            put_u32(&mut frame, &mut cursor, 0);
        }
        HostCallReply::Rejected {
            errno,
            helper_duration_ns,
            unexpected_fence_output,
            ..
        } => {
            put_u64(&mut frame, &mut cursor, helper_duration_ns);
            put_i32(&mut frame, &mut cursor, errno);
            put_u8(&mut frame, &mut cursor, u8::from(unexpected_fence_output));
            put_u8(&mut frame, &mut cursor, 0);
            put_u16(&mut frame, &mut cursor, 0);
        }
        HostCallReply::ProbeAccepted {
            sequence,
            helper_duration_ns,
            ..
        } => {
            put_u64(&mut frame, &mut cursor, helper_duration_ns);
            put_u64(&mut frame, &mut cursor, sequence);
        }
        HostCallReply::ProbeRejected {
            errno,
            helper_duration_ns,
            ..
        } => {
            put_u64(&mut frame, &mut cursor, helper_duration_ns);
            put_i32(&mut frame, &mut cursor, errno);
            put_u32(&mut frame, &mut cursor, 0);
        }
    }
    frame
}

#[doc(hidden)]
pub fn decode_reply(frame: &[u8]) -> Result<HostCallReply, ProtocolError> {
    let kind = decode_header(frame)?;
    if kind != KIND_REPLY {
        return Err(ProtocolError::Kind(kind));
    }
    if frame.len() != REPLY_FRAME_LEN {
        return Err(ProtocolError::Length);
    }
    let mut cursor = HEADER_LEN;
    let tag = take_u8(frame, &mut cursor)?;
    let _pad = take_u8(frame, &mut cursor)?;
    let _pad = take_u16(frame, &mut cursor)?;
    let correlation = take_correlation(frame, &mut cursor)?;
    let helper_duration_ns = take_u64(frame, &mut cursor)?;
    match tag {
        REPLY_TAG_ACCEPTED => {
            let out_fence_mask = take_u32(frame, &mut cursor)?;
            Ok(HostCallReply::Accepted {
                correlation,
                helper_duration_ns,
                out_fence_mask,
            })
        }
        REPLY_TAG_REJECTED => {
            let errno = take_i32(frame, &mut cursor)?;
            let unexpected = match take_u8(frame, &mut cursor)? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::Field("unexpected fence output")),
            };
            Ok(HostCallReply::Rejected {
                correlation,
                errno,
                helper_duration_ns,
                unexpected_fence_output: unexpected,
            })
        }
        REPLY_TAG_PROBE_ACCEPTED => Ok(HostCallReply::ProbeAccepted {
            correlation,
            sequence: take_u64(frame, &mut cursor)?,
            helper_duration_ns,
        }),
        REPLY_TAG_PROBE_REJECTED => Ok(HostCallReply::ProbeRejected {
            correlation,
            errno: take_i32(frame, &mut cursor)?,
            helper_duration_ns,
        }),
        _ => Err(ProtocolError::Field("reply tag")),
    }
}

#[allow(dead_code)] // Consumed by the handshake in task 6.
#[doc(hidden)]
pub fn encode_handshake_request(request: &HandshakeRequest) -> Vec<u8> {
    let mut frame = vec![0u8; HEADER_LEN + HANDSHAKE_REQUEST_PAYLOAD_LEN];
    encode_header(
        &mut frame,
        KIND_HANDSHAKE_REQUEST,
        HANDSHAKE_REQUEST_PAYLOAD_LEN,
    );
    let mut cursor = HEADER_LEN;
    put_u64(&mut frame, &mut cursor, request.incarnation.get());
    put_u64(&mut frame, &mut cursor, request.lifecycle_epoch.get());
    frame
}

#[allow(dead_code)] // Consumed by the handshake in task 6.
#[doc(hidden)]
pub fn decode_handshake_request(frame: &[u8]) -> Result<HandshakeRequest, ProtocolError> {
    let kind = decode_header(frame)?;
    if kind != KIND_HANDSHAKE_REQUEST {
        return Err(ProtocolError::Kind(kind));
    }
    let mut cursor = HEADER_LEN;
    Ok(HandshakeRequest {
        incarnation: IncarnationId::from_raw(nonzero(
            take_u64(frame, &mut cursor)?,
            "incarnation",
        )?),
        lifecycle_epoch: LifecycleEpochId::from_raw(nonzero(
            take_u64(frame, &mut cursor)?,
            "lifecycle epoch",
        )?),
    })
}

#[allow(dead_code)] // Consumed by the handshake in task 6.
#[doc(hidden)]
pub fn encode_handshake_reply(reply: &HandshakeReply) -> Vec<u8> {
    let mut frame = vec![0u8; HEADER_LEN + HANDSHAKE_REPLY_PAYLOAD_LEN];
    encode_header(
        &mut frame,
        KIND_HANDSHAKE_REPLY,
        HANDSHAKE_REPLY_PAYLOAD_LEN,
    );
    let mut cursor = HEADER_LEN;
    put_u64(&mut frame, &mut cursor, reply.incarnation.get());
    put_u64(&mut frame, &mut cursor, reply.lifecycle_epoch.get());
    put_u32(&mut frame, &mut cursor, reply.helper_pid);
    frame
}

#[allow(dead_code)] // Consumed by the handshake in task 6.
#[doc(hidden)]
pub fn decode_handshake_reply(frame: &[u8]) -> Result<HandshakeReply, ProtocolError> {
    let kind = decode_header(frame)?;
    if kind != KIND_HANDSHAKE_REPLY {
        return Err(ProtocolError::Kind(kind));
    }
    let mut cursor = HEADER_LEN;
    Ok(HandshakeReply {
        incarnation: IncarnationId::from_raw(nonzero(
            take_u64(frame, &mut cursor)?,
            "incarnation",
        )?),
        lifecycle_epoch: LifecycleEpochId::from_raw(nonzero(
            take_u64(frame, &mut cursor)?,
            "lifecycle epoch",
        )?),
        helper_pid: take_u32(frame, &mut cursor)?,
    })
}

#[cfg(test)]
pub(crate) fn golden_atomic_correlation_for_tests() -> HostCallCorrelation {
    HostCallCorrelation::Atomic {
        seq: RequestSeq::from_raw(0x11),
        incarnation: IncarnationId::from_raw(0x22),
        lifecycle_epoch: LifecycleEpochId::from_raw(0x33),
        transition: Some(LifecycleTransitionId::from_raw(0x44)),
        commit: CommitId::from_raw(0x55),
        event_token: EventToken::tagged_for_tests(0x66),
    }
}

#[cfg(test)]
pub(crate) fn golden_probe_correlation_for_tests() -> HostCallCorrelation {
    HostCallCorrelation::ClockProbe {
        seq: RequestSeq::from_raw(0x11),
        incarnation: IncarnationId::from_raw(0x22),
        lifecycle_epoch: LifecycleEpochId::from_raw(0x33),
        topology_generation: 0x44,
        hardware_crtc: 0x55,
        clock_epoch: ClockEpochId::from_raw(0x66),
        probe: ClockProbeId::from_raw(0x77),
    }
}

#[cfg(test)]
pub(crate) fn golden_atomic_request_for_tests() -> AtomicRequest {
    AtomicRequest {
        correlation: golden_atomic_correlation_for_tests(),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: vec![0x0A11, 0x0A22],
            count_props: vec![1, 2],
            props: vec![0x0B11, 0x0B22, 0x0B33],
            values: vec![0x0C11, 0x0C22, 0x0C33],
        },
        out_fence_slots: vec![OutFenceSlot {
            crtc_id: 0x0A11,
            value_index: 0,
        }],
    }
}

#[cfg(test)]
pub(crate) fn golden_probe_request_for_tests() -> ClockProbeRequest {
    ClockProbeRequest {
        correlation: golden_probe_correlation_for_tests(),
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    fn u64_at(f: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(f[o..o + 8].try_into().unwrap())
    }
    fn u32_at(f: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(f[o..o + 4].try_into().unwrap())
    }

    #[test]
    fn the_declared_head_lengths_equal_the_sums_of_their_fields() {
        // An encoder and decoder sharing one wrong offset pass a round-trip
        // test, so assert the constants independently of any round trip.
        assert_eq!(ATOMIC_HEAD_LEN, 6 * 8 + 1 + 1 + 2 + 4 * 4);
        assert_eq!(ATOMIC_HEAD_LEN, 68);
        assert_eq!(PROBE_HEAD_LEN, 6 * 8 + 4 + 4);
        assert_eq!(PROBE_HEAD_LEN, 56);
        assert_eq!(HEADER_LEN + ATOMIC_HEAD_LEN, 80);
    }

    /// Every atomic head field at its documented offset, each with a distinct
    /// value so transposing any two cannot pass.
    #[test]
    fn a_golden_atomic_frame_places_every_field_at_its_documented_offset() {
        let f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));

        assert_eq!(&f[0..4], &PROTOCOL_MAGIC);
        assert_eq!(
            u16::from_le_bytes(f[4..6].try_into().unwrap()),
            2,
            "version"
        );
        assert_eq!(
            u16::from_le_bytes(f[6..8].try_into().unwrap()),
            KIND_ATOMIC_REQUEST
        );
        assert_eq!(u32_at(&f, 8) as usize, f.len() - HEADER_LEN, "payload_len");

        assert_eq!(u64_at(&f, 12), 0x11, "seq @0");
        assert_eq!(u64_at(&f, 20), 0x22, "incarnation @8");
        assert_eq!(u64_at(&f, 28), 0x33, "lifecycle_epoch @16");
        assert_eq!(u64_at(&f, 36), 0x44, "transition @24");
        assert_eq!(u64_at(&f, 44), 0x55, "commit @32");
        assert_eq!(
            u64_at(&f, 52),
            EventToken::tagged_for_tests(0x66).as_user_data(),
            "event_token @40"
        );
        assert_eq!(f[60], 1, "transition_present @48");
        assert_eq!(
            f[61],
            HostCallClass::SeatActiveNonblock.wire_tag(),
            "class @49"
        );
        assert_eq!(
            u16::from_le_bytes(f[62..64].try_into().unwrap()),
            0,
            "pad @50"
        );
        assert_eq!(u32_at(&f, 64), DRM_MODE_ATOMIC_NONBLOCK, "flags @52");
        assert_eq!(u32_at(&f, 68), 2, "object_count @56");
        assert_eq!(u32_at(&f, 72), 3, "prop_count @60");
        assert_eq!(u32_at(&f, 76), 1, "slot_count @64");

        assert_eq!(u32_at(&f, 80), 0x0A11);
        assert_eq!(u32_at(&f, 84), 0x0A22);
        assert_eq!(u32_at(&f, 88), 1);
        assert_eq!(u32_at(&f, 92), 2);
        assert_eq!(u32_at(&f, 96), 0x0B11);
        assert_eq!(u32_at(&f, 100), 0x0B22);
        assert_eq!(u32_at(&f, 104), 0x0B33);
        assert_eq!(u64_at(&f, 108), 0x0C11);
        assert_eq!(u64_at(&f, 116), 0x0C22);
        assert_eq!(u64_at(&f, 124), 0x0C33);
        assert_eq!(u32_at(&f, 132), 0x0A11, "slot crtc_id");
        assert_eq!(u32_at(&f, 136), 0, "slot value_index");
        assert_eq!(f.len(), 140);

        let HostCallRequest::Atomic(back) = decode_request(&f).expect("decode") else {
            panic!("kind changed across the wire")
        };
        assert_eq!(back, golden_atomic_request_for_tests());
    }

    #[test]
    fn an_absent_transition_writes_a_zero_flag_and_a_zero_field() {
        let mut request = golden_atomic_request_for_tests();
        let HostCallCorrelation::Atomic {
            ref mut transition, ..
        } = request.correlation
        else {
            unreachable!("golden request is atomic")
        };
        *transition = None;
        let f = encode_request(&HostCallRequest::Atomic(request.clone()));
        assert_eq!(f[60], 0, "transition_present @48");
        assert_eq!(u64_at(&f, 36), 0, "transition @24");
        let HostCallRequest::Atomic(back) = decode_request(&f).expect("decode") else {
            panic!("kind changed across the wire")
        };
        assert_eq!(back.correlation, request.correlation);
    }

    #[test]
    fn a_golden_probe_frame_places_every_field_at_its_documented_offset() {
        // spec:641-645 requires the probe tuple to carry topology generation.
        // Stage 1 already did; dropping it here would be a regression, so it
        // is asserted at a fixed offset.
        let f = encode_request(&HostCallRequest::ClockProbe(
            golden_probe_request_for_tests(),
        ));
        assert_eq!(
            u16::from_le_bytes(f[6..8].try_into().unwrap()),
            KIND_CLOCK_PROBE_REQUEST
        );
        assert_eq!(u64_at(&f, 12), 0x11, "seq @0");
        assert_eq!(u64_at(&f, 20), 0x22, "incarnation @8");
        assert_eq!(u64_at(&f, 28), 0x33, "lifecycle_epoch @16");
        assert_eq!(u64_at(&f, 36), 0x44, "topology_generation @24");
        assert_eq!(u64_at(&f, 44), 0x66, "clock_epoch @32");
        assert_eq!(u64_at(&f, 52), 0x77, "probe @40");
        assert_eq!(u32_at(&f, 60), 0x55, "hardware_crtc @48");
        assert_eq!(u32_at(&f, 64), 0, "pad @52");
        assert_eq!(f.len(), HEADER_LEN + PROBE_HEAD_LEN);
        assert_eq!(
            decode_request(&f).expect("decode"),
            HostCallRequest::ClockProbe(golden_probe_request_for_tests())
        );
    }

    #[test]
    fn every_reply_echoes_the_request_correlation() {
        // ID-3: a reply is current only when incarnation, lifecycle epoch,
        // optional transition id and commit id all match. A per-socket
        // sequence number cannot classify a late success against a changed
        // lifecycle.
        let correlation = golden_atomic_correlation_for_tests();
        for reply in [
            HostCallReply::Accepted {
                correlation,
                helper_duration_ns: 10,
                out_fence_mask: 0b11,
            },
            HostCallReply::Rejected {
                correlation,
                errno: libc::EINVAL,
                helper_duration_ns: 10,
                unexpected_fence_output: true,
            },
        ] {
            assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
            assert_eq!(reply.correlation(), correlation);
            assert_eq!(reply.family(), RequestKind::Atomic);
        }
    }

    #[test]
    fn both_probe_replies_echo_the_probe_correlation_and_belong_to_its_family() {
        let correlation = golden_probe_correlation_for_tests();
        for reply in [
            HostCallReply::ProbeAccepted {
                correlation,
                sequence: 42,
                helper_duration_ns: 10,
            },
            HostCallReply::ProbeRejected {
                correlation,
                errno: libc::EOPNOTSUPP,
                helper_duration_ns: 10,
            },
        ] {
            assert_eq!(decode_reply(&encode_reply(&reply)).expect("decode"), reply);
            assert_eq!(reply.family(), RequestKind::ClockProbe);
        }
    }

    /// Without ProbeRejected an EOPNOTSUPP probe result is unrepresentable:
    /// the family check would classify every rejected probe as malformed, and
    /// 2b decides a CRTC is structurally incapable from exactly this errno.
    #[test]
    fn a_rejected_probe_is_representable_and_matches_its_request_family() {
        let request = HostCallRequest::ClockProbe(golden_probe_request_for_tests());
        let reply = HostCallReply::ProbeRejected {
            correlation: request.correlation(),
            errno: libc::EOPNOTSUPP,
            helper_duration_ns: 1,
        };
        assert_eq!(reply.family(), request.kind());
        assert_eq!(reply.correlation(), request.correlation());
    }

    #[test]
    fn a_reply_of_the_wrong_family_is_detectable_even_when_its_correlation_matches() {
        // Correlation equality alone would let a probe request accept an
        // atomic Accepted carrying the probe's own tuple. Deriving the family
        // from the correlation would not help: the correlation is exactly
        // what matches. It has to come from the variant.
        let request = HostCallRequest::ClockProbe(golden_probe_request_for_tests());
        let impostor = HostCallReply::Accepted {
            correlation: request.correlation(),
            helper_duration_ns: 1,
            out_fence_mask: 0,
        };
        assert_eq!(
            impostor.correlation(),
            request.correlation(),
            "genuinely matches"
        );
        assert_ne!(impostor.family(), request.kind(), "but the family does not");
    }

    #[test]
    fn a_reply_whose_lifecycle_epoch_differs_is_not_equal_to_the_sent_tuple() {
        let sent = golden_atomic_correlation_for_tests();
        let HostCallCorrelation::Atomic {
            seq,
            incarnation,
            transition,
            commit,
            event_token,
            ..
        } = sent
        else {
            unreachable!("golden correlation is atomic")
        };
        let stale = HostCallCorrelation::Atomic {
            seq,
            incarnation,
            lifecycle_epoch: LifecycleEpochId::from_raw(9999),
            transition,
            commit,
            event_token,
        };
        assert_ne!(sent, stale);
        assert_eq!(
            sent.seq(),
            stale.seq(),
            "a matching seq must not imply a matching tuple"
        );
    }

    #[test]
    fn property_list_counts_must_agree() {
        for (list, expected) in [
            (
                AtomicPropertyList {
                    objects: vec![31, 42],
                    count_props: vec![2],
                    props: vec![7, 8, 9],
                    values: vec![1, 2, 3],
                },
                ProtocolError::Field("count_props length"),
            ),
            (
                AtomicPropertyList {
                    objects: vec![31],
                    count_props: vec![2],
                    props: vec![7, 8, 9],
                    values: vec![1, 2, 3],
                },
                ProtocolError::Field("prop count sum"),
            ),
            (
                AtomicPropertyList {
                    objects: vec![31],
                    count_props: vec![3],
                    props: vec![7, 8, 9],
                    values: vec![1, 2],
                },
                ProtocolError::Field("value count"),
            ),
        ] {
            assert_eq!(list.validate(), Err(expected));
        }
    }

    #[test]
    fn oversized_property_lists_are_rejected_before_the_wire() {
        let list = AtomicPropertyList {
            objects: vec![1],
            count_props: vec![(MAX_ATOMIC_PROPS + 1) as u32],
            props: vec![1; MAX_ATOMIC_PROPS + 1],
            values: vec![0; MAX_ATOMIC_PROPS + 1],
        };
        assert_eq!(
            list.validate(),
            Err(ProtocolError::Field("prop count limit"))
        );

        let too_many = AtomicPropertyList {
            objects: vec![1; MAX_ATOMIC_OBJECTS + 1],
            count_props: vec![0; MAX_ATOMIC_OBJECTS + 1],
            props: vec![],
            values: vec![],
        };
        assert_eq!(
            too_many.validate(),
            Err(ProtocolError::Field("object count limit"))
        );
    }

    /// A decoder that allocates from wire counts before checking them can be
    /// made to attempt a multi-gigabyte reservation by an eighty-byte frame.
    #[test]
    fn declared_counts_are_bounded_before_anything_is_allocated() {
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        for (offset, expected) in [
            (68, ProtocolError::Field("object count limit")),
            (72, ProtocolError::Field("prop count limit")),
            (76, ProtocolError::Field("slot count limit")),
        ] {
            let saved = f[offset..offset + 4].to_vec();
            f[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert_eq!(decode_request(&f), Err(expected));
            f[offset..offset + 4].copy_from_slice(&saved);
        }
    }

    #[test]
    fn declared_counts_must_match_the_frames_own_length() {
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        f[68..72].copy_from_slice(&1u32.to_le_bytes()); // object_count 2 -> 1
        assert_eq!(decode_request(&f), Err(ProtocolError::Length));
    }

    #[test]
    fn truncation_at_every_length_is_an_error_not_a_short_read() {
        let f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        for cut in 0..f.len() {
            assert!(
                decode_request(&f[..cut]).is_err(),
                "truncation at {cut} decoded"
            );
        }
    }

    #[test]
    fn an_out_fence_slot_index_must_be_inside_the_value_array() {
        let request = AtomicRequest {
            properties: AtomicPropertyList {
                objects: vec![42],
                count_props: vec![1],
                props: vec![9],
                values: vec![0],
            },
            out_fence_slots: vec![OutFenceSlot {
                crtc_id: 42,
                value_index: 1,
            }],
            ..golden_atomic_request_for_tests()
        };
        let f = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
        assert_eq!(
            decode_request(&f),
            Err(ProtocolError::Field("out fence slot index"))
        );
    }

    /// Two slots on one value index would make the helper patch two holder
    /// addresses into the same u64, so one CRTC silently loses its completion
    /// evidence. Two slots naming one CRTC breaks the same model.
    #[test]
    fn duplicate_slot_indices_and_duplicate_crtcs_are_rejected() {
        let props = AtomicPropertyList {
            objects: vec![42, 43],
            count_props: vec![1, 1],
            props: vec![9, 9],
            values: vec![0, 0],
        };
        for (slots, expected) in [
            (
                vec![
                    OutFenceSlot {
                        crtc_id: 42,
                        value_index: 0,
                    },
                    OutFenceSlot {
                        crtc_id: 43,
                        value_index: 0,
                    },
                ],
                ProtocolError::Field("duplicate out fence slot index"),
            ),
            (
                vec![
                    OutFenceSlot {
                        crtc_id: 42,
                        value_index: 0,
                    },
                    OutFenceSlot {
                        crtc_id: 42,
                        value_index: 1,
                    },
                ],
                ProtocolError::Field("duplicate out fence slot crtc"),
            ),
        ] {
            let request = AtomicRequest {
                properties: props.clone(),
                out_fence_slots: slots,
                ..golden_atomic_request_for_tests()
            };
            let f = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
            assert_eq!(decode_request(&f), Err(expected));
        }
    }

    /// COMMIT-5 and the ValidationOnly rule are enforced here, not by
    /// convention at the call site.
    #[test]
    fn a_frame_whose_flags_contradict_its_class_is_rejected() {
        for (class, flags, why) in [
            (
                HostCallClass::SeatActiveNonblock,
                0,
                "seat-active nonblock requires NONBLOCK",
            ),
            (
                HostCallClass::SeatActiveNonblock,
                DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
                "a live commit must not set TEST_ONLY",
            ),
            (
                HostCallClass::SeatActiveValidation,
                DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
                "validation omits NONBLOCK",
            ),
            (
                HostCallClass::SeatActiveValidation,
                0,
                "validation requires TEST_ONLY",
            ),
            (
                HostCallClass::ColdStartOrOfflineBlocking,
                DRM_MODE_ATOMIC_NONBLOCK,
                "a blocking call must not set NONBLOCK",
            ),
            (
                HostCallClass::ColdStartOrOfflineBlocking,
                DRM_MODE_ATOMIC_TEST_ONLY,
                "a blocking commit must not set TEST_ONLY",
            ),
            (
                HostCallClass::ColdStartOrOfflineValidation,
                DRM_MODE_ATOMIC_NONBLOCK | DRM_MODE_ATOMIC_TEST_ONLY,
                "validation omits NONBLOCK at either boundary",
            ),
            (
                HostCallClass::ColdStartOrOfflineValidation,
                0,
                "validation requires TEST_ONLY at either boundary",
            ),
        ] {
            let request = AtomicRequest {
                class,
                flags,
                ..golden_atomic_request_for_tests()
            };
            let f = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request));
            assert_eq!(
                decode_request(&f),
                Err(ProtocolError::Field("class flag agreement")),
                "{why}"
            );
        }
    }

    /// spec:320-325 — TEST_ONLY creates no out-fence. Written over BOTH
    /// validation classes: covering only the seat-active one left the
    /// cold/offline variant able to carry slots.
    #[test]
    fn neither_validation_class_may_request_out_fences() {
        for class in [
            HostCallClass::SeatActiveValidation,
            HostCallClass::ColdStartOrOfflineValidation,
        ] {
            let request = AtomicRequest {
                class,
                flags: DRM_MODE_ATOMIC_TEST_ONLY,
                properties: AtomicPropertyList {
                    objects: vec![42],
                    count_props: vec![1],
                    props: vec![9],
                    values: vec![0],
                },
                out_fence_slots: vec![OutFenceSlot {
                    crtc_id: 42,
                    value_index: 0,
                }],
                ..golden_atomic_request_for_tests()
            };
            let f = encode_request_unchecked_for_tests(&HostCallRequest::Atomic(request.clone()));
            assert_eq!(
                decode_request(&f),
                Err(ProtocolError::Field("validation out fence slot")),
                "{class:?}"
            );
            // The encoder must refuse it too, not only the decoder.
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    encode_request(&HostCallRequest::Atomic(request))
                }))
                .is_err(),
                "{class:?} encoder accepted an out-fence slot"
            );
        }
    }

    #[test]
    fn an_unknown_class_tag_is_rejected_rather_than_defaulted() {
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        f[61] = 0xFF;
        assert_eq!(decode_request(&f), Err(ProtocolError::Field("class tag")));
    }

    #[test]
    fn an_untagged_event_token_does_not_survive_the_wire() {
        // The purpose tag is checked on decode, so a raw literal token is a
        // protocol error rather than a value the helper acts on.
        let mut f = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        f[52..60].copy_from_slice(&0x66u64.to_le_bytes());
        assert_eq!(decode_request(&f), Err(ProtocolError::Field("event token")));
    }

    #[test]
    fn the_handshake_round_trips_and_carries_the_epoch() {
        // spec:416-425 is unqualified: every executor request and reply
        // carries the epoch. The handshake is a separate frame family only so
        // the host-call accessors can stay total and bare.
        let request = HandshakeRequest {
            incarnation: IncarnationId::from_raw(7),
            lifecycle_epoch: LifecycleEpochId::from_raw(9),
        };
        let f = encode_handshake_request(&request);
        assert_eq!(u64_at(&f, 12), 7, "incarnation @0");
        assert_eq!(u64_at(&f, 20), 9, "lifecycle_epoch @8");
        assert_eq!(decode_handshake_request(&f).expect("decode"), request);

        let reply = HandshakeReply {
            incarnation: IncarnationId::from_raw(7),
            lifecycle_epoch: LifecycleEpochId::from_raw(9),
            helper_pid: 4321,
        };
        assert_eq!(
            decode_handshake_reply(&encode_handshake_reply(&reply)).expect("decode"),
            reply
        );
    }

    #[test]
    fn each_frame_family_rejects_the_others_kind() {
        // The two families share one transport, so neither decoder may
        // misread the other's frames.
        let handshake = encode_handshake_request(&HandshakeRequest {
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
        });
        assert!(matches!(
            decode_request(&handshake),
            Err(ProtocolError::Kind(_))
        ));
        assert!(matches!(
            decode_reply(&handshake),
            Err(ProtocolError::Kind(_))
        ));

        let host_call = encode_request(&HostCallRequest::Atomic(golden_atomic_request_for_tests()));
        assert!(matches!(
            decode_handshake_request(&host_call),
            Err(ProtocolError::Kind(_))
        ));
    }
}
