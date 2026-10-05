//! Skeleton of the single-threaded core loop.
//!
//! B4 established the shape; D4 wired `Message::Request` against the
//! new `process_request` entry point and the lifecycle arms
//! (SetupAllocate, ClientSetupComplete, ClientDisconnected, HostInput).
//! E3/E4 (DRM + signalfd) and F2
//! (host-X11) supply the missing token arms; D5 supplies the
//! listener.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    sync::Arc,
    time::{Duration, Instant},
};

use log::{error, warn};
use mio::{Events, Interest, Poll, unix::SourceFd};

use super::{
    auth::AuthState,
    client_io::{self, WriteOutcome},
    generation::{self, Generation},
    input_inventory::InputInventory,
    message::{HostInputEvent, Message, SetupAllocateResponse, XiConfigRequest},
    poll_tokens::{
        ClientIdAllocator, NOTIFY_TOKEN, backend_token, client_token, listener_token,
        token_to_backend_index, token_to_client, token_to_listener_index,
    },
    process_request::{
        CrtcConfigPublication, CrtcConfigReply, PendingForcedReprobe, PropertyDispatchError,
        RequestOutcome, ValidatedXiChange, complete_crtc_config_reply,
        complete_forced_reprobe_reply, fire_present_configure_notify_for_window, process_request,
        publish_crtc_config,
    },
    reset::{GenerationLocals, ResetAction, ResetPolicy, ResetTrigger, reset_generation},
    sender::{CoreReceiver, CoreSender},
    setup_thread::{self, SetupRegistry},
    xdmcp::{XDMCP_TOKEN, XdmcpOutcome, XdmcpService},
};
use crate::{
    backend::{
        Backend, BackendFdKind, CrtcConfigToken, HostSocketStatus, RequesterAbandon,
        RequesterlessPublication,
    },
    host_x11::HostEvent,
    server::{KeyRepeatState, ServerState},
    transport::{Listener, Transport},
};

/// Diagnostic: per-second loop telemetry emit interval. Toggle via
/// `YSERVER_LOOP_TELEMETRY=1` env var (off by default to avoid log
/// spam in normal runs). When on, every ~1s we emit a single
/// `info!` line with:
///   - iterations/sec
///   - requests/sec + max-drain-per-iter
///   - top-3 opcodes by count + total time
///   - host_input + page_flip dispatches/sec
///   - max time between subsequent HostInput dispatches (cursor-lag proxy)
///   - max single-iteration wall time
///   - total/per-client deferred depth and request age
///   - largest shared-channel request drain, including its dominant client
///   - accepted sequence 0xffff/0x0000 boundary counts
///
/// Costs are deliberately accepted only when explicitly enabled: request
/// timestamps cross the reader boundary, and the core maintains small
/// per-client maps in addition to the existing per-opcode counters.
const TELEMETRY_EMIT_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum time the core waits for the input thread's VT pause barrier. Xorg
/// handles XI property requests synchronously before its VT disable sequence
/// (`Xi/xiproperty.c:1156-1158`, `hw/xfree86/common/xf86Events.c:310-313`);
/// this deadline prevents a failed producer from freezing that boundary.
const VT_INPUT_PAUSE_TIMEOUT: Duration = Duration::from_millis(500);

/// Number of opcodes to show in the per-second telemetry emit.
const TELEMETRY_TOP_N: usize = 3;

#[derive(Debug, Default)]
struct ClientLoopTelemetry {
    deferred_current: usize,
    deferred_max: usize,
    accepted: u64,
    dispatched: u64,
    request_age_max: Duration,
    sequence_ffff: u64,
    sequence_zero: u64,
    requests_by_opcode: HashMap<(u8, Option<u8>), u64>,
}

#[derive(Debug, Default)]
pub(crate) struct LoopTelemetry {
    enabled: bool,
    last_emit: Option<Instant>,
    iter_count: u64,
    requests_total: u64,
    requests_per_iter_max: u32,
    requests_by_opcode: HashMap<u8, (u64, Duration)>,
    request_total_time: Duration,
    longest_request: (u8, Duration),
    host_input_count: u64,
    host_input_max_gap: Duration,
    last_host_input: Option<Instant>,
    page_flip_count: u64,
    max_iter_wall: Duration,
    /// Peak depth of `deferred_requests` observed this window.
    ///
    /// Distinguishes two failure modes that look identical from the
    /// outside during a request flood. A shallow backlog (tens) means
    /// the drain keeps up and any residual stutter is
    /// request-vs-input scheduling — what `REQUEST_TIME_BUDGET`
    /// addresses. A deep, growing backlog (thousands) means requests
    /// arrive faster than they drain, so a low-rate client's request
    /// (marco's `ConfigureWindow`, which is what actually moves a
    /// dragged window) waits behind a high-rate client's flood — a
    /// per-client fairness problem the time budget does NOT fix.
    /// Added because that distinction had been argued repeatedly
    /// without ever being measured.
    deferred_current: usize,
    max_deferred_depth: usize,
    clients: HashMap<yserver_protocol::x11::ClientId, ClientLoopTelemetry>,
    channel_request_batch_max: usize,
    channel_client_batch_max: (u32, usize),
    /// Last `report_export_holders` run, and whether it saw a change.
    export_holders_last: Option<Instant>,
    export_holders_changed: bool,
}

impl LoopTelemetry {
    fn new() -> Self {
        let enabled = std::env::var_os("YSERVER_LOOP_TELEMETRY").is_some();
        Self {
            enabled,
            last_emit: None,
            ..Default::default()
        }
    }

    fn record_request(
        &mut self,
        client: yserver_protocol::x11::ClientId,
        opcode: u8,
        data: u8,
        dur: Duration,
        age: Duration,
    ) {
        if !self.enabled {
            return;
        }
        self.requests_total += 1;
        self.request_total_time += dur;
        let entry = self.requests_by_opcode.entry(opcode).or_default();
        entry.0 += 1;
        entry.1 += dur;
        if dur > self.longest_request.1 {
            self.longest_request = (opcode, dur);
        }
        let client_stats = self.clients.entry(client).or_default();
        client_stats.dispatched += 1;
        client_stats.request_age_max = client_stats.request_age_max.max(age);
        let request_key = (opcode, (opcode >= 128).then_some(data));
        *client_stats
            .requests_by_opcode
            .entry(request_key)
            .or_default() += 1;
    }

    fn record_request_accepted(
        &mut self,
        client: yserver_protocol::x11::ClientId,
        sequence: yserver_protocol::x11::SequenceNumber,
    ) {
        if !self.enabled {
            return;
        }
        let client_stats = self.clients.entry(client).or_default();
        client_stats.accepted += 1;
        match sequence.0 {
            0xffff => client_stats.sequence_ffff += 1,
            0 => client_stats.sequence_zero += 1,
            _ => {}
        }
    }

    fn record_deferred_push(&mut self, client: yserver_protocol::x11::ClientId) {
        if !self.enabled {
            return;
        }
        self.deferred_current += 1;
        self.max_deferred_depth = self.max_deferred_depth.max(self.deferred_current);
        let client_stats = self.clients.entry(client).or_default();
        client_stats.deferred_current += 1;
        client_stats.deferred_max = client_stats.deferred_max.max(client_stats.deferred_current);
    }

    fn record_deferred_pop(&mut self, client: yserver_protocol::x11::ClientId) {
        if !self.enabled {
            return;
        }
        self.deferred_current = self.deferred_current.saturating_sub(1);
        let client_stats = self.clients.entry(client).or_default();
        client_stats.deferred_current = client_stats.deferred_current.saturating_sub(1);
    }

    fn record_channel_drain(
        &mut self,
        requests: usize,
        requests_by_client: &HashMap<yserver_protocol::x11::ClientId, usize>,
    ) {
        if !self.enabled {
            return;
        }
        self.channel_request_batch_max = self.channel_request_batch_max.max(requests);
        if let Some((&client, &count)) = requests_by_client.iter().max_by_key(|(_, count)| *count)
            && count > self.channel_client_batch_max.1
        {
            self.channel_client_batch_max = (client.0, count);
        }
    }

    fn record_host_input(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        self.host_input_count += 1;
        if let Some(prev) = self.last_host_input {
            let gap = now.saturating_duration_since(prev);
            if gap > self.host_input_max_gap {
                self.host_input_max_gap = gap;
            }
        }
        self.last_host_input = Some(now);
    }

    fn record_iteration(&mut self, requests_this_iter: u32, iter_wall: Duration) {
        if !self.enabled {
            return;
        }
        self.iter_count += 1;
        if requests_this_iter > self.requests_per_iter_max {
            self.requests_per_iter_max = requests_this_iter;
        }
        if iter_wall > self.max_iter_wall {
            self.max_iter_wall = iter_wall;
        }
    }

    /// Whether the export-holders report is due: enabled and 1 s since the last run.
    fn export_holders_due(&self, now: Instant) -> bool {
        self.enabled
            && self
                .export_holders_last
                .is_none_or(|last| now.saturating_duration_since(last) >= TELEMETRY_EMIT_INTERVAL)
    }

    fn note_export_holders(&mut self, now: Instant, changed: bool) {
        self.export_holders_last = Some(now);
        self.export_holders_changed = changed;
    }

    /// Re-check a second after a change so a set that settles while idle still gets logged.
    fn export_holders_deadline(&self) -> Option<Instant> {
        let last = self.export_holders_last?;
        self.export_holders_changed
            .then(|| last + TELEMETRY_EMIT_INTERVAL)
    }

    fn maybe_emit(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        let last = match self.last_emit {
            Some(t) => t,
            None => {
                self.last_emit = Some(now);
                return;
            }
        };
        let elapsed = now.saturating_duration_since(last);
        if elapsed < TELEMETRY_EMIT_INTERVAL {
            return;
        }
        let secs = elapsed.as_secs_f64().max(1e-6);

        // Top-N opcodes by total time (the most-actionable view; opcodes
        // that fire often but cheap-each don't dominate, opcodes that
        // fire rarely but expensive-each do).
        let mut by_time: Vec<(u8, u64, Duration)> = self
            .requests_by_opcode
            .iter()
            .map(|(op, (cnt, t))| (*op, *cnt, *t))
            .collect();
        by_time.sort_by_key(|(_, _, total)| std::cmp::Reverse(*total));
        let top_time: Vec<String> = by_time
            .iter()
            .take(TELEMETRY_TOP_N)
            .map(|(op, cnt, t)| format!("op{op}:n={cnt}/t={:.1}ms", t.as_secs_f64() * 1000.0))
            .collect();

        let mut by_count = by_time.clone();
        by_count.sort_by_key(|(_, count, _)| std::cmp::Reverse(*count));
        let top_count: Vec<String> = by_count
            .iter()
            .take(TELEMETRY_TOP_N)
            .map(|(op, cnt, t)| format!("op{op}:n={cnt}/t={:.1}ms", t.as_secs_f64() * 1000.0))
            .collect();

        let mut deferred_clients: Vec<_> = self.clients.iter().collect();
        deferred_clients.sort_by_key(|(_, stats)| std::cmp::Reverse(stats.deferred_max));
        let top_deferred: Vec<String> = deferred_clients
            .iter()
            .take(TELEMETRY_TOP_N)
            .map(|(id, stats)| {
                format!(
                    "c{}:cur={}/max={}",
                    id.0, stats.deferred_current, stats.deferred_max
                )
            })
            .collect();

        let mut age_clients: Vec<_> = self.clients.iter().collect();
        age_clients.sort_by_key(|(_, stats)| std::cmp::Reverse(stats.request_age_max));
        let top_age: Vec<String> = age_clients
            .iter()
            .take(TELEMETRY_TOP_N)
            .map(|(id, stats)| {
                format!(
                    "c{}:n={}/max={:.1}ms",
                    id.0,
                    stats.dispatched,
                    stats.request_age_max.as_secs_f64() * 1000.0
                )
            })
            .collect();
        let sequence_ffff: u64 = self.clients.values().map(|stats| stats.sequence_ffff).sum();
        let sequence_zero: u64 = self.clients.values().map(|stats| stats.sequence_zero).sum();

        let mut request_clients: Vec<_> = self.clients.iter().collect();
        request_clients
            .sort_by_key(|(_, stats)| std::cmp::Reverse(stats.accepted.max(stats.dispatched)));
        let request_client_mix: Vec<String> = request_clients
            .iter()
            .filter(|(_, stats)| stats.accepted != 0 || stats.dispatched != 0)
            .take(TELEMETRY_TOP_N)
            .map(|(id, stats)| {
                let mut operations: Vec<_> = stats.requests_by_opcode.iter().collect();
                operations.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
                let top: Vec<String> = operations
                    .iter()
                    .take(5)
                    .map(|((major, minor), count)| match minor {
                        Some(minor) => format!("{major}.{minor}={count}"),
                        None => format!("{major}={count}"),
                    })
                    .collect();
                format!(
                    "c{}:accepted={}/dispatched={}/top={}",
                    id.0,
                    stats.accepted,
                    stats.dispatched,
                    top.join("|")
                )
            })
            .collect();

        let outbound = crate::core_loop::fanout::take_outbound_telemetry();
        let mut outbound_by_client: HashMap<_, Vec<_>> = HashMap::new();
        for ((client, kind), count) in outbound {
            outbound_by_client
                .entry(client)
                .or_default()
                .push((kind, count));
        }
        let mut outbound_clients: Vec<_> = outbound_by_client.into_iter().collect();
        outbound_clients.sort_by_key(|(_, kinds)| {
            std::cmp::Reverse(kinds.iter().map(|(_, count)| count).sum::<u64>())
        });
        let outbound_client_mix: Vec<String> = outbound_clients
            .iter_mut()
            .take(TELEMETRY_TOP_N)
            .map(|(id, kinds)| {
                kinds.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
                let total: u64 = kinds.iter().map(|(_, count)| count).sum();
                let top: Vec<String> = kinds
                    .iter()
                    .take(5)
                    .map(|(kind, count)| {
                        use crate::core_loop::fanout::OutboundTelemetryKind;
                        let label = match kind {
                            OutboundTelemetryKind::Reply => "reply".to_string(),
                            OutboundTelemetryKind::Error(code) => format!("err{code}"),
                            OutboundTelemetryKind::Event(event) => format!("e{event}"),
                            OutboundTelemetryKind::GenericEvent {
                                extension,
                                event_type,
                            } => format!("ge{extension}.{event_type}"),
                        };
                        format!("{label}={count}")
                    })
                    .collect();
                format!("c{}:n={total}/top={}", id.0, top.join("|"))
            })
            .collect();

        log::info!(
            "loop telemetry [{:.2}s]: iter/s={:.0} req/s={:.0} drain_max={} \
             req_time={:.1}ms ({:.1}%) longest=op{}:{:.2}ms \
             host_input/s={:.1} gap_max={:.1}ms \
             page_flip/s={:.1} iter_wall_max={:.1}ms deferred={}/{} \
             channel_batch_max={} channel_client_max=c{}:{} seq_boundary[ffff={} zero={}] \
             deferred_clients=[{}] age_clients=[{}] \
             request_clients=[{}] outbound_clients=[{}] \
             top_by_time=[{}] top_by_count=[{}]",
            secs,
            self.iter_count as f64 / secs,
            self.requests_total as f64 / secs,
            self.requests_per_iter_max,
            self.request_total_time.as_secs_f64() * 1000.0,
            self.request_total_time.as_secs_f64() / secs * 100.0,
            self.longest_request.0,
            self.longest_request.1.as_secs_f64() * 1000.0,
            self.host_input_count as f64 / secs,
            self.host_input_max_gap.as_secs_f64() * 1000.0,
            self.page_flip_count as f64 / secs,
            self.max_iter_wall.as_secs_f64() * 1000.0,
            self.deferred_current,
            self.max_deferred_depth,
            self.channel_request_batch_max,
            self.channel_client_batch_max.0,
            self.channel_client_batch_max.1,
            sequence_ffff,
            sequence_zero,
            top_deferred.join(","),
            top_age.join(","),
            request_client_mix.join(","),
            outbound_client_mix.join(","),
            top_time.join(","),
            top_count.join(","),
        );

        // Reset accumulators for next window. Keep `enabled` /
        // `last_host_input` (cross-window gap measurement) /
        // `last_emit`. Everything else zeroes.
        self.last_emit = Some(now);
        self.iter_count = 0;
        self.requests_total = 0;
        self.requests_per_iter_max = 0;
        self.requests_by_opcode.clear();
        self.request_total_time = Duration::ZERO;
        self.longest_request = (0, Duration::ZERO);
        self.host_input_count = 0;
        self.host_input_max_gap = Duration::ZERO;
        self.page_flip_count = 0;
        self.max_iter_wall = Duration::ZERO;
        self.max_deferred_depth = self.deferred_current;
        self.channel_request_batch_max = 0;
        self.channel_client_batch_max = (0, 0);
        self.clients.retain(|_, stats| {
            stats.deferred_max = stats.deferred_current;
            stats.accepted = 0;
            stats.dispatched = 0;
            stats.request_age_max = Duration::ZERO;
            stats.sequence_ffff = 0;
            stats.sequence_zero = 0;
            stats.requests_by_opcode.clear();
            stats.deferred_current != 0
        });
    }

    /// Drop every per-client row and zero the deferred-depth gauges.
    ///
    /// Called only from the server-reset boundary, and only because the
    /// window rollover above prunes a client row when its
    /// `deferred_current` reaches zero — which happens through
    /// `record_deferred_pop`, i.e. only when a request is actually
    /// dispatched. A reset DISCARDS the queues instead, so without this
    /// the gauge would stay permanently non-zero, the dead client's row
    /// would never be pruned, and — since client ids are reused across
    /// generations — the next generation's client 7 would inherit the
    /// previous one's numbers. Diagnostics only; nothing on the
    /// protocol path reads these.
    #[allow(dead_code)] // called by `reset::reset_generation`; armed in step 5
    pub(crate) fn forget_clients(&mut self) {
        self.clients.clear();
        self.deferred_current = 0;
        self.max_deferred_depth = 0;
    }
}

/// Core-loop work cap. Each main-loop iteration processes at
/// most this many X protocol requests before yielding back to the
/// outer poll / maintenance pass. Excess requests are buffered in
/// `deferred_requests` and picked up at the start of the next
/// iteration.
///
/// **Why this matters** (per the telemetry rollups from the bee /
/// adapta-nokto investigation): without a cap, `Message::Request`
/// can monopolise the thread for SECONDS at a time on a single
/// iteration when GTK fires bursts of RENDER traffic during a
/// window drag — observed iter_wall_max=6884ms with
/// drain_max=32857 in one iteration. During that window,
/// `HostInput` messages and DRM readiness sit undelivered, so the cursor
/// visibly freezes (gap_max
/// up to 8.5 seconds between consecutive cursor events).
///
/// 32 chosen as the initial cap because: typical request cost is
/// ~0.25 ms, so 32 × 0.25 ≈ 8 ms per iteration worst case — about
/// one frame at 120 Hz, well below the perceptual cursor-lag
/// threshold.
///
/// The count cap alone is NOT sufficient: it presumes the ~0.25 ms
/// figure above, and that presumption was measured false. See
/// [`REQUEST_TIME_BUDGET`], which now bounds the same iteration by
/// wall clock. The count cap is retained because for well-behaved
/// requests it binds first (32 × 0.25 ms == the 8 ms budget by
/// construction), so the fast path is unchanged.
const MAX_REQUESTS_PER_ITER: usize = 32;

/// Maximum time a parkable RANDR mutation may wait in the server-wide gate.
const RANDR_GATE_QUEUE_TIMEOUT: Duration = Duration::from_secs(30);

/// Wall-clock ceiling on request processing per main-loop iteration,
/// enforced alongside [`MAX_REQUESTS_PER_ITER`] — whichever trips
/// first ends the drain.
///
/// **Why the count cap was not enough** (measured on silence, dual
/// 1440p, MATE + adapta-nokto, dragging the mate-control-center
/// window — `YSERVER_LOOP_TELEMETRY=1`): GTK emits ~200,000 requests
/// per second during that drag (each themed fill costs CreatePixmap +
/// CreatePicture + FillRectangles + FreePicture + FreePixmap), and
/// individual requests reach **44-50 ms** (`longest=op70:44.23ms`,
/// `op70:49.61ms`) because a request that closes the open frame
/// absorbs the whole batch flush. 32 × 44 ms is ~1.4 s inside one
/// iteration, while `HostInput` and DRM readiness still need service —
/// so the cursor and the window position stall together
/// (`gap_max` 225-360 ms between consecutive input events, against
/// `host_input/s` ≈ 128 arriving fine). The visible symptom is a drag
/// that tracks, lags, then skips.
///
/// A deadline cannot preempt a request already running, so this does
/// not make a 44 ms request cheaper — it stops that request from
/// authorising 31 more. Worst-case iteration becomes one overrunning
/// request instead of 32.
///
/// 8 ms is the figure `MAX_REQUESTS_PER_ITER` was already aiming at
/// (one frame at 120 Hz), so this restores the intended design point
/// rather than picking a new one.
const REQUEST_TIME_BUDGET: Duration = Duration::from_millis(8);

/// One backend-owned source registered with the core poller. The vector index
/// is encoded in its mio token, preserving the exact fd identity even when
/// several entries share one `BackendFdKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackendPollSource {
    fd: RawFd,
    kind: BackendFdKind,
}

fn deregister_backend_poll_sources(
    registry: &mio::Registry,
    sources: &[BackendPollSource],
) -> io::Result<()> {
    for source in sources {
        if let Err(error) = registry.deregister(&mut SourceFd(&source.fd))
            && !matches!(error.raw_os_error(), Some(libc::EBADF | libc::ENOENT))
        {
            return Err(error);
        }
    }
    Ok(())
}

fn register_backend_poll_sources(
    registry: &mio::Registry,
    sources: &[BackendPollSource],
) -> io::Result<()> {
    for (index, source) in sources.iter().enumerate() {
        let token = backend_token(index).ok_or_else(|| {
            io::Error::other(format!(
                "backend exposes too many poll fds: {}",
                sources.len()
            ))
        })?;
        registry.register(&mut SourceFd(&source.fd), token, Interest::READABLE)?;
    }
    Ok(())
}

/// Whether this iteration's request drain must stop, given how many
/// requests remain in the count budget and how long the drain has been
/// running. Split out as a pure function so the count-vs-deadline
/// interaction is unit-testable without driving the whole core loop.
///
/// `elapsed` is measured from the top of the iteration, so the first
/// request always passes (`elapsed` ≈ 0) — that guarantees forward
/// progress even when every request overruns the budget.
fn budget_exhausted(remaining: usize, elapsed: Duration) -> bool {
    remaining == 0 || elapsed >= REQUEST_TIME_BUDGET
}

/// One pending X protocol request accepted by a reader but not yet dispatched.
pub(crate) struct DeferredRequest {
    id: yserver_protocol::x11::ClientId,
    sequence: yserver_protocol::x11::SequenceNumber,
    accepted_at: Option<Instant>,
    randr_gate_ticket: Option<u64>,
    randr_gate_expired: bool,
    header: yserver_protocol::x11::RequestHeader,
    body: Vec<u8>,
    attached_fd: Option<OwnedFd>,
}

/// A request whose backend work is running asynchronously. The raw request is
/// deliberately not retained: validation and begin-side effects run exactly
/// once, and completion resumes only the protocol reply/notification tail.
#[derive(Clone, Copy)]
struct ParkedCrtcConfig {
    token: CrtcConfigToken,
    client_id: yserver_protocol::x11::ClientId,
    sequence: yserver_protocol::x11::SequenceNumber,
    reply: CrtcConfigReply,
    request_wire_bytes: usize,
}

#[derive(Clone, Copy)]
struct ParkedForcedReprobe {
    client_id: Option<yserver_protocol::x11::ClientId>,
    sequence: yserver_protocol::x11::SequenceNumber,
    byte_order: yserver_protocol::x11::ClientByteOrder,
    request_wire_bytes: usize,
}

/// Backend waits indexed both by opaque token (completion) and by client
/// (strict same-client FIFO blocking/cancellation).
#[derive(Default)]
pub(crate) struct PendingBackendRequests {
    crtc_by_token: HashMap<CrtcConfigToken, ParkedCrtcConfig>,
    crtc_by_client: HashMap<yserver_protocol::x11::ClientId, CrtcConfigToken>,
    forced_reprobe_by_token: HashMap<CrtcConfigToken, ParkedForcedReprobe>,
    forced_reprobe_by_client: HashMap<yserver_protocol::x11::ClientId, CrtcConfigToken>,
    xi_config_clients: HashSet<yserver_protocol::x11::ClientId>,
}

impl PendingBackendRequests {
    fn client_is_blocked(&self, client: yserver_protocol::x11::ClientId) -> bool {
        self.crtc_by_client.contains_key(&client)
            || self.forced_reprobe_by_client.contains_key(&client)
            || self.xi_config_clients.contains(&client)
    }

    fn block_xi_config(&mut self, client: yserver_protocol::x11::ClientId) -> bool {
        self.xi_config_clients.insert(client)
    }

    fn unblock_xi_config(&mut self, client: yserver_protocol::x11::ClientId) {
        self.xi_config_clients.remove(&client);
    }

    #[cfg(test)]
    pub(crate) fn xi_config_client_is_blocked_for_test(
        &self,
        client: yserver_protocol::x11::ClientId,
    ) -> bool {
        self.xi_config_clients.contains(&client)
    }

    fn park_crtc(&mut self, parked: ParkedCrtcConfig) -> Result<(), &'static str> {
        let client = parked.client_id;
        let token = parked.token;
        if self.crtc_by_client.contains_key(&client)
            || self.forced_reprobe_by_client.contains_key(&client)
        {
            return Err("client already has a pending backend request");
        }
        if self.crtc_by_token.contains_key(&token)
            || self.forced_reprobe_by_token.contains_key(&token)
        {
            return Err("backend reused a live CRTC configuration token");
        }
        self.crtc_by_client.insert(client, token);
        self.crtc_by_token.insert(token, parked);
        Ok(())
    }

    fn take_crtc(&mut self, token: CrtcConfigToken) -> Option<ParkedCrtcConfig> {
        let parked = self.crtc_by_token.remove(&token)?;
        self.crtc_by_client.remove(&parked.client_id);
        Some(parked)
    }

    fn take_crtc_reply(&mut self, token: CrtcConfigToken) -> Option<ParkedCrtcConfig> {
        self.take_crtc(token)
    }

    fn park_forced_reprobe(
        &mut self,
        pending: PendingForcedReprobe,
        client_id: yserver_protocol::x11::ClientId,
        sequence: yserver_protocol::x11::SequenceNumber,
        request_wire_bytes: usize,
    ) -> Result<(), &'static str> {
        let token = pending.token;
        if self.crtc_by_client.contains_key(&client_id)
            || self.forced_reprobe_by_client.contains_key(&client_id)
        {
            return Err("client already has a pending backend request");
        }
        if self.crtc_by_token.contains_key(&token)
            || self.forced_reprobe_by_token.contains_key(&token)
        {
            return Err("backend reused a live asynchronous token");
        }
        self.forced_reprobe_by_client.insert(client_id, token);
        self.forced_reprobe_by_token.insert(
            token,
            ParkedForcedReprobe {
                client_id: Some(client_id),
                sequence,
                byte_order: pending.byte_order,
                request_wire_bytes,
            },
        );
        Ok(())
    }

    fn take_forced_reprobe_reply(&mut self, token: CrtcConfigToken) -> Option<ParkedForcedReprobe> {
        let parked = self.forced_reprobe_by_token.remove(&token)?;
        if let Some(client_id) = parked.client_id {
            self.forced_reprobe_by_client.remove(&client_id);
        }
        Some(parked)
    }

    /// Detach a departed requester's reply while retaining the token and its
    /// gate turn until the backend work reaches a terminal result.
    fn detach_forced_reprobe_requester(&mut self, client_id: yserver_protocol::x11::ClientId) {
        let Some(token) = self.forced_reprobe_by_client.remove(&client_id) else {
            return;
        };
        if let Some(parked) = self.forced_reprobe_by_token.get_mut(&token) {
            parked.client_id = None;
        }
    }

    fn take_client_crtc(
        &mut self,
        client: yserver_protocol::x11::ClientId,
    ) -> Option<CrtcConfigToken> {
        let token = self.crtc_by_client.remove(&client)?;
        self.crtc_by_token.remove(&token);
        Some(token)
    }

    /// Park a CRTC configuration with only the fields a lifetime test
    /// needs. The protocol continuation is inert filler: nothing here
    /// completes the request, it only has to be cancellable.
    #[cfg(test)]
    pub(crate) fn park_crtc_for_test(
        &mut self,
        client: yserver_protocol::x11::ClientId,
        token: CrtcConfigToken,
    ) -> Result<(), &'static str> {
        self.park_crtc(ParkedCrtcConfig {
            token,
            client_id: client,
            sequence: yserver_protocol::x11::SequenceNumber(1),
            reply: CrtcConfigReply {
                byte_order: yserver_protocol::x11::ClientByteOrder::LittleEndian,
            },
            request_wire_bytes: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.crtc_by_token.is_empty()
            && self.crtc_by_client.is_empty()
            && self.forced_reprobe_by_token.is_empty()
            && self.forced_reprobe_by_client.is_empty()
            && self.xi_config_clients.is_empty()
    }

    pub(crate) fn take_all_crtc_tokens(&mut self) -> Vec<CrtcConfigToken> {
        self.crtc_by_client.clear();
        self.forced_reprobe_by_client.clear();
        self.crtc_by_token
            .drain()
            .map(|(token, _)| token)
            .chain(self.forced_reprobe_by_token.drain().map(|(token, _)| token))
            .collect()
    }
}

/// The RANDR extension's major opcode, as `process_request.rs` fixes it.
const RANDR_MAJOR_OPCODE: u8 = 128;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum RandrRequestClass {
    NonGate,
    Mutation,
    ForcedReprobe,
}

/// Classify every RANDR minor opcode without consulting request contents.
/// Unknown and reserved minors are non-gating because they cannot change
/// configuration in `handle_randr_request`.
fn classify_randr_minor(minor: u8) -> RandrRequestClass {
    use yserver_protocol::x11::randr as rr;

    match minor {
        rr::RR_SET_SCREEN_CONFIG
        | rr::RR_SET_SCREEN_SIZE
        | rr::RR_SET_CRTC_CONFIG
        | rr::RR_SET_OUTPUT_PRIMARY
        | rr::RR_SET_PROVIDER_OUTPUT_SOURCE
        | rr::RR_SET_PROVIDER_OFFLOAD_SINK
        | rr::RR_SET_PANNING
        | rr::RR_SET_CRTC_TRANSFORM
        | rr::RR_SET_MONITOR
        | rr::RR_DELETE_MONITOR => RandrRequestClass::Mutation,
        rr::RR_GET_SCREEN_RESOURCES => RandrRequestClass::ForcedReprobe,
        _ => RandrRequestClass::NonGate,
    }
}

#[derive(Debug, Clone, Copy)]
struct RandrGateWaiter {
    ticket: u64,
    client_id: yserver_protocol::x11::ClientId,
    class: RandrRequestClass,
    arrived_at: Instant,
    expires_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct RandrGateFlight {
    kind: RandrGateFlightKind,
    token: Option<CrtcConfigToken>,
    publication: Option<CrtcConfigPublication>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum RandrGateFlightKind {
    Mutation,
    ForcedReprobe,
}

/// Server-wide serial admission for configuration-changing RANDR requests.
/// Requests receive a ticket when accepted from the core channel, preserving
/// their cross-client arrival order independently of the fair ready ring.
#[derive(Default)]
pub(crate) struct RandrMutationGate {
    waiting: VecDeque<RandrGateWaiter>,
    in_flight: Option<RandrGateFlight>,
    topology_episode: Option<u64>,
    requested_topology_episodes: VecDeque<u64>,
    requesterless_publications: VecDeque<RequesterlessPublication>,
    next_ticket: u64,
}

impl RandrMutationGate {
    fn hold_forced_reprobe_turn_for_tests(&mut self, token: CrtcConfigToken) {
        assert!(
            self.in_flight.is_none(),
            "test forced turn starts on an idle gate"
        );
        self.in_flight = Some(RandrGateFlight {
            kind: RandrGateFlightKind::ForcedReprobe,
            token: Some(token),
            publication: None,
        });
    }

    fn active_install_capable(&self, backend: &dyn Backend) -> bool {
        self.in_flight
            .as_ref()
            .and_then(|flight| flight.token)
            .is_some_and(|token| backend.crtc_config_install_capable(token))
    }

    fn holds_requesterless_turn(&self, backend: &dyn Backend) -> bool {
        self.topology_episode.is_some()
            || !self.requested_topology_episodes.is_empty()
            || self
                .in_flight
                .as_ref()
                .is_some_and(|flight| flight.kind == RandrGateFlightKind::ForcedReprobe)
            || self.active_install_capable(backend)
    }

    fn request_topology_episode(&mut self, episode_id: u64) {
        if self.topology_episode == Some(episode_id)
            || self.requested_topology_episodes.contains(&episode_id)
        {
            log::error!("topology episode {episode_id} was requested twice");
            return;
        }
        self.requested_topology_episodes.push_back(episode_id);
    }

    fn begin_topology_episode(
        &mut self,
        state: &mut ServerState,
        backend: &mut dyn Backend,
        episode_id: u64,
    ) {
        if let Some(active) = self.topology_episode {
            log::error!("topology episode {episode_id} began while episode {active} is still open");
            debug_assert!(false, "only one topology episode may be open at a time");
            return;
        }
        self.topology_episode = Some(episode_id);
        backend.on_topology_episode_granted(state, episode_id);
    }

    fn withdraw_topology_episode_request(&mut self, episode_id: u64) -> bool {
        let Some(index) = self
            .requested_topology_episodes
            .iter()
            .position(|requested| *requested == episode_id)
        else {
            return false;
        };
        self.requested_topology_episodes.remove(index);
        true
    }

    fn grant_requested_topology_episode(
        &mut self,
        state: &mut ServerState,
        backend: &mut dyn Backend,
    ) {
        if self.in_flight.is_some() || self.topology_episode.is_some() {
            return;
        }
        let Some(episode_id) = self.requested_topology_episodes.pop_front() else {
            return;
        };
        self.begin_topology_episode(state, backend, episode_id);
    }

    fn finish_topology_episode(&mut self, episode_id: u64) -> bool {
        match self.topology_episode {
            Some(active) if active == episode_id => {
                self.topology_episode = None;
                true
            }
            active => {
                log::error!(
                    "topology episode {episode_id} ended while active episode is {active:?}"
                );
                debug_assert_eq!(
                    active,
                    Some(episode_id),
                    "topology episode end must match the open episode"
                );
                false
            }
        }
    }

    fn register_request(&mut self, req: &mut DeferredRequest, backend: &dyn Backend) {
        if req.header.opcode != RANDR_MAJOR_OPCODE || req.randr_gate_ticket.is_some() {
            return;
        }
        let class = classify_randr_minor(req.header.data);
        if class == RandrRequestClass::NonGate {
            return;
        }
        if class == RandrRequestClass::ForcedReprobe
            && !self.holds_requesterless_turn(backend)
            && !backend.forced_reprobe_may_be_pending()
        {
            return;
        }
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1);
        req.randr_gate_ticket = Some(ticket);
        let arrived_at = req.accepted_at.unwrap_or_else(Instant::now);
        self.waiting.push_back(RandrGateWaiter {
            ticket,
            client_id: req.id,
            class,
            arrived_at,
            expires_at: (class == RandrRequestClass::Mutation
                && req.header.data == yserver_protocol::x11::randr::RR_SET_CRTC_CONFIG)
                .then(|| arrived_at + RANDR_GATE_QUEUE_TIMEOUT),
        });
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.waiting
            .iter()
            .filter_map(|waiter| waiter.expires_at)
            .min()
    }

    fn take_expired_waiters(&mut self, now: Instant) -> Vec<RandrGateWaiter> {
        let expired = self.expired_waiter_order(now);
        let tickets: HashSet<_> = expired.iter().map(|waiter| waiter.ticket).collect();
        self.waiting
            .retain(|waiter| !tickets.contains(&waiter.ticket));
        expired
    }

    fn expired_waiter_order(&self, now: Instant) -> Vec<RandrGateWaiter> {
        self.waiting
            .iter()
            .filter(|waiter| waiter.expires_at.is_some_and(|deadline| now >= deadline))
            .copied()
            .collect()
    }

    fn remove_waiters_for_client(&mut self, client: yserver_protocol::x11::ClientId) {
        self.waiting.retain(|waiter| waiter.client_id != client);
    }

    fn request_is_runnable(&self, req: &DeferredRequest, backend: &dyn Backend) -> bool {
        let Some(ticket) = req.randr_gate_ticket else {
            let request_class = if req.header.opcode == RANDR_MAJOR_OPCODE {
                classify_randr_minor(req.header.data)
            } else {
                RandrRequestClass::NonGate
            };
            return match request_class {
                RandrRequestClass::Mutation => {
                    self.in_flight.is_none()
                        && self.topology_episode.is_none()
                        && self.requested_topology_episodes.is_empty()
                }
                RandrRequestClass::ForcedReprobe => !self.holds_requesterless_turn(backend),
                RandrRequestClass::NonGate => true,
            };
        };
        let Some(waiter) = self.waiting.iter().find(|waiter| waiter.ticket == ticket) else {
            return true;
        };
        match waiter.class {
            RandrRequestClass::Mutation => {
                self.in_flight.is_none()
                    && self.topology_episode.is_none()
                    && self.requested_topology_episodes.is_empty()
                    && self
                        .waiting
                        .front()
                        .is_some_and(|head| head.ticket == ticket)
            }
            RandrRequestClass::ForcedReprobe => {
                self.in_flight.is_none()
                    && self.topology_episode.is_none()
                    && self.requested_topology_episodes.is_empty()
                    && self
                        .waiting
                        .front()
                        .is_some_and(|head| head.ticket == ticket)
            }
            RandrRequestClass::NonGate => true,
        }
    }

    /// Consume a gate request immediately before it is dispatched. `true`
    /// means this request owns the gate through its synchronous completion or
    /// pending backend result. A forced reprobe owns the same turn while its
    /// worker runs, so a topology episode cannot publish ahead of its reply.
    fn admit(&mut self, req: &DeferredRequest, _backend: &dyn Backend) -> bool {
        let Some(ticket) = req.randr_gate_ticket else {
            return false;
        };
        let Some(index) = self
            .waiting
            .iter()
            .position(|waiter| waiter.ticket == ticket)
        else {
            return false;
        };
        let waiter = self.waiting[index];
        debug_assert_eq!(waiter.client_id, req.id);
        debug_assert!(waiter.arrived_at <= Instant::now());

        match waiter.class {
            RandrRequestClass::Mutation => {
                if self.in_flight.is_some()
                    || self.topology_episode.is_some()
                    || !self.requested_topology_episodes.is_empty()
                    || self
                        .waiting
                        .front()
                        .is_none_or(|head| head.ticket != ticket)
                {
                    return false;
                }
                self.waiting.pop_front();
                self.in_flight = Some(RandrGateFlight {
                    kind: RandrGateFlightKind::Mutation,
                    token: None,
                    publication: None,
                });
                true
            }
            RandrRequestClass::ForcedReprobe => {
                if self.in_flight.is_none()
                    && self.topology_episode.is_none()
                    && self.requested_topology_episodes.is_empty()
                    && self
                        .waiting
                        .front()
                        .is_some_and(|head| head.ticket == ticket)
                {
                    self.waiting.pop_front();
                    self.in_flight = Some(RandrGateFlight {
                        kind: RandrGateFlightKind::ForcedReprobe,
                        token: None,
                        publication: None,
                    });
                    true
                } else {
                    false
                }
            }
            RandrRequestClass::NonGate => false,
        }
    }

    fn mark_pending(&mut self, token: CrtcConfigToken, publication: CrtcConfigPublication) {
        let Some(flight) = self.in_flight.as_mut() else {
            debug_assert!(false, "pending CRTC config without a RANDR gate owner");
            return;
        };
        flight.token = Some(token);
        flight.publication = Some(publication);
    }

    fn mark_forced_reprobe_pending(&mut self, token: CrtcConfigToken) {
        let Some(flight) = self.in_flight.as_mut() else {
            debug_assert!(false, "pending forced reprobe without a gate owner");
            return;
        };
        debug_assert_eq!(flight.kind, RandrGateFlightKind::ForcedReprobe);
        flight.token = Some(token);
    }

    fn owns_pending_token(&self, token: CrtcConfigToken) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|flight| flight.token == Some(token))
    }

    fn take_publication(&mut self, token: CrtcConfigToken) -> Option<CrtcConfigPublication> {
        self.in_flight
            .as_mut()
            .filter(|flight| {
                flight.kind == RandrGateFlightKind::Mutation && flight.token == Some(token)
            })
            .and_then(|flight| flight.publication.take())
    }

    fn finish_synchronous(&mut self) {
        self.in_flight = None;
    }

    fn finish_pending(&mut self, token: CrtcConfigToken) {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|flight| flight.token == Some(token))
        {
            self.in_flight = None;
        }
    }

    fn queue_requesterless_publications(
        &mut self,
        publications: impl IntoIterator<Item = RequesterlessPublication>,
    ) {
        self.requesterless_publications.extend(publications);
    }

    fn take_requesterless_publications(&mut self) -> Vec<RequesterlessPublication> {
        self.requesterless_publications.drain(..).collect()
    }

    pub(crate) fn clear(&mut self) {
        self.waiting.clear();
        self.in_flight = None;
        self.topology_episode = None;
        self.requested_topology_episodes.clear();
        self.requesterless_publications.clear();
        self.next_ticket = 0;
    }

    #[cfg(test)]
    fn is_busy(&self) -> bool {
        self.in_flight.is_some() || self.topology_episode.is_some()
    }
}

fn defer_reset_action_for_install_capable_mutation(
    gate: &RandrMutationGate,
    backend: &dyn Backend,
    deferred: &mut Option<ResetAction>,
    newly_pending: Option<ResetAction>,
) -> Option<ResetAction> {
    let action = match (deferred.take(), newly_pending) {
        (Some(ResetAction::Reset), _) | (_, Some(ResetAction::Reset)) => Some(ResetAction::Reset),
        (Some(ResetAction::Terminate), _) | (_, Some(ResetAction::Terminate)) => {
            Some(ResetAction::Terminate)
        }
        (None, None) => None,
    };
    if gate.active_install_capable(backend) {
        *deferred = action;
        None
    } else {
        action
    }
}

fn xdmcp_termination_can_finish(
    termination_pending: bool,
    gate: &RandrMutationGate,
    backend: &dyn Backend,
) -> bool {
    termination_pending && !gate.active_install_capable(backend)
}

fn observe_xdmcp_session_departure(
    service: &mut XdmcpService,
    state: &ServerState,
    auth: &AuthState,
    generation: crate::core_loop::Generation,
) {
    if let Some(client) = service.live_session_client()
        && !state.clients.contains_key(&client.0)
    {
        service.note_session_client_disconnected(client, auth, generation);
    }
}

/// An accepted recognized property write waiting for its FIFO turn. The
/// generation is stamped by the runner, after request parsing has captured
/// only request-local wire data.
struct QueuedXiConfig {
    request: XiConfigRequest,
    generation: Generation,
    request_wire_bytes: usize,
}

struct XiConfigCompletion {
    validated: ValidatedXiChange,
    generation: Generation,
    request_wire_bytes: usize,
}

/// The one submitted input operation remains here through reset or client
/// disconnect. `protocol=None` drops generation/atom/reply metadata while the
/// source, setting and token continue to occupy the global FIFO lane.
struct XiConfigInFlight {
    token: crate::xinput::libinput_props::DeviceConfigToken,
    source: crate::xinput::InputSourceId,
    change: crate::xinput::libinput_props::DeviceConfigChange,
    cancel: crate::xinput::libinput_props::DeviceConfigCancelToken,
    protocol: Option<XiConfigCompletion>,
}

/// The client has already received BadMatch, but the input thread may have
/// crossed its pre-apply cancellation check before the timeout. Keep enough
/// information to reconcile a later confirmation without another reply.
struct TimedOutXiConfig {
    change: crate::xinput::libinput_props::DeviceConfigChange,
}

#[derive(Default)]
pub(crate) struct XiConfigLane {
    queued: VecDeque<QueuedXiConfig>,
    in_flight: Option<XiConfigInFlight>,
    timed_out: HashMap<
        (
            crate::xinput::libinput_props::DeviceConfigToken,
            crate::xinput::InputSourceId,
        ),
        TimedOutXiConfig,
    >,
    reject_unsubmitted_for_vt_release: bool,
}

impl XiConfigLane {
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.queued.is_empty() && self.in_flight.is_none() && self.timed_out.is_empty()
    }
}

/// Per-client FIFO queues behind a round-robin ready ring.
///
/// Request order is preserved within each client, as required by X11, while a
/// continuously busy client gets at most one request before every other ready
/// client gets a turn. Cross-client request order has no protocol meaning.
#[derive(Default)]
pub(crate) struct FairRequestQueue {
    by_client: HashMap<yserver_protocol::x11::ClientId, VecDeque<DeferredRequest>>,
    ready: VecDeque<yserver_protocol::x11::ClientId>,
    len: usize,
}

impl FairRequestQueue {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drop every queued request. Used only by the server-reset
    /// generation boundary: the clients that issued them are gone, and
    /// a request is meaningless in a generation whose resource ids mean
    /// something else.
    #[allow(dead_code)] // called by `reset::reset_generation`; armed in step 5
    pub(crate) fn clear(&mut self) {
        self.by_client.clear();
        self.ready.clear();
        self.len = 0;
    }

    pub(crate) fn push_back(&mut self, req: DeferredRequest) {
        let client = req.id;
        let queue = self.by_client.entry(client).or_default();
        if queue.is_empty() {
            self.ready.push_back(client);
        }
        queue.push_back(req);
        self.len += 1;
    }

    /// Restore an older, temporarily parked prefix ahead of this client's
    /// remaining requests without changing the client's place in the ready
    /// ring. `requests` must contain exactly one client's requests in arrival
    /// order.
    fn prepend_client(&mut self, mut requests: VecDeque<DeferredRequest>) {
        let Some(first) = requests.front() else {
            return;
        };
        let client = first.id;
        debug_assert!(requests.iter().all(|req| req.id == client));
        let added = requests.len();

        if let Some(existing) = self.by_client.get_mut(&client) {
            requests.append(existing);
            *existing = requests;
        } else {
            self.ready.push_back(client);
            self.by_client.insert(client, requests);
        }
        self.len = self.len.saturating_add(added);
    }

    #[cfg(test)]
    fn pop_front(&mut self) -> Option<DeferredRequest> {
        self.pop_front_if(|_, _| true)
    }

    /// Whether some queued client may run: not waiting on backend work and
    /// not suspended by a SYNC await.
    #[cfg(test)]
    fn has_runnable(&self, pending: &PendingBackendRequests, state: &ServerState) -> bool {
        self.ready
            .iter()
            .any(|client| client_runnable(pending, state, *client))
    }

    fn has_runnable_with_gate(
        &self,
        pending: &PendingBackendRequests,
        state: &ServerState,
        gate: &RandrMutationGate,
        backend: &dyn Backend,
    ) -> bool {
        self.ready.iter().any(|client| {
            if !client_runnable(pending, state, *client) {
                return false;
            }
            self.by_client
                .get(client)
                .and_then(VecDeque::front)
                .is_some_and(|req| gate.request_is_runnable(req, backend))
        })
    }

    fn register_front_gate_heads(&mut self, gate: &mut RandrMutationGate, backend: &dyn Backend) {
        let clients: Vec<_> = self.ready.iter().copied().collect();
        for client in clients {
            if let Some(req) = self
                .by_client
                .get_mut(&client)
                .and_then(VecDeque::front_mut)
            {
                gate.register_request(req, backend);
            }
        }
    }

    #[cfg(test)]
    fn pop_front_unblocked(
        &mut self,
        pending: &PendingBackendRequests,
        state: &ServerState,
    ) -> Option<DeferredRequest> {
        self.pop_front_if(|client, _| client_runnable(pending, state, client))
    }

    fn pop_front_runnable(
        &mut self,
        pending: &PendingBackendRequests,
        state: &ServerState,
        gate: &RandrMutationGate,
        backend: &dyn Backend,
    ) -> Option<DeferredRequest> {
        self.pop_front_if(|client, req| {
            client_runnable(pending, state, client) && gate.request_is_runnable(req, backend)
        })
    }

    fn pop_front_if(
        &mut self,
        mut is_runnable: impl FnMut(yserver_protocol::x11::ClientId, &DeferredRequest) -> bool,
    ) -> Option<DeferredRequest> {
        // Inspect each currently-ready client at most once. Blocked clients
        // retain their position in the ring while other clients keep moving.
        let candidates = self.ready.len();
        for _ in 0..candidates {
            let Some(client) = self.ready.pop_front() else {
                break;
            };
            let Some(front) = self.by_client.get(&client).and_then(VecDeque::front) else {
                self.by_client.remove(&client);
                continue;
            };
            if !is_runnable(client, front) {
                self.ready.push_back(client);
                continue;
            }
            let (request, remains_ready) = {
                let Some(queue) = self.by_client.get_mut(&client) else {
                    continue;
                };
                (queue.pop_front(), !queue.is_empty())
            };
            let Some(request) = request else {
                self.by_client.remove(&client);
                continue;
            };
            self.len = self.len.saturating_sub(1);
            if remains_ready {
                self.ready.push_back(client);
            } else {
                self.by_client.remove(&client);
            }
            return Some(request);
        }
        None
    }

    fn take_gate_ticket(&mut self, ticket: u64) -> Option<DeferredRequest> {
        let client = self.by_client.iter().find_map(|(client, requests)| {
            requests
                .iter()
                .position(|request| request.randr_gate_ticket == Some(ticket))
                .map(|_| *client)
        })?;
        let requests = self.by_client.get_mut(&client)?;
        let index = requests
            .iter()
            .position(|request| request.randr_gate_ticket == Some(ticket))?;
        let request = requests.remove(index)?;
        self.len = self.len.saturating_sub(1);
        if requests.is_empty() {
            self.by_client.remove(&client);
            self.ready.retain(|ready_client| *ready_client != client);
        }
        Some(request)
    }
}

/// A minimal `DeferredRequest` for tests outside this module. The
/// opcode is arbitrary: the reset boundary discards these without ever
/// decoding one.
#[cfg(test)]
pub(crate) fn deferred_request_for_test(id: u32) -> DeferredRequest {
    DeferredRequest {
        id: yserver_protocol::x11::ClientId(id),
        sequence: yserver_protocol::x11::SequenceNumber(1),
        accepted_at: None,
        randr_gate_ticket: None,
        randr_gate_expired: false,
        header: yserver_protocol::x11::RequestHeader {
            opcode: 127,
            data: 0,
            length_units: 1,
        },
        body: Vec::new(),
        attached_fd: None,
    }
}

/// A client's queued requests may be dispatched unless it waits on
/// asynchronous backend work, a SYNC `Await` / `AwaitFence` suspended it, or
/// it is the data connection of an enabled RECORD context (Xorg
/// `IgnoreClient`). Either way its requests keep their order and
/// every other client keeps running.
fn client_runnable(
    pending: &PendingBackendRequests,
    state: &ServerState,
    client: yserver_protocol::x11::ClientId,
) -> bool {
    !pending.client_is_blocked(client)
        && !crate::core_loop::sync_await::client_is_suspended(state, client)
        && !crate::core_loop::record::client_blocks_requests(state, client)
}

fn blocked_by_server_grab(state: &ServerState, req: &DeferredRequest) -> bool {
    state.server_grab_owner.is_some_and(|owner| owner != req.id)
}

/// Restore parked server-grab requests to the fair queue without changing
/// their per-client arrival order.
pub(crate) fn release_server_grab_waiters(
    deferred_requests: &mut FairRequestQueue,
    server_grab_waiters: &mut VecDeque<DeferredRequest>,
    telemetry: &mut LoopTelemetry,
) {
    // A waiter is an older prefix temporarily removed from one client's fair
    // queue while another client owned GrabServer. Restore each prefix ahead
    // of that client's requests which remained queued. Appending here breaks
    // X11's strict per-client order (observed as #59264 dispatched before
    // #59216), causing Xlib/XCB to abort with threads_sequence_lost.
    let mut client_order = Vec::new();
    let mut by_client: HashMap<_, VecDeque<_>> = HashMap::new();
    while let Some(req) = server_grab_waiters.pop_front() {
        telemetry.record_deferred_push(req.id);
        let client = req.id;
        match by_client.entry(client) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().push_back(req);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                client_order.push(client);
                entry.insert(VecDeque::from([req]));
            }
        }
    }
    for client in client_order {
        deferred_requests.prepend_client(
            by_client
                .remove(&client)
                .expect("server-grab waiter client recorded"),
        );
    }
}

fn grant_request_credit(
    state: &ServerState,
    client: yserver_protocol::x11::ClientId,
    bytes: usize,
) {
    if let Some(control) = state
        .clients
        .get(&client.0)
        .and_then(|client| client.reader_control.as_ref())
    {
        let _ = control.send(crate::server::ReaderControl::GrantRequestBytes(bytes));
    }
}

/// Complete one client's disconnect and tell the reset trigger about
/// it.
///
/// Every path in this file that removes a client from `state.clients`
/// funnels through here — a request handler asking for a disconnect, a
/// failed `park_crtc`, an asynchronous CRTC completion, a failed
/// `ClientSetupComplete`, the reader thread's `ClientDisconnected`, a
/// failed outbound drain and the writable-interest reconcile — which is
/// what lets the trigger be an *event* rather than a state check.
pub(super) fn disconnect_with_xi_pending_cleanup(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    xi_config_lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    client: yserver_protocol::x11::ClientId,
) {
    abandon_client_randr_requests(backend, pending, gate, client);
    xi_config_lane
        .queued
        .retain(|queued| queued.request.client != client);
    if xi_config_lane
        .in_flight
        .as_ref()
        .and_then(|in_flight| in_flight.protocol.as_ref())
        .is_some_and(|protocol| protocol.validated.request.client == client)
        && let Some(in_flight) = xi_config_lane.in_flight.as_mut()
    {
        in_flight.protocol = None;
    }
    pending.unblock_xi_config(client);

    crate::core_loop::process_disconnect::process_disconnect(state, backend, client);
    reset_trigger.note_client_departed(state.clients.len());
}

pub(super) fn disconnect_with_pending_cleanup(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    reset_trigger: &mut ResetTrigger,
    client: yserver_protocol::x11::ClientId,
) {
    abandon_client_randr_requests(backend, pending, gate, client);
    crate::core_loop::process_disconnect::process_disconnect(state, backend, client);
    reset_trigger.note_client_departed(state.clients.len());
}

/// Prune every gate waiter for a departed client and detach its reply from an
/// in-flight CRTC publication. A dispatched backend operation keeps the gate
/// until its terminal result; a cancellable operation frees it immediately.
fn abandon_client_randr_requests(
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    client: yserver_protocol::x11::ClientId,
) {
    gate.remove_waiters_for_client(client);
    if let Some(token) = pending.take_client_crtc(client)
        && backend.abandon_crtc_config_requester(token) == RequesterAbandon::Cancelled
    {
        gate.finish_pending(token);
    }
    pending.detach_forced_reprobe_requester(client);
}

pub(super) fn cancel_unsubmitted_xi_configs(
    lane: &mut XiConfigLane,
    pending: &mut PendingBackendRequests,
) {
    lane.queued.clear();
    pending.xi_config_clients.clear();
    if let Some(in_flight) = lane.in_flight.as_mut() {
        in_flight.protocol = None;
    }
}

fn xi_error_for_config(
    request: &XiConfigRequest,
    error: crate::xinput::libinput_props::DeviceConfigError,
) -> PropertyDispatchError {
    use crate::xinput::libinput_props::DeviceConfigError as ConfigError;
    match error {
        ConfigError::Unsupported => PropertyDispatchError::BadMatch,
        ConfigError::Invalid => PropertyDispatchError::BadValue {
            error_value: u32::from(request.format),
        },
        ConfigError::Cancelled => PropertyDispatchError::BadMatch,
        // xf86-input-libinput returns BadMatch when its shared handle is
        // absent (`xf86libinput.c:4392-4409, 4579-4607`).
        ConfigError::SourceGone => PropertyDispatchError::BadMatch,
    }
}

fn emit_xi_config_error(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    request: &XiConfigRequest,
    error: PropertyDispatchError,
    generation: Generation,
    request_wire_bytes: usize,
    current_generation: Generation,
) {
    if generation == current_generation && state.clients.contains_key(&request.client.0) {
        match crate::core_loop::process_request::emit_property_dispatch_error(
            state,
            request.client,
            request.sequence,
            error,
            request.minor_opcode,
        ) {
            Ok(RequestOutcome::Disconnect(client)) => disconnect_with_xi_pending_cleanup(
                state,
                backend,
                pending,
                gate,
                lane,
                reset_trigger,
                client,
            ),
            Ok(RequestOutcome::Handled) => {}
            Ok(
                RequestOutcome::PendingCrtcConfig(_)
                | RequestOutcome::PendingForcedReprobe(_)
                | RequestOutcome::PendingXiConfig(_),
            ) => {
                unreachable!("error emission cannot start backend work")
            }
            Err(err) => warn!("XI config error reply failed: {err}"),
        }
        grant_request_credit(state, request.client, request_wire_bytes);
    }
    pending.unblock_xi_config(request.client);
}

fn apply_confirmed_xi_config(
    state: &mut ServerState,
    input_inventory: &mut InputInventory,
    source: crate::xinput::InputSourceId,
    change: crate::xinput::libinput_props::DeviceConfigChange,
    validated: Option<&ValidatedXiChange>,
) -> Result<(u16, yserver_protocol::x11::AtomId, crate::xinput::PropWhat), PropertyDispatchError> {
    if let Some(info) = state.xi_devices.source_mut(source) {
        info.config.apply_confirmed(change);
    }
    input_inventory.update_config(source, change);
    crate::core_loop::process_request::commit_confirmed_xi_change(state, source, change, validated)
}

fn drive_xi_config_lane(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
) {
    while lane.in_flight.is_none() {
        let Some(queued) = lane.queued.pop_front() else {
            break;
        };
        let client = queued.request.client;
        if queued.generation != current_generation || !state.clients.contains_key(&client.0) {
            pending.unblock_xi_config(client);
            continue;
        }
        let validated =
            match crate::core_loop::process_request::validate_xi_change(state, &queued.request) {
                Ok(validated) => validated,
                Err(error) => {
                    emit_xi_config_error(
                        state,
                        backend,
                        pending,
                        gate,
                        lane,
                        reset_trigger,
                        &queued.request,
                        error,
                        queued.generation,
                        queued.request_wire_bytes,
                        current_generation,
                    );
                    continue;
                }
            };
        let cancel = crate::xinput::libinput_props::DeviceConfigCancelToken::new();
        match backend.start_device_config(validated.source_id, validated.change, cancel.clone()) {
            Ok(crate::xinput::libinput_props::DeviceConfigStart::Applied) => {
                match apply_confirmed_xi_config(
                    state,
                    input_inventory,
                    validated.source_id,
                    validated.change,
                    Some(&validated),
                ) {
                    Ok((facet_id, property, what)) => {
                        let _ = crate::core_loop::process_request::emit_property_change(
                            state, facet_id, property, what,
                        );
                        backend.mark_dirty();
                        pending.unblock_xi_config(client);
                        grant_request_credit(state, client, queued.request_wire_bytes);
                    }
                    Err(error) => emit_xi_config_error(
                        state,
                        backend,
                        pending,
                        gate,
                        lane,
                        reset_trigger,
                        &queued.request,
                        error,
                        queued.generation,
                        queued.request_wire_bytes,
                        current_generation,
                    ),
                }
            }
            Ok(crate::xinput::libinput_props::DeviceConfigStart::Pending(token)) => {
                lane.in_flight = Some(XiConfigInFlight {
                    token,
                    source: validated.source_id,
                    change: validated.change,
                    cancel,
                    protocol: Some(XiConfigCompletion {
                        validated,
                        generation: queued.generation,
                        request_wire_bytes: queued.request_wire_bytes,
                    }),
                });
                break;
            }
            Err(error) => {
                let error = xi_error_for_config(&queued.request, error);
                emit_xi_config_error(
                    state,
                    backend,
                    pending,
                    gate,
                    lane,
                    reset_trigger,
                    &queued.request,
                    error,
                    queued.generation,
                    queued.request_wire_bytes,
                    current_generation,
                );
            }
        }
    }
}

/// Accept the owned request outcome produced by XI1/XI2 request parsing.
/// This is shared by the main request dispatcher and focused tests so the
/// production lane performs generation stamping, disabled-source rejection,
/// client blocking, dequeue-time validation, backend start, and commit.
pub(super) fn route_pending_xi_config(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    request: XiConfigRequest,
    request_wire_bytes: usize,
    current_generation: Generation,
) {
    let client = request.client;
    let source_disabled = !state
        .xi_devices
        .source_has_enabled_facet(request.expected_source);
    if lane.reject_unsubmitted_for_vt_release || source_disabled {
        let error = crate::core_loop::process_request::validate_xi_change(state, &request)
            .err()
            .unwrap_or(PropertyDispatchError::BadMatch);
        emit_xi_config_error(
            state,
            backend,
            pending,
            gate,
            lane,
            reset_trigger,
            &request,
            error,
            current_generation,
            request_wire_bytes,
            current_generation,
        );
    } else if !pending.block_xi_config(client) {
        log::error!(
            "client {} entered XI config lane while already blocked",
            client.0
        );
        disconnect_with_xi_pending_cleanup(
            state,
            backend,
            pending,
            gate,
            lane,
            reset_trigger,
            client,
        );
    } else {
        lane.queued.push_back(QueuedXiConfig {
            request,
            generation: current_generation,
            request_wire_bytes,
        });
        drive_xi_config_lane(
            state,
            backend,
            input_inventory,
            pending,
            gate,
            lane,
            reset_trigger,
            current_generation,
        );
    }
}

pub(super) fn cancel_queued_xi_configs_for_source(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    source: crate::xinput::InputSourceId,
    current_generation: Generation,
) {
    let mut retained = VecDeque::new();
    while let Some(queued) = lane.queued.pop_front() {
        if queued.request.expected_source != source {
            retained.push_back(queued);
            continue;
        }
        let error = crate::core_loop::process_request::validate_xi_change(state, &queued.request)
            .err()
            .unwrap_or_else(|| {
                if state.xi_devices.source(source).is_some() {
                    PropertyDispatchError::BadMatch
                } else {
                    PropertyDispatchError::BadDevice {
                        deviceid: queued.request.deviceid,
                    }
                }
            });
        emit_xi_config_error(
            state,
            backend,
            pending,
            gate,
            lane,
            reset_trigger,
            &queued.request,
            error,
            queued.generation,
            queued.request_wire_bytes,
            current_generation,
        );
    }
    lane.queued = retained;
}

/// Fail config requests still queued in the core lane when VT release starts.
/// They have not reached an input-thread handle, so they cannot be committed;
/// the in-flight operation is completed by the input pause barrier instead.
fn fail_unsubmitted_xi_configs_for_vt_release(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
) {
    while let Some(queued) = lane.queued.pop_front() {
        let error = crate::core_loop::process_request::validate_xi_change(state, &queued.request)
            .err()
            .unwrap_or(PropertyDispatchError::BadMatch);
        emit_xi_config_error(
            state,
            backend,
            pending,
            gate,
            lane,
            reset_trigger,
            &queued.request,
            error,
            queued.generation,
            queued.request_wire_bytes,
            current_generation,
        );
    }
    lane.reject_unsubmitted_for_vt_release = true;
}

/// Cancel the one submitted XI config write if the input thread cannot
/// confirm it before VT release finishes. A later Applied result is retained
/// for reconciliation, while the client's BadMatch remains final.
fn fail_in_flight_xi_config_for_vt_release(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
) {
    let Some(in_flight) = lane.in_flight.take() else {
        return;
    };
    in_flight.cancel.cancel();
    let protocol = in_flight.protocol;
    lane.timed_out.insert(
        (in_flight.token, in_flight.source),
        TimedOutXiConfig {
            change: in_flight.change,
        },
    );
    if let Some(protocol) = protocol {
        emit_xi_config_error(
            state,
            backend,
            pending,
            gate,
            lane,
            reset_trigger,
            &protocol.validated.request,
            PropertyDispatchError::BadMatch,
            protocol.generation,
            protocol.request_wire_bytes,
            current_generation,
        );
    }
}

/// Dispatch the production `Message::VtRelease` lifecycle callback. Keep the
/// process-lifetime source inventory and backend device facets in the same
/// unavailable boundary before the backend releases DRM master. The callback
/// drains already-submitted config results after the input thread's FIFO pause
/// and before KMS performs the yielding operations.
/// Xorg's `ProcXIChangeProperty` runs `change_property` before returning
/// (`Xi/xiproperty.c:1156-1158`), and its VT handler calls `DisableDevice`
/// after processing held keys (`xf86Events.c:302-313`).
pub fn dispatch_vt_release(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    before_yield: impl FnOnce(&mut ServerState, &mut dyn Backend, &mut InputInventory, bool),
) {
    if backend.vt_switching_armed() {
        input_inventory.suspend_all();
        let pause_barrier_queued = backend.begin_vt_release();
        before_yield(state, backend, input_inventory, pause_barrier_queued);
        backend.finish_vt_release(state, input_inventory);
    }
}

/// Dispatch the production `Message::VtAcquire` lifecycle callback.
pub fn dispatch_vt_acquire(state: &mut ServerState, backend: &mut dyn Backend) {
    if backend.vt_switching_armed() {
        backend.on_vt_acquire(state);
    }
}

/// Dispatch one host input event through the same lifecycle and XI-config
/// cancellation path used by `run_core`.
pub(super) fn dispatch_host_input(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    xi_config_lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    ev: HostInputEvent,
    current_generation: Generation,
) {
    // Process-lifetime bookkeeping: maintain `InputInventory` regardless of
    // generation (it always dispatches, see above) so a device lifecycle
    // transition is never missed, even mid-reset once resets exist. Nothing
    // consumes the inventory yet — purely additive.
    let config_source_lifecycle = match &ev {
        HostInputEvent::DeviceAdded(info) => {
            input_inventory.add(info.clone());
            None
        }
        HostInputEvent::DeviceSuspended { source_id, .. } => {
            input_inventory.suspend(*source_id);
            Some(*source_id)
        }
        HostInputEvent::DeviceResumed(info) => {
            input_inventory.resume(info.clone());
            None
        }
        HostInputEvent::DeviceRemoved { source_id, .. } => {
            input_inventory.remove(*source_id);
            Some(*source_id)
        }
        _ => None,
    };
    handle_host_input(state, backend, ev);
    if let Some(source_id) = config_source_lifecycle {
        cancel_queued_xi_configs_for_source(
            state,
            backend,
            pending,
            gate,
            xi_config_lane,
            reset_trigger,
            source_id,
            current_generation,
        );
    }
    backend.mark_dirty();
}

fn finish_xi_config_result(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    token: crate::xinput::libinput_props::DeviceConfigToken,
    source: crate::xinput::InputSourceId,
    result: Result<(), crate::xinput::libinput_props::DeviceConfigError>,
    current_generation: Generation,
) {
    let matches_submitted = lane
        .in_flight
        .as_ref()
        .is_some_and(|in_flight| in_flight.token == token && in_flight.source == source);
    if !matches_submitted {
        if let Some(timed_out) = lane.timed_out.remove(&(token, source)) {
            if result.is_ok() {
                match apply_confirmed_xi_config(
                    state,
                    input_inventory,
                    source,
                    timed_out.change,
                    None,
                ) {
                    Ok((facet_id, property, what)) => {
                        let _ = crate::core_loop::process_request::emit_property_change(
                            state, facet_id, property, what,
                        );
                        backend.mark_dirty();
                    }
                    Err(error) => {
                        warn!(
                            "late applied input config could not be reconciled token={} source={}: {error:?}",
                            token.0, source.0
                        );
                    }
                }
            }
            drive_xi_config_lane(
                state,
                backend,
                input_inventory,
                pending,
                gate,
                lane,
                reset_trigger,
                current_generation,
            );
            return;
        }
        warn!(
            "discarding stale input config result token={} source={}",
            token.0, source.0
        );
        return;
    }
    let in_flight = lane
        .in_flight
        .take()
        .expect("matching submitted config exists");
    let protocol = in_flight.protocol.filter(|protocol| {
        protocol.generation == current_generation
            && state
                .clients
                .contains_key(&protocol.validated.request.client.0)
    });
    match result {
        Ok(()) => {
            // Xorg stores the property only after all check-only handlers
            // succeed (`Xi/xiproperty.c:759-801`); commit only the confirmed
            // libinput result here.
            let validated = protocol.as_ref().map(|protocol| &protocol.validated);
            match apply_confirmed_xi_config(
                state,
                input_inventory,
                in_flight.source,
                in_flight.change,
                validated,
            ) {
                Ok((facet_id, property, what)) => {
                    let _ = crate::core_loop::process_request::emit_property_change(
                        state, facet_id, property, what,
                    );
                    backend.mark_dirty();
                    if let Some(protocol) = protocol.as_ref() {
                        let client = protocol.validated.request.client;
                        pending.unblock_xi_config(client);
                        grant_request_credit(state, client, protocol.request_wire_bytes);
                    }
                }
                Err(error) => {
                    if let Some(protocol) = protocol.as_ref() {
                        let request = &protocol.validated.request;
                        emit_xi_config_error(
                            state,
                            backend,
                            pending,
                            gate,
                            lane,
                            reset_trigger,
                            request,
                            error,
                            protocol.generation,
                            protocol.request_wire_bytes,
                            current_generation,
                        );
                    }
                }
            }
        }
        Err(error) => {
            if let Some(protocol) = protocol.as_ref() {
                let request = &protocol.validated.request;
                emit_xi_config_error(
                    state,
                    backend,
                    pending,
                    gate,
                    lane,
                    reset_trigger,
                    request,
                    xi_error_for_config(request, error),
                    protocol.generation,
                    protocol.request_wire_bytes,
                    current_generation,
                );
            }
        }
    }
    drive_xi_config_lane(
        state,
        backend,
        input_inventory,
        pending,
        gate,
        lane,
        reset_trigger,
        current_generation,
    );
}

/// Dispatch the process-lifetime completion message through the same runner
/// path used by `run_core`. Keeping the `Message` boundary here lets tests
/// exercise token/source matching and late-generation commits without
/// substituting a completion-only test helper.
pub(super) fn dispatch_device_config_result(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    message: Message,
    current_generation: Generation,
) {
    let Message::DeviceConfigResult {
        token,
        source,
        result,
    } = message
    else {
        unreachable!("device config dispatcher received a different message")
    };
    finish_xi_config_result(
        state,
        backend,
        input_inventory,
        pending,
        gate,
        lane,
        reset_trigger,
        token,
        source,
        result,
        current_generation,
    );
}

pub(crate) fn cancel_all_pending_backend_requests(
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
) {
    for token in pending.take_all_crtc_tokens() {
        backend.cancel_crtc_config(token);
        backend.cancel_forced_reprobe(token);
    }
}

fn process_one_request(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    telemetry: &mut LoopTelemetry,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    xi_config_lane: &mut XiConfigLane,

    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
    requests_this_iter: &mut u32,
    request_budget: &mut usize,
    gate_mutation: bool,
    req: DeferredRequest,
) {
    let req_opcode = req.header.opcode;
    let req_data = req.header.data;
    let req_wire_bytes = usize::try_from(req.header.length_units)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
        .max(4);
    let req_client = req.id;
    let req_age = req.accepted_at.map(|accepted_at| accepted_at.elapsed());
    let req_start = if telemetry.enabled {
        Some(Instant::now())
    } else {
        None
    };
    let clients_before: Vec<_> = state.clients.keys().copied().collect();
    let outcome = if req.randr_gate_expired {
        super::process_request::fail_expired_crtc_config(
            state,
            req.id,
            req.sequence,
            req.header,
            &req.body,
        )
        .unwrap_or_else(|err| {
            log::warn!(
                "expired RRSetCrtcConfig reply failed (client {}): {err}",
                req.id.0
            );
            RequestOutcome::Handled
        })
    } else {
        process_request_inline(
            state,
            backend,
            req.id,
            req.sequence,
            req.header,
            &req.body,
            req.attached_fd,
        )
    };
    // The one client removal that does NOT come back as
    // `RequestOutcome::Disconnect`: `KillClient` naming a resource owned
    // by a *different* client calls `process_disconnect` inline
    // (`process_request.rs`, "Force-disconnect the other client"). That
    // is still a departure and the trigger has to hear about it. Gated
    // on the count actually dropping, so this stays an event — a
    // request that removes nobody reports nothing.
    //
    // Today it can never be the departure that drains the session (the
    // killer is still connected, so the set is non-empty), but nothing
    // in the handler guarantees that, and an unreported departure is a
    // trigger that silently never fires again.
    for departed in clients_before
        .into_iter()
        .filter(|client| !state.clients.contains_key(client))
    {
        let departed = yserver_protocol::x11::ClientId(departed);
        abandon_client_randr_requests(backend, pending, gate, departed);
        xi_config_lane
            .queued
            .retain(|queued| queued.request.client != departed);
        pending.unblock_xi_config(departed);
        reset_trigger.note_client_departed(state.clients.len());
    }
    if let Some(start) = req_start {
        telemetry.record_request(
            req_client,
            req_opcode,
            req_data,
            start.elapsed(),
            req_age.unwrap_or_default(),
        );
    }
    *requests_this_iter += 1;
    *request_budget = request_budget.saturating_sub(1);
    let mut pending_gate_mutation = false;
    match outcome {
        RequestOutcome::Handled => grant_request_credit(state, req_client, req_wire_bytes),
        RequestOutcome::Disconnect(disc_id) => disconnect_with_xi_pending_cleanup(
            state,
            backend,
            pending,
            gate,
            xi_config_lane,
            reset_trigger,
            disc_id,
        ),
        RequestOutcome::PendingCrtcConfig(continuation) => {
            let token = continuation.token;
            let parked = ParkedCrtcConfig {
                token,
                client_id: req_client,
                sequence: req.sequence,
                reply: continuation.reply,
                request_wire_bytes: req_wire_bytes,
            };
            if let Err(reason) = pending.park_crtc(parked) {
                log::error!(
                    "cannot park asynchronous RRSetCrtcConfig for client {} token {}: {reason}",
                    req_client.0,
                    token.0,
                );
                if !pending.crtc_by_token.contains_key(&token) {
                    backend.cancel_crtc_config(token);
                }
                disconnect_with_xi_pending_cleanup(
                    state,
                    backend,
                    pending,
                    gate,
                    xi_config_lane,
                    reset_trigger,
                    req_client,
                );
            } else if gate_mutation {
                gate.mark_pending(token, continuation.publication);
                pending_gate_mutation = true;
            }
        }
        RequestOutcome::PendingForcedReprobe(continuation) => {
            let token = continuation.token;
            if let Err(reason) =
                pending.park_forced_reprobe(continuation, req_client, req.sequence, req_wire_bytes)
            {
                log::error!(
                    "cannot park asynchronous RRGetScreenResources for client {} token {}: {reason}",
                    req_client.0,
                    token.0,
                );
                if !pending.forced_reprobe_by_token.contains_key(&token)
                    && !pending.crtc_by_token.contains_key(&token)
                {
                    backend.cancel_forced_reprobe(token);
                }
                disconnect_with_xi_pending_cleanup(
                    state,
                    backend,
                    pending,
                    gate,
                    xi_config_lane,
                    reset_trigger,
                    req_client,
                );
            } else if gate_mutation {
                gate.mark_forced_reprobe_pending(token);
                pending_gate_mutation = true;
            } else {
                log::error!(
                    "backend returned pending forced reprobe token {} without a gate turn",
                    token.0,
                );
                let _ = pending.take_forced_reprobe_reply(token);
                backend.cancel_forced_reprobe(token);
                disconnect_with_xi_pending_cleanup(
                    state,
                    backend,
                    pending,
                    gate,
                    xi_config_lane,
                    reset_trigger,
                    req_client,
                );
            }
        }
        RequestOutcome::PendingXiConfig(request) => route_pending_xi_config(
            state,
            backend,
            input_inventory,
            pending,
            gate,
            xi_config_lane,
            reset_trigger,
            request,
            req_wire_bytes,
            current_generation,
        ),
    }
    if gate_mutation && !pending_gate_mutation {
        gate.finish_synchronous();
    }
}

fn drain_pending_requests(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    telemetry: &mut LoopTelemetry,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    xi_config_lane: &mut XiConfigLane,

    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
    deferred_requests: &mut FairRequestQueue,
    server_grab_waiters: &mut VecDeque<DeferredRequest>,
    requests_this_iter: &mut u32,
    request_budget: &mut usize,
    drain_start: Instant,
) {
    drain_expired_randr_gate_waiters(
        state,
        backend,
        input_inventory,
        telemetry,
        pending,
        gate,
        xi_config_lane,
        reset_trigger,
        current_generation,
        deferred_requests,
        server_grab_waiters,
        requests_this_iter,
        request_budget,
    );
    while !budget_exhausted(*request_budget, drain_start.elapsed()) {
        deferred_requests.register_front_gate_heads(gate, backend);
        let Some(req) = deferred_requests.pop_front_runnable(pending, state, gate, backend) else {
            break;
        };
        telemetry.record_deferred_pop(req.id);
        if blocked_by_server_grab(state, &req) {
            server_grab_waiters.push_back(req);
            continue;
        }
        let gate_mutation = gate.admit(&req, backend);
        let is_forced_reprobe = req.header.opcode == RANDR_MAJOR_OPCODE
            && classify_randr_minor(req.header.data) == RandrRequestClass::ForcedReprobe;
        process_one_request(
            state,
            backend,
            input_inventory,
            telemetry,
            pending,
            gate,
            xi_config_lane,
            reset_trigger,
            current_generation,
            requests_this_iter,
            request_budget,
            gate_mutation,
            req,
        );
        // A Legacy mutation and Legacy's forced reprobe run on this thread,
        // so the loop cannot service deadlines during them. Owner forced
        // reprobes return Pending and let the loop continue through the
        // worker's two-second probe deadline.
        if gate_mutation || is_forced_reprobe {
            drain_expired_randr_gate_waiters(
                state,
                backend,
                input_inventory,
                telemetry,
                pending,
                gate,
                xi_config_lane,
                reset_trigger,
                current_generation,
                deferred_requests,
                server_grab_waiters,
                requests_this_iter,
                request_budget,
            );
        }
        if state.server_grab_owner.is_none() {
            release_server_grab_waiters(deferred_requests, server_grab_waiters, telemetry);
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "deadline servicing shares the core request-drain state and must reply inline"
)]
fn drain_expired_randr_gate_waiters(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    telemetry: &mut LoopTelemetry,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    xi_config_lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
    deferred_requests: &mut FairRequestQueue,
    server_grab_waiters: &mut VecDeque<DeferredRequest>,
    requests_this_iter: &mut u32,
    request_budget: &mut usize,
) {
    for waiter in gate.take_expired_waiters(Instant::now()) {
        let queued_request = deferred_requests
            .take_gate_ticket(waiter.ticket)
            .inspect(|request| {
                telemetry.record_deferred_pop(request.id);
            });
        let Some(mut request) = queued_request.or_else(|| {
            server_grab_waiters
                .iter()
                .position(|request| request.randr_gate_ticket == Some(waiter.ticket))
                .and_then(|index| server_grab_waiters.remove(index))
        }) else {
            continue;
        };
        request.randr_gate_expired = true;
        process_one_request(
            state,
            backend,
            input_inventory,
            telemetry,
            pending,
            gate,
            xi_config_lane,
            reset_trigger,
            current_generation,
            requests_this_iter,
            request_budget,
            false,
            request,
        );
    }
}

fn drain_vt_release_requests(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    input_inventory: &mut InputInventory,
    telemetry: &mut LoopTelemetry,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    lane: &mut XiConfigLane,
    reset_trigger: &mut ResetTrigger,
    current_generation: Generation,
    deferred_requests: &mut FairRequestQueue,
    server_grab_waiters: &mut VecDeque<DeferredRequest>,
) {
    let mut requests_this_iter = 0;
    let mut request_budget = usize::MAX;
    drain_pending_requests(
        state,
        backend,
        input_inventory,
        telemetry,
        pending,
        gate,
        lane,
        reset_trigger,
        current_generation,
        deferred_requests,
        server_grab_waiters,
        &mut requests_this_iter,
        &mut request_budget,
        Instant::now(),
    );
}

/// Process one X protocol request and run its post-handler bookkeeping
/// (mark_dirty + disconnect-on-error). Factored so the two drain paths
/// in `run_core` (the deferred queue at the top of each iteration and
/// the channel drain inside `NOTIFY_TOKEN`) share identical semantics.
///
fn process_request_inline(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    id: yserver_protocol::x11::ClientId,
    sequence: yserver_protocol::x11::SequenceNumber,
    header: yserver_protocol::x11::RequestHeader,
    body: &[u8],
    attached_fd: Option<OwnedFd>,
) -> RequestOutcome {
    // Half-closed-socket / post-disconnect guard. The `Message::Request`
    // The reader/channel/fair-queue path preserves per-client arrival order.
    // When a client crashes (e.g.
    // mate-appearance-properties cratering with the keyring locked) and
    // the client_reader thread enqueues a burst of bogus requests
    // before/around the EOF, the main thread can still be draining those
    // queued Requests *after* `process_disconnect` removed the client
    // from `state.clients`. Several handlers (CreatePixmap, CreateGC,
    // CreateWindow, etc. — eight sites at process_request.rs) read
    // `state.clients.get(client_id).expect("client registered")` to
    // validate the request's resource XID against the client's
    // allocation range, and panic the whole server when the lookup misses.
    //
    // Without this guard we observed a session crash on 2026-05-26 in
    // the adapta-nokto investigation: 240 BadIDChoice warnings for
    // CreatePixmap pid=0xffffffff, then panic at process_request.rs:11686
    // when state.clients.remove(client_51) finally won the race.
    //
    // Drop silently: the client is gone, no reply / error can be
    // delivered to anyone, and the work would be a no-op. Tests that
    // exercise individual handlers via `process_request` directly are
    // unaffected (they don't go through this dispatcher).
    if !state.clients.contains_key(&id.0) {
        log::debug!(
            "process_request_inline: dropping request from already-disconnected client {} \
             (opcode={}, seq={})",
            id.0,
            header.opcode,
            sequence.0,
        );
        return RequestOutcome::Handled;
    }
    let outcome = match process_request(state, backend, id, sequence, header, body, attached_fd) {
        Ok(out) => out,
        Err(err) => {
            // A request handler errored — usually a backend-side
            // limit (e.g., "too many points"). Log + continue rather
            // than killing the server. Pre-existing bug: bogus client
            // requests shouldn't be fatal.
            log::warn!(
                "request handler error (client {} opcode {}): {err}",
                id.0,
                header.opcode,
            );
            RequestOutcome::Handled
        }
    };
    // Pending work has not committed any visible result yet. Its completion
    // path performs this bookkeeping exactly once when the result is applied.
    if !matches!(
        &outcome,
        RequestOutcome::PendingCrtcConfig(_)
            | RequestOutcome::PendingForcedReprobe(_)
            | RequestOutcome::PendingXiConfig(_),
    ) {
        if std::mem::take(&mut state.damage_notify_flush_pending) {
            backend.flush_before_damage_notify();
        }
        backend.mark_dirty();
    }
    // A request that changed the displayed cursor (DefineCursor, a grab,
    // XFIXES ChangeCursor, a map under the pointer) reports it before the
    // client's next request runs, as Xorg does from DisplayCursor.
    crate::core_loop::process_request::emit_xfixes_cursor_notify(state, backend);
    outcome
}

/// Resume every asynchronous CRTC request whose backend result is ready.
/// The backend is always finished even after the requester leaves; publication
/// and reply delivery are selected independently below.
#[cfg(test)]
pub(crate) fn drain_ready_crtc_configs(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    reset_trigger: &mut ResetTrigger,
) {
    drain_ready_crtc_configs_with_gate(
        state,
        backend,
        pending,
        &mut RandrMutationGate::default(),
        reset_trigger,
    );
}

#[cfg(test)]
pub(crate) fn drain_ready_crtc_configs_with_gate(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    reset_trigger: &mut ResetTrigger,
) {
    drain_ready_crtc_configs_with_gate_policy(state, backend, pending, gate, reset_trigger, true);
}

fn drain_ready_crtc_configs_with_gate_policy(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    pending: &mut PendingBackendRequests,
    gate: &mut RandrMutationGate,
    reset_trigger: &mut ResetTrigger,
    publish_old_generation: bool,
) {
    drain_requesterless_publications(state, backend, gate, publish_old_generation, None);
    for token in backend.drain_ready_crtc_configs() {
        if let Some(parked) = pending.take_forced_reprobe_reply(token) {
            if !gate.owns_pending_token(token) {
                backend.cancel_forced_reprobe(token);
                gate.finish_pending(token);
                drain_requesterless_publications(
                    state,
                    backend,
                    gate,
                    publish_old_generation,
                    None,
                );
                continue;
            }
            let result = if publish_old_generation {
                backend.finish_forced_reprobe(token, state)
            } else {
                backend.cancel_forced_reprobe(token);
                Ok(crate::backend::ForcedReprobeResult::Expired)
            };
            if publish_old_generation {
                if std::mem::take(&mut state.damage_notify_flush_pending) {
                    backend.flush_before_damage_notify();
                }
                backend.mark_dirty();
            }
            let outcome = parked
                .client_id
                .map_or(Ok(RequestOutcome::Handled), |client_id| {
                    complete_forced_reprobe_reply(
                        state,
                        client_id,
                        parked.sequence,
                        parked.byte_order,
                        result,
                    )
                });
            gate.finish_pending(token);
            // A topology episode requested by a worker while this forced
            // reprobe owned the turn is granted only after its reply.
            drain_requesterless_publications(state, backend, gate, publish_old_generation, None);
            match outcome {
                Ok(RequestOutcome::Disconnect(client)) => {
                    disconnect_with_pending_cleanup(
                        state,
                        backend,
                        pending,
                        gate,
                        reset_trigger,
                        client,
                    );
                }
                Ok(RequestOutcome::Handled) => {
                    if let Some(client_id) = parked.client_id {
                        grant_request_credit(state, client_id, parked.request_wire_bytes);
                    }
                }
                Ok(
                    RequestOutcome::PendingCrtcConfig(_)
                    | RequestOutcome::PendingForcedReprobe(_)
                    | RequestOutcome::PendingXiConfig(_),
                ) => {
                    unreachable!("forced reprobe completion cannot park a second backend request")
                }
                Err(error) => log::warn!(
                    "RRGetScreenResources completion reply failed (token {}): {error}",
                    token.0
                ),
            }
            continue;
        }
        let parked = pending.take_crtc_reply(token);
        let Some(publication) = gate.take_publication(token) else {
            // Cancellation may race a worker completion. A token without a
            // gate-owned publication was cancelled or otherwise superseded.
            backend.cancel_crtc_config(token);
            gate.finish_pending(token);
            drain_requesterless_publications(state, backend, gate, publish_old_generation, None);
            continue;
        };

        let install_capable = backend.crtc_config_install_capable(token);
        if !publish_old_generation && !install_capable {
            // A generation boundary can discard work that was never
            // dispatched. Its completion cannot change backend state, so
            // cancel it before the reset snapshot instead of advancing it.
            backend.cancel_crtc_config(token);
            gate.finish_pending(token);
            drain_requesterless_publications(state, backend, gate, publish_old_generation, None);
            continue;
        }

        let result = backend.finish_crtc_config(token);
        let reply_kind = publication.reply_kind;
        let set_time = publication.set_time;
        let status = if publish_old_generation {
            Some(publish_crtc_config(state, backend, publication, result))
        } else {
            // The backend result is terminal, but this generation is already
            // committed to reset/termination. The new generation snapshots
            // the installed topology after this gate is released.
            let _ = (publication, result);
            None
        };

        if publish_old_generation {
            if std::mem::take(&mut state.damage_notify_flush_pending) {
                backend.flush_before_damage_notify();
            }
            backend.mark_dirty();
        }
        gate.finish_pending(token);
        // A publication queued behind dispatched work is released immediately
        // after that work's state publication and before its client reply or
        // any waiting request can be admitted.
        drain_requesterless_publications(state, backend, gate, publish_old_generation, None);
        let outcome = if let Some(status) = status {
            parked.map_or(
                RequestOutcome::Handled,
                |reply| match complete_crtc_config_reply(
                    state,
                    reply.client_id,
                    reply.sequence,
                    reply.reply,
                    reply_kind,
                    set_time,
                    status,
                ) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        log::warn!(
                            "RANDR CRTC completion reply failed (client {} token {}): {err}",
                            reply.client_id.0,
                            token.0,
                        );
                        RequestOutcome::Handled
                    }
                },
            )
        } else {
            RequestOutcome::Handled
        };
        match outcome {
            RequestOutcome::Disconnect(client) => {
                disconnect_with_pending_cleanup(
                    state,
                    backend,
                    pending,
                    gate,
                    reset_trigger,
                    client,
                );
            }
            RequestOutcome::Handled => {
                if let Some(reply) = parked {
                    grant_request_credit(state, reply.client_id, reply.request_wire_bytes);
                }
            }
            RequestOutcome::PendingCrtcConfig(_)
            | RequestOutcome::PendingForcedReprobe(_)
            | RequestOutcome::PendingXiConfig(_) => {
                unreachable!("CRTC completion cannot start a second asynchronous request")
            }
        }
    }
}

/// What the core-entry test driver actually consumed from a backend.
///
/// This signature lets integration tests prove the delivery path without
/// reaching into a backend's private publication queue after the core drains
/// it. Ordinary state publications remain represented by the resulting
/// server state; this trace records event multiplicity and urgent resource
/// identities.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEntryDelivery {
    UrgentWithdrawal {
        output_ids: Vec<u32>,
        crtc_ids: Vec<u32>,
    },
    TopologyEpisodeBegin(u64),
    TopologyEpisodeEnd {
        episode_id: u64,
        published: bool,
    },
}

fn drain_requesterless_publications(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    gate: &mut RandrMutationGate,
    publish_old_generation: bool,
    mut delivery_trace: Option<&mut Vec<CoreEntryDelivery>>,
) {
    let urgent_publications = backend.drain_urgent_requesterless_publications();
    let episode_events = backend.drain_topology_episode_events();
    if publish_old_generation {
        for publication in urgent_publications {
            if let Some(trace) = delivery_trace.as_mut()
                && let Some((output_ids, crtc_ids)) = publication.urgent_withdrawal_ids()
            {
                trace.push(CoreEntryDelivery::UrgentWithdrawal {
                    output_ids: output_ids.to_vec(),
                    crtc_ids: crtc_ids.to_vec(),
                });
            }
            publish_requesterless_publication(state, backend, publication);
        }
        for event in episode_events {
            if let Some(trace) = delivery_trace.as_mut() {
                match &event {
                    crate::backend::TopologyEpisodeEvent::EpisodeBegin(episode_id) => {
                        trace.push(CoreEntryDelivery::TopologyEpisodeBegin(*episode_id));
                    }
                    crate::backend::TopologyEpisodeEvent::EpisodeEnd(episode_id, publication) => {
                        trace.push(CoreEntryDelivery::TopologyEpisodeEnd {
                            episode_id: *episode_id,
                            published: publication.is_some(),
                        });
                    }
                }
            }
            match event {
                crate::backend::TopologyEpisodeEvent::EpisodeBegin(episode_id) => {
                    gate.request_topology_episode(episode_id);
                }
                crate::backend::TopologyEpisodeEvent::EpisodeEnd(episode_id, publication) => {
                    if gate.topology_episode == Some(episode_id) {
                        if let Some(publication) = publication {
                            publish_requesterless_publication(state, backend, publication);
                        }
                        gate.finish_topology_episode(episode_id);
                    } else if !gate.withdraw_topology_episode_request(episode_id) {
                        // A stale end has no authority to release another
                        // episode's turn or publish an ungranted change.
                        log::error!(
                            "topology episode {episode_id} ended without an active or requested turn"
                        );
                    } else {
                        if publication.is_some() {
                            log::error!(
                                "ungranted topology episode {episode_id} carried a publication"
                            );
                        }
                    }
                }
            }
        }
    }
    gate.grant_requested_topology_episode(state, backend);

    let publications = backend.drain_requesterless_publications();
    if !publish_old_generation {
        // Requester-less events belong to the generation whose backend state
        // produced them. A reset/termination takes its fresh snapshot from the
        // backend after the terminal result and must not publish old events.
        gate.requesterless_publications.clear();
        return;
    }
    gate.queue_requesterless_publications(publications);
    if gate.holds_requesterless_turn(backend) {
        return;
    }

    for publication in gate.take_requesterless_publications() {
        publish_requesterless_publication(state, backend, publication);
    }
}

fn publish_requesterless_publication(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    publication: RequesterlessPublication,
) {
    let output_bbox_before = enabled_output_bbox(state);
    publication.publish(state, output_bbox_before);
    if std::mem::take(&mut state.damage_notify_flush_pending) {
        backend.flush_before_damage_notify();
    }
    backend.mark_dirty();
}

/// X11 default auto-repeat initial delay before the first synthetic
/// KeyPress fires. Matches xset's `-r` defaults; not yet pulled from
/// the XKB Controls block.
const REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(660);

/// X11 default auto-repeat period (25 Hz = 40 ms between synthetic
/// KeyPress events while a key is held).
const REPEAT_PERIOD: Duration = Duration::from_millis(40);

/// Run the core loop until `Message::Shutdown` is observed.
///
/// `poll` must already have its waker registered against `NOTIFY_TOKEN`
/// (see `core_loop::channel`). Additional fds (listener, client
/// writers, drm, libinput, signalfd, host-X11) get registered by their
/// respective phase tasks before this function takes over the thread.
///
/// `state` and `backend` are owned by the core loop for the duration
/// of the run — the whole point of the single-threaded refactor is
/// that only this thread can mutate them.
pub fn run_core(
    poll: Poll,
    rx: CoreReceiver,
    sender: CoreSender,
    state: &mut ServerState,
    backend: &mut dyn Backend,
    listeners: impl IntoIterator<Item = Listener>,
    client_id_allocator: &ClientIdAllocator,
    auth: Arc<AuthState>,
    reset_policy: ResetPolicy,
    xdmcp: Option<XdmcpService>,
) -> io::Result<()> {
    let mut input_inventory = InputInventory::new();
    run_core_with_inventory(
        poll,
        rx,
        sender,
        state,
        backend,
        listeners,
        client_id_allocator,
        auth,
        reset_policy,
        xdmcp,
        &mut input_inventory,
    )
}

fn run_core_with_inventory(
    mut poll: Poll,
    rx: CoreReceiver,
    sender: CoreSender,
    state: &mut ServerState,
    backend: &mut dyn Backend,
    listeners: impl IntoIterator<Item = Listener>,
    client_id_allocator: &ClientIdAllocator,
    auth: Arc<AuthState>,
    reset_policy: ResetPolicy,
    xdmcp: Option<XdmcpService>,
    input_inventory: &mut InputInventory,
) -> io::Result<()> {
    let setup_registry = setup_thread::make_registry();
    // The generation counter is shared with every `CoreSender`; the
    // receiver is the loop's sole handle to it, and the reset boundary
    // is the only thing that bumps it.
    let generations = rx.generation_counter();
    // The armed trigger (server-reset design, "The trigger must be
    // armed, not inferred"). False until a client becomes established,
    // and false again immediately after every reset.
    let mut reset_trigger = ResetTrigger::new(reset_policy);
    let listeners: Vec<_> = listeners
        .into_iter()
        .enumerate()
        .map(|(index, listener)| {
            listener.set_nonblocking(true)?;
            let raw = listener.as_raw_fd();
            let token = listener_token(index)
                .ok_or_else(|| io::Error::other("too many client listeners"))?;
            poll.registry()
                .register(&mut SourceFd(&raw), token, Interest::READABLE)?;
            Ok(listener)
        })
        .collect::<io::Result<_>>()?;
    let mut listener_readiness = ListenerReadiness::new(listeners.len());

    // XDMCP: one UDP socket in this same poll set, and the first query.
    // `None` unless argv named `-query`/`-broadcast`/`-indirect`, in which
    // case nothing below this point does anything at all (invariant 4).
    let mut xdmcp = xdmcp;
    if let Some(service) = xdmcp.as_mut() {
        service.register(poll.registry())?;
        // `XdmcpInit` (`xdmcp.c:600`): the query goes out before the first
        // poll, so a manager on the same host can answer within the first
        // iteration.
        service.start(&auth, rx.current_generation());
    }

    // E3: register backend-owned fds with the core poller. KMS returns
    // `Drm` only after `take_input_ctx`; the libinput context, when
    // present, is owned by the dedicated libinput thread (E2/E4) so
    // the core never sees the libinput fd in production. The Libinput
    // arm is registered defensively in case a backend variant chooses
    // to skip the dedicated thread and run libinput on the core poll.
    let mut backend_poll_sources: Vec<_> = backend
        .poll_fds()
        .into_iter()
        .map(|(fd, kind)| BackendPollSource { fd, kind })
        .collect();
    register_backend_poll_sources(poll.registry(), &backend_poll_sources)?;
    let mut backend_poll_source_generation = backend.poll_source_generation();

    // Probe input devices at startup, Xorg-style: drain libinput's
    // initial device enumeration and seed `state.xi_devices` BEFORE the
    // serve loop begins, so the first client to connect sees the real
    // device model (including a physical touchpad facet with its dynamic
    // ID) immediately. Without this the registry carries only the static
    // master and XTEST devices until
    // libinput's first `DeviceAdded` burst is dispatched from the loop,
    // which on real hardware can land seconds after the desktop's
    // clients have already enumerated devices and cached a plain
    // pointer. No-op for backends without an on-core libinput context
    // (Direct mode, host-X11/nested) — see `Backend::probe_input_devices`.
    let seeded = backend.probe_input_devices(state);
    log::info!("xi: startup input probe — {seeded} devices seeded");
    // TODO(direct-mode startup probe): in Direct mode the libinput
    // Context lives on the dedicated input thread, so the hook above is
    // a no-op here. The input thread already dispatches the initial
    // enumeration and sends the `DeviceAdded` burst on the channel as
    // its very first action (input_thread::run, before its epoll loop),
    // which shrinks the startup probe race. Fully closing
    // it would mean draining already-queued `Message::HostInput` device
    // events from `rx` here before the serve loop — left out for now to
    // avoid reordering/duplicating the loop's own message handling for a
    // path that isn't the primary (M2/Asahi) target.

    // Xorg seeds `_XKB_RULES_NAMES` on the root at init; setxkbmap reads
    // it to learn the current rules before applying a new layout.
    crate::core_loop::xkb_layout::publish_xkb_rules_names(state, backend);
    // Xorg's XkbFinishInit: the keyboard's per-key auto-repeat comes from
    // the keymap.
    crate::core_loop::xkb_layout::seed_keyboard_auto_repeats(state, backend);

    let mut events = Events::with_capacity(64);
    let mut telemetry = LoopTelemetry::new();
    if telemetry.enabled {
        crate::core_loop::fanout::enable_outbound_telemetry();
        log::info!(
            "loop telemetry: enabled (YSERVER_LOOP_TELEMETRY set); \
             1s rollups via info!"
        );
    }
    let mut deferred_requests = FairRequestQueue::default();
    let mut server_grab_waiters: VecDeque<DeferredRequest> = VecDeque::new();
    let mut pending_backend_requests = PendingBackendRequests::default();
    let mut randr_mutation_gate = RandrMutationGate::default();
    let mut deferred_reset_action = None;
    let mut xdmcp_terminate_pending = false;
    let mut xi_config_lane = XiConfigLane::default();

    // Process-lifetime, not per-generation — see `input_inventory`'s
    // module docs. Populated below on every `HostInput` device event;
    // nothing consumes it yet (step 1 of the server-reset plan).
    loop {
        // The grab can be dropped by paths that have no release check of
        // their own — notably the two disconnect sites outside the message
        // loop (a failed outbound write, and the writable-interest
        // reconcile). Re-check once per iteration so a released grab always
        // frees its waiters no matter who released it. Without this, an
        // owner that dies via a failed write leaves waiters parked while
        // `deferred_requests` stays empty, so the timeout below blocks on
        // deadlines and those clients hang until unrelated traffic arrives.
        if state.server_grab_owner.is_none() {
            release_server_grab_waiters(
                &mut deferred_requests,
                &mut server_grab_waiters,
                &mut telemetry,
            );
        }
        // BlockHandler analog (cf. Xorg glamor_block_handler → glamor_flush):
        // reap GPU render-op resources whose fences have signaled right
        // before we block, and service host replies, queued raw events,
        // newly adopted/canonical fences, deadlines, then eligible sequence sends.
        // Driving this before computing poll_timeout ensures any deadline or
        // readiness modified during before_block bounds the subsequent poll.
        backend.before_block();
        // Backends can replace device incarnations while servicing this loop.
        // Refresh their sources after `before_block` so newly opened executor
        // control fds become pollable before the next wait.
        let current_backend_poll_sources: Vec<_> = backend
            .poll_fds()
            .into_iter()
            .map(|(fd, kind)| BackendPollSource { fd, kind })
            .collect();
        let current_backend_poll_source_generation = backend.poll_source_generation();
        if current_backend_poll_sources != backend_poll_sources
            || current_backend_poll_source_generation != backend_poll_source_generation
        {
            deregister_backend_poll_sources(poll.registry(), &backend_poll_sources)?;
            backend_poll_sources = current_backend_poll_sources;
            register_backend_poll_sources(poll.registry(), &backend_poll_sources)?;
            backend_poll_source_generation = current_backend_poll_source_generation;
        }
        // Compute poll timeout. If there are runnable deferred requests, do
        // not block: drain them immediately. Otherwise, blocking could wait for
        // a fresh fd event, leaving the backlog stranded.
        let poll_timeout = if deferred_requests.has_runnable_with_gate(
            &pending_backend_requests,
            state,
            &randr_mutation_gate,
            backend,
        ) || listener_readiness.has_pending()
        {
            Some(Duration::ZERO)
        } else {
            // Wake for the earliest deadline owned by either core
            // key-repeat or the backend (for example, a compositor
            // commit retry). `Duration::ZERO` keeps mio returning
            // immediately when a deadline is already due.
            let now = Instant::now();
            let repeat_deadline = state
                .key_repeats
                .values()
                .map(|repeat| repeat.next_fire)
                .min();
            let backend_deadline = backend.next_wakeup();
            let dpms_deadline = state.dpms_transition_deadline();
            let ss_idle_deadline = state.screensaver_idle_deadline();
            let ss_cycle_deadline = state.screensaver_cycle_deadline();
            let idletime_alarm_deadline = state.idletime_alarm_deadline();
            let randr_gate_deadline = randr_mutation_gate.next_deadline();
            let sync_counter_deadline =
                crate::core_loop::sync_await::system_counter_deadline(state);
            // The XDMCP retransmission/dormancy deadline joins the existing
            // computation rather than bringing a thread of its own — the
            // state machine belongs on this loop, where it can see the
            // generation boundary directly.
            let xdmcp_deadline = xdmcp.as_ref().and_then(XdmcpService::next_deadline);
            let holders_deadline = telemetry.export_holders_deadline();
            repeat_deadline
                .into_iter()
                .chain(backend_deadline)
                .chain(holders_deadline)
                .chain(dpms_deadline)
                .chain(ss_idle_deadline)
                .chain(ss_cycle_deadline)
                .chain(idletime_alarm_deadline)
                .chain(randr_gate_deadline)
                .chain(sync_counter_deadline)
                .chain(xdmcp_deadline)
                .min()
                .map(|deadline| {
                    deadline
                        .checked_duration_since(now)
                        .unwrap_or(Duration::ZERO)
                })
        };
        // Retry on EINTR. A signal delivered while we're blocked in poll()
        // surfaces as `ErrorKind::Interrupted` — notably SIGCONT and the
        // VT/seat signals on resume-from-suspend. That is NOT fatal: re-poll.
        // Propagating it `?` crashed yserver on wake from sleep (run_core
        // returned EINTR → exit → drop to the display manager). Mirrors the
        // Interrupted handling in `client_reader.rs`.
        loop {
            match poll.poll(&mut events, poll_timeout) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    cancel_all_pending_backend_requests(backend, &mut pending_backend_requests);
                    return Err(e);
                }
            }
        }
        let iter_start = if telemetry.enabled {
            Some(Instant::now())
        } else {
            None
        };
        let mut requests_this_iter: u32 = 0;
        let mut request_budget: usize = MAX_REQUESTS_PER_ITER;
        // Deadline for this iteration's request processing, paired with
        // `request_budget` — see `REQUEST_TIME_BUDGET`. Taken
        // unconditionally (not gated on telemetry) because the drain
        // loops below depend on it for latency bounding, and measured
        // from here so the first request of the iteration always runs.
        let drain_start = Instant::now();
        // Drain backlog from prior iterations first. The ready ring gives each
        // active client one turn while the count/time cap still guarantees
        // input and page-flip maintenance between request slices.
        drain_pending_requests(
            state,
            backend,
            input_inventory,
            &mut telemetry,
            &mut pending_backend_requests,
            &mut randr_mutation_gate,
            &mut xi_config_lane,
            &mut reset_trigger,
            rx.current_generation(),
            &mut deferred_requests,
            &mut server_grab_waiters,
            &mut requests_this_iter,
            &mut request_budget,
            drain_start,
        );
        for ev in events.iter() {
            if let Some(index) = token_to_listener_index(ev.token()) {
                listener_readiness.mark_ready(index);
                continue;
            }
            if ev.token() == XDMCP_TOKEN {
                // `XdmcpSocketNotify` (`xdmcp.c:655`). Whatever the machine
                // decides is latched on the service and acted on at the
                // tail of this iteration, where the reset boundary lives.
                if let Some(service) = xdmcp.as_mut() {
                    service.handle_readable(&auth, rx.current_generation());
                }
                continue;
            }
            if let Some(index) = token_to_backend_index(ev.token()) {
                let Some(source) = backend_poll_sources.get(index).copied() else {
                    warn!(
                        "core_loop::run: backend poll token {:?} has no source",
                        ev.token()
                    );
                    continue;
                };
                match source.kind {
                    BackendFdKind::Drm => {
                        // Drain only the DRM device whose fd became readable.
                        // `receive_events()` may block on an idle device, so a
                        // multi-device backend must retain this exact identity.
                        if telemetry.enabled {
                            telemetry.page_flip_count += 1;
                        }
                        backend.on_page_flip_ready(state, source.fd);
                    }
                    BackendFdKind::DrmHotplug => {
                        backend.on_display_hotplug(state);
                    }
                    BackendFdKind::Libinput => {
                        // Optional core-owned libinput path. Direct KMS does
                        // not expose this fd because its input thread owns it.
                        backend.on_libinput_ready(state);
                    }
                    BackendFdKind::HostX11 => {
                        // Drain host frames into the backend's pending
                        // reply/event queues. Fanout remains at the outer-loop
                        // boundary to avoid recursive dispatch.
                        match backend.drain_host_socket() {
                            Ok(HostSocketStatus::WouldBlock) => {}
                            Ok(HostSocketStatus::Eof) => {
                                log::info!("host X11 connection closed; shutting down");
                                cancel_all_pending_backend_requests(
                                    backend,
                                    &mut pending_backend_requests,
                                );
                                return Ok(());
                            }
                            Err(err) => {
                                log::warn!("drain_host_socket: {err}");
                                cancel_all_pending_backend_requests(
                                    backend,
                                    &mut pending_backend_requests,
                                );
                                return Ok(());
                            }
                        }
                    }
                    BackendFdKind::PresentCompletion => {
                        drain_present_completions(state, backend);
                    }
                    BackendFdKind::ScanoutRenderCompletion => {
                        backend.on_scanout_render_completion(state);
                    }
                    BackendFdKind::ExecutorControl => {
                        backend.on_executor_readable(state);
                    }
                    BackendFdKind::OwnerCompletion => {
                        backend.on_owner_completion_ready(state);
                    }
                }
                continue;
            }
            match ev.token() {
                NOTIFY_TOKEN => {
                    let mut channel_requests = 0_usize;
                    let mut channel_requests_by_client = HashMap::new();
                    let mut channel_messages: VecDeque<_> = rx.try_recv_all_tagged().collect();
                    while let Some((msg_generation, msg)) = channel_messages.pop_front() {
                        // Discard stale session-scoped traffic at the top
                        // of dispatch (server-reset plan step 2). Inert
                        // today: the generation never advances yet, so
                        // `msg_generation` always equals the current one
                        // and every message dispatches exactly as before.
                        if !generation::should_dispatch(
                            rx.current_generation(),
                            msg_generation,
                            &msg,
                        ) {
                            continue;
                        }
                        match msg {
                            Message::Shutdown => {
                                setup_thread::shutdown_all(&setup_registry);
                                cancel_all_pending_backend_requests(
                                    backend,
                                    &mut pending_backend_requests,
                                );
                                return Ok(());
                            }
                            Message::ResetRequested => {
                                // SIGHUP under `-reset` / `-terminate`.
                                // Latched, not executed here: the
                                // boundary needs the loop-local
                                // collections that this dispatch arm
                                // has borrowed, and it runs at the tail
                                // of this same iteration.
                                log::info!("reset: SIGHUP requested a server reset");
                                reset_trigger.note_reset_requested();
                            }
                            Message::Request {
                                id,
                                sequence,
                                accepted_at,
                                header,
                                body,
                                attached_fd,
                            } => {
                                if telemetry.enabled {
                                    channel_requests += 1;
                                    *channel_requests_by_client.entry(id).or_insert(0) += 1;
                                    telemetry.record_request_accepted(id, sequence);
                                }
                                let req = DeferredRequest {
                                    id,
                                    sequence,
                                    accepted_at,
                                    randr_gate_ticket: None,
                                    randr_gate_expired: false,
                                    header,
                                    body,
                                    attached_fd,
                                };
                                // Keep one canonical per-client FIFO even
                                // while another client owns GrabServer. The
                                // drain path may temporarily park an older
                                // prefix, but newly accepted requests must
                                // remain behind the requests already queued
                                // for this client. Sending them directly to
                                // `server_grab_waiters` lets new arrivals jump
                                // ahead of that remaining suffix on release.
                                let mut req = req;
                                randr_mutation_gate.register_request(&mut req, backend);
                                telemetry.record_deferred_push(req.id);
                                deferred_requests.push_back(req);
                            }
                            Message::SetupAllocate { id, response_tx } => {
                                handle_setup_allocate(state, id, response_tx);
                            }
                            Message::ClientSetupComplete {
                                id,
                                generation,
                                stream,
                                resource_id_base,
                                resource_id_mask,
                                byte_order,
                                is_local,
                                fd_passing,
                                setup_reply,
                            } => {
                                if let Err(err) = handle_client_setup_complete(
                                    poll.registry(),
                                    &sender,
                                    &setup_registry,
                                    state,
                                    id,
                                    generation,
                                    stream,
                                    resource_id_base,
                                    resource_id_mask,
                                    byte_order,
                                    is_local,
                                    fd_passing,
                                    &setup_reply,
                                ) {
                                    error!("ClientSetupComplete for client {} failed: {err}", id.0);
                                    disconnect_with_xi_pending_cleanup(
                                        state,
                                        backend,
                                        &mut pending_backend_requests,
                                        &mut randr_mutation_gate,
                                        &mut xi_config_lane,
                                        &mut reset_trigger,
                                        id,
                                    );
                                } else if let Some(service) = xdmcp.as_mut() {
                                    // `XdmcpOpenDisplay` (`xdmcp.c:632`),
                                    // called from `ClientAuthorized`
                                    // (`os/connection.c:581`) for every
                                    // client that completes an authorized
                                    // setup. Immediately after
                                    // establishment, not deferred to the
                                    // tail: this is the ordering that
                                    // decides a `Refuse` racing an
                                    // in-flight setup, and the service
                                    // reports a client the race left with
                                    // no session to belong to.
                                    if service.note_client_established(
                                        id,
                                        is_local,
                                        &auth,
                                        rx.current_generation(),
                                    ) {
                                        // Orphaned by a lost `Refuse`
                                        // race. Drop it WITHOUT having
                                        // armed the reset trigger: an
                                        // orphan never counted as an
                                        // established client, so its
                                        // departure must not drain an
                                        // armed set and start a
                                        // generation mid-retry.
                                        disconnect_with_xi_pending_cleanup(
                                            state,
                                            backend,
                                            &mut pending_backend_requests,
                                            &mut randr_mutation_gate,
                                            &mut xi_config_lane,
                                            &mut reset_trigger,
                                            id,
                                        );
                                    } else {
                                        reset_trigger.note_client_established();
                                    }
                                } else {
                                    reset_trigger.note_client_established();
                                }
                            }
                            Message::ClientDisconnected { id, reason: _ } => {
                                disconnect_with_xi_pending_cleanup(
                                    state,
                                    backend,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut xi_config_lane,
                                    &mut reset_trigger,
                                    id,
                                );
                            }
                            Message::HostInput(ev) => {
                                if telemetry.enabled {
                                    telemetry.record_host_input(Instant::now());
                                }
                                dispatch_host_input(
                                    state,
                                    backend,
                                    input_inventory,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut xi_config_lane,
                                    &mut reset_trigger,
                                    ev,
                                    rx.current_generation(),
                                );
                            }
                            Message::CrtcConfigReady => {
                                if let Some(service) = xdmcp.as_mut() {
                                    observe_xdmcp_session_departure(
                                        service,
                                        state,
                                        &auth,
                                        rx.current_generation(),
                                    );
                                    match service.take_outcome() {
                                        Some(XdmcpOutcome::Terminate) => {
                                            xdmcp_terminate_pending = true;
                                        }
                                        Some(XdmcpOutcome::Reset) => {
                                            reset_trigger.note_reset_requested();
                                        }
                                        None => {}
                                    }
                                }
                                let publish_old_generation = !xdmcp_terminate_pending
                                    && deferred_reset_action.is_none()
                                    && !reset_trigger.has_pending();
                                drain_ready_crtc_configs_with_gate_policy(
                                    state,
                                    backend,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut reset_trigger,
                                    publish_old_generation,
                                );
                            }
                            message @ Message::DeviceConfigResult { .. } => {
                                dispatch_device_config_result(
                                    state,
                                    backend,
                                    input_inventory,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut xi_config_lane,
                                    &mut reset_trigger,
                                    message,
                                    rx.current_generation(),
                                )
                            }
                            Message::InputPaused => {
                                // Consumed only by the synchronous VT-release
                                // barrier; ignore any duplicate or stale ack.
                            }
                            Message::VtRelease => {
                                // When VT switching isn't armed there is no
                                // switch to service — ignore. (Deliberate
                                // diagnostic dumps go through DumpScanout /
                                // DumpDrawables via the Ctrl-Alt-Enter /
                                // Ctrl-Alt-F12 hotkeys, not this path.)
                                if backend.vt_switching_armed() {
                                    drain_vt_release_requests(
                                        state,
                                        backend,
                                        input_inventory,
                                        &mut telemetry,
                                        &mut pending_backend_requests,
                                        &mut randr_mutation_gate,
                                        &mut xi_config_lane,
                                        &mut reset_trigger,
                                        rx.current_generation(),
                                        &mut deferred_requests,
                                        &mut server_grab_waiters,
                                    );
                                    fail_unsubmitted_xi_configs_for_vt_release(
                                        state,
                                        backend,
                                        &mut pending_backend_requests,
                                        &mut randr_mutation_gate,
                                        &mut xi_config_lane,
                                        &mut reset_trigger,
                                        rx.current_generation(),
                                    );
                                }
                                dispatch_vt_release(
                                    state,
                                    backend,
                                    input_inventory,
                                    |state, backend, input_inventory, pause_barrier_queued| {
                                        let mut deferred = VecDeque::new();
                                        let mut pause_acknowledged = false;
                                        if pause_barrier_queued {
                                            let deadline = Instant::now() + VT_INPUT_PAUSE_TIMEOUT;
                                            loop {
                                                let remaining = deadline
                                                    .saturating_duration_since(Instant::now());
                                                if remaining.is_zero() {
                                                    break;
                                                }
                                                // Messages after VtRelease may already be in
                                                // this batch: prefer them before receiving, or a
                                                // pre-drained pause ack could be stranded behind
                                                // the release.
                                                let message = if let Some(message) =
                                                    channel_messages.pop_front()
                                                {
                                                    Ok(message)
                                                } else {
                                                    rx.recv_tagged_timeout(remaining)
                                                };
                                                let Ok((completion_generation, completion)) =
                                                    message
                                                else {
                                                    break;
                                                };
                                                if !generation::should_dispatch(
                                                    rx.current_generation(),
                                                    completion_generation,
                                                    &completion,
                                                ) {
                                                    continue;
                                                }
                                                match completion {
                                                    message @ Message::DeviceConfigResult {
                                                        ..
                                                    } => {
                                                        dispatch_device_config_result(
                                                            state,
                                                            backend,
                                                            input_inventory,
                                                            &mut pending_backend_requests,
                                                            &mut randr_mutation_gate,
                                                            &mut xi_config_lane,
                                                            &mut reset_trigger,
                                                            message,
                                                            rx.current_generation(),
                                                        );
                                                        drain_vt_release_requests(
                                                            state,
                                                            backend,
                                                            input_inventory,
                                                            &mut telemetry,
                                                            &mut pending_backend_requests,
                                                            &mut randr_mutation_gate,
                                                            &mut xi_config_lane,
                                                            &mut reset_trigger,
                                                            rx.current_generation(),
                                                            &mut deferred_requests,
                                                            &mut server_grab_waiters,
                                                        );
                                                    }
                                                    Message::InputPaused => {
                                                        drain_vt_release_requests(
                                                            state,
                                                            backend,
                                                            input_inventory,
                                                            &mut telemetry,
                                                            &mut pending_backend_requests,
                                                            &mut randr_mutation_gate,
                                                            &mut xi_config_lane,
                                                            &mut reset_trigger,
                                                            rx.current_generation(),
                                                            &mut deferred_requests,
                                                            &mut server_grab_waiters,
                                                        );
                                                        pause_acknowledged = true;
                                                        break;
                                                    }
                                                    Message::Request {
                                                        id,
                                                        sequence,
                                                        accepted_at,
                                                        header,
                                                        body,
                                                        attached_fd,
                                                    } => {
                                                        if telemetry.enabled {
                                                            channel_requests += 1;
                                                            *channel_requests_by_client
                                                                .entry(id)
                                                                .or_insert(0) += 1;
                                                            telemetry.record_request_accepted(
                                                                id, sequence,
                                                            );
                                                        }
                                                        telemetry.record_deferred_push(id);
                                                        deferred_requests.push_back(
                                                            DeferredRequest {
                                                                id,
                                                                sequence,
                                                                accepted_at,
                                                                header,
                                                                body,
                                                                attached_fd,
                                                                randr_gate_ticket: None,
                                                                randr_gate_expired: false,
                                                            },
                                                        );
                                                        drain_vt_release_requests(
                                                            state,
                                                            backend,
                                                            input_inventory,
                                                            &mut telemetry,
                                                            &mut pending_backend_requests,
                                                            &mut randr_mutation_gate,
                                                            &mut xi_config_lane,
                                                            &mut reset_trigger,
                                                            rx.current_generation(),
                                                            &mut deferred_requests,
                                                            &mut server_grab_waiters,
                                                        );
                                                    }
                                                    message => deferred.push_back((
                                                        completion_generation,
                                                        message,
                                                    )),
                                                }
                                            }
                                        }
                                        if pause_barrier_queued && !pause_acknowledged {
                                            warn!(
                                                "VT input pause barrier was not acknowledged within {:?}; failing outstanding XI config write",
                                                VT_INPUT_PAUSE_TIMEOUT,
                                            );
                                        }
                                        // This is normally empty after InputPaused because
                                        // input-thread config results precede the FIFO ack.
                                        // Still reject any residue, including when no barrier
                                        // could be queued, before KMS starts yielding the VT.
                                        fail_in_flight_xi_config_for_vt_release(
                                            state,
                                            backend,
                                            &mut pending_backend_requests,
                                            &mut randr_mutation_gate,
                                            &mut xi_config_lane,
                                            &mut reset_trigger,
                                            rx.current_generation(),
                                        );
                                        channel_messages.append(&mut deferred);
                                    },
                                );
                                xi_config_lane.reject_unsubmitted_for_vt_release = false;
                            }
                            Message::VtAcquire => {
                                drain_vt_release_requests(
                                    state,
                                    backend,
                                    input_inventory,
                                    &mut telemetry,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut xi_config_lane,
                                    &mut reset_trigger,
                                    rx.current_generation(),
                                    &mut deferred_requests,
                                    &mut server_grab_waiters,
                                );
                                fail_unsubmitted_xi_configs_for_vt_release(
                                    state,
                                    backend,
                                    &mut pending_backend_requests,
                                    &mut randr_mutation_gate,
                                    &mut xi_config_lane,
                                    &mut reset_trigger,
                                    rx.current_generation(),
                                );
                            }
                            Message::SwitchVt(vt) => {
                                if backend.vt_switching_armed() {
                                    backend.request_vt_switch(vt);
                                }
                            }
                            Message::DumpScanout => backend.dump_scanout(),
                            Message::DumpDrawables => backend.dump_drawables(),
                        }
                        if state.server_grab_owner.is_none() {
                            release_server_grab_waiters(
                                &mut deferred_requests,
                                &mut server_grab_waiters,
                                &mut telemetry,
                            );
                        }
                    }
                    telemetry.record_channel_drain(channel_requests, &channel_requests_by_client);
                    drain_pending_requests(
                        state,
                        backend,
                        input_inventory,
                        &mut telemetry,
                        &mut pending_backend_requests,
                        &mut randr_mutation_gate,
                        &mut xi_config_lane,
                        &mut reset_trigger,
                        rx.current_generation(),
                        &mut deferred_requests,
                        &mut server_grab_waiters,
                        &mut requests_this_iter,
                        &mut request_budget,
                        drain_start,
                    );
                }
                tok => {
                    let Some(client_id) = token_to_client(tok) else {
                        warn!("core_loop::run: unhandled poll token {tok:?}");
                        continue;
                    };

                    // I3: WRITABLE-readiness on a client writer fd.
                    // Drain the outbound buffer; if it empties, the
                    // post-loop interest reconciliation drops
                    // WRITABLE. If the peer disappeared, mark the
                    // client for disconnect.
                    if !ev.is_writable() {
                        // mio always reports both READABLE+WRITABLE
                        // as readiness even when only one was asked
                        // for; the writer fd's READABLE wakeups are
                        // ignored — the reader thread owns reads.
                        continue;
                    }
                    let Some(client) = state.clients.get_mut(&client_id.0) else {
                        // Already removed by a prior disconnect; the
                        // poller will be deregistered after.
                        continue;
                    };
                    match client_io::drain_outbound(client) {
                        Ok(WriteOutcome::Done | WriteOutcome::WouldBlock) => {}
                        Ok(WriteOutcome::Disconnect) | Err(_) => {
                            disconnect_with_xi_pending_cleanup(
                                state,
                                backend,
                                &mut pending_backend_requests,
                                &mut randr_mutation_gate,
                                &mut xi_config_lane,
                                &mut reset_trigger,
                                client_id,
                            );
                        }
                    }
                }
            }
        }
        listener_readiness.accept_ready(
            &listeners,
            client_id_allocator,
            &sender,
            &setup_registry,
            &auth,
        );
        // F2: drain any host-X11 events the backend decoded during
        // this iteration. Fanout runs at the outermost stack frame
        // — no `wait_for_reply` is on the stack here — so handlers
        // that issue further host requests are safe.
        if dispatch_pending_host_events(state, backend) {
            // Host events (pointer, expose, configure) can change
            // visible state; mark dirty so the KMS gate re-arms. No-op
            // for backends without their own composite loop.
            backend.mark_dirty();
        }

        // Auto-repeat: if a key is held and its `next_fire` has
        // elapsed (either because the poll woke on the timeout, or
        // because an unrelated event arrived after the deadline),
        // fan out a synthetic KeyRelease+KeyPress pair.
        if !state.key_repeats.is_empty() {
            // Only poke the compositor when a repeat actually fired.
            // `fire_pending_repeats` returns false when the armed key
            // is merely not-yet-due (the common case every iteration
            // while a key is held) — an unconditional `mark_dirty()`
            // here re-dirtied the scene at the loop-iteration rate,
            // busy-spinning the compositor (and never letting it idle
            // when a phantom key is stuck armed).
            if fire_pending_repeats(state, backend) {
                backend.mark_dirty();
            }
        }

        // DPMS: evaluate idle-cascade transitions.
        if let Some(deadline) = state.dpms_transition_deadline() {
            let now = Instant::now();
            if now >= deadline {
                // Saturate rather than truncate — `as_millis()` returns u128
                // and idle > 49 days would silently wrap a `as u32` cast,
                // which would then fall *below* the timeout thresholds.
                let idle_ms = u32::try_from(state.dpms.last_activity.elapsed().as_millis())
                    .unwrap_or(u32::MAX);
                let target =
                    crate::server::next_dpms_level(state.dpms.power_level, idle_ms, &state.dpms);
                if target != state.dpms.power_level {
                    crate::core_loop::process_request::apply_dpms_transition(
                        state, backend, target,
                    );
                }
            }
        }

        // SS: evaluate idle activation and Cycle re-fire.
        evaluate_screen_saver_post_poll(state, backend);
        evaluate_idletime_alarms_post_poll(state, backend);
        crate::core_loop::sync_await::evaluate_servertime(state);

        // F2: if a `wait_for_reply` (called by `process_request`
        // mid-handler) saw the host close, propagate it as a clean
        // shutdown. The IO error already surfaced to the caller; we
        // observe the EOF flag here and stop the core loop.
        if backend.host_socket_eof() {
            log::info!("host X11 EOF observed; shutting down");
            cancel_all_pending_backend_requests(backend, &mut pending_backend_requests);
            return Ok(());
        }

        // I2: walk clients once per loop iteration and reconcile
        // poll interest against the live state of `outbound`. A
        // client whose buffer just became non-empty needs WRITABLE;
        // one that just drained back to empty drops it. Swallows
        // reregister errors that mean "fd already deregistered" so a
        // disconnect that ran during this iteration doesn't break
        // the next one.
        // RECORD data connections whose stream write failed (queued, since
        // the write can happen inside another client's disconnect).
        for disc_id in crate::core_loop::record::take_failed_recorders(state) {
            disconnect_with_xi_pending_cleanup(
                state,
                backend,
                &mut pending_backend_requests,
                &mut randr_mutation_gate,
                &mut xi_config_lane,
                &mut reset_trigger,
                disc_id,
            );
        }
        for disc_id in reconcile_client_writable_interest(poll.registry(), state) {
            disconnect_with_xi_pending_cleanup(
                state,
                backend,
                &mut pending_backend_requests,
                &mut randr_mutation_gate,
                &mut xi_config_lane,
                &mut reset_trigger,
                disc_id,
            );
        }

        run_iteration_tail(state, backend);

        // Diagnostic: per-iteration accounting + per-second telemetry
        // emit. Both are no-ops when `YSERVER_LOOP_TELEMETRY` is unset.
        if let Some(start) = iter_start {
            let now = Instant::now();
            let wall = now.saturating_duration_since(start);
            telemetry.record_iteration(requests_this_iter, wall);
            telemetry.maybe_emit(now);
            if telemetry.export_holders_due(now) {
                let core_state: &ServerState = state;
                let changed = backend.report_export_holders(&|| {
                    crate::backend::export_holders::collect_core_holders(core_state)
                });
                telemetry.note_export_holders(now, changed);
            }
        }

        // The generation boundary. Reached only from an action the
        // trigger LATCHED earlier in this iteration — a departure that
        // drained an armed generation, or a SIGHUP — never from a state
        // check here: an idle client set is indistinguishable from a
        // drained one, and a `-reset` server that inspected
        // `state.clients` would reset itself repeatedly at startup.
        //
        // The boundary runs at the tail rather than at the disconnect
        // site because it needs the loop-local collections
        // (`GenerationLocals`) that the dispatch arms have borrowed.
        // Deferring it inside one iteration is also what makes the
        // cancellation in `note_client_established` meaningful: a
        // client that completes setup after the drain, in this same
        // batch, un-drains the session before the boundary is reached.
        // XDMCP, once per iteration and immediately before the boundary:
        // fire a due timer, notice the session client leaving, and act on
        // whatever the machine decided.
        if let Some(service) = xdmcp.as_mut() {
            service.service_timer(Instant::now(), &auth, rx.current_generation());
            // `XdmcpCloseDisplay` (`xdmcp.c:642`). Ids are allocated
            // monotonically and only `disconnect_with_pending_cleanup`
            // removes an entry, so a recorded session client that is no
            // longer in `state.clients` HAS departed — this is the
            // departure, not a guess about one.
            observe_xdmcp_session_departure(service, state, &auth, rx.current_generation());
            match service.take_outcome() {
                None => {}
                Some(XdmcpOutcome::Terminate) => {
                    log::info!("xdmcp: terminating the server");
                    xdmcp_terminate_pending = true;
                }
                Some(XdmcpOutcome::Reset) => {
                    // Forced, like SIGHUP: a client connecting between the
                    // session ending and the boundary must not veto the
                    // renewal the protocol already committed to.
                    reset_trigger.note_reset_requested();
                }
            }
        }

        if xdmcp_termination_can_finish(xdmcp_terminate_pending, &randr_mutation_gate, backend) {
            setup_thread::shutdown_all(&setup_registry);
            cancel_all_pending_backend_requests(backend, &mut pending_backend_requests);
            return Ok(());
        }

        let pending_action = defer_reset_action_for_install_capable_mutation(
            &randr_mutation_gate,
            backend,
            &mut deferred_reset_action,
            reset_trigger.take_pending(),
        );
        match pending_action {
            None => {}
            Some(ResetAction::Terminate) => {
                log::info!("reset: -terminate — last client left, shutting down");
                setup_thread::shutdown_all(&setup_registry);
                cancel_all_pending_backend_requests(backend, &mut pending_backend_requests);
                return Ok(());
            }
            Some(ResetAction::Reset) => {
                // Unsubmitted client requests belong to the generation
                // being retired. Keep an already submitted input command in
                // the process-lifetime lane, but discard its old protocol
                // continuation so a late success commits using current atoms.
                cancel_unsubmitted_xi_configs(&mut xi_config_lane, &mut pending_backend_requests);
                let outcome = reset_generation(
                    state,
                    backend,
                    poll.registry(),
                    &generations,
                    &setup_registry,
                    input_inventory,
                    GenerationLocals {
                        deferred_requests: &mut deferred_requests,
                        server_grab_waiters: &mut server_grab_waiters,
                        pending_backend_requests: &mut pending_backend_requests,
                        randr_mutation_gate: &mut randr_mutation_gate,
                        telemetry: &mut telemetry,
                    },
                );
                // The boundary refused: the old session's composite overlay
                // could not be released, so there is no safe generation to
                // continue into. `reset_generation` has already logged why.
                // Shut down the same way `-terminate` does — under XDMCP the
                // display manager re-queries and gets a clean process.
                let Some(generation) = outcome else {
                    setup_thread::shutdown_all(&setup_registry);
                    cancel_all_pending_backend_requests(backend, &mut pending_backend_requests);
                    return Ok(());
                };
                // Disarm for the generation just installed. Without
                // this the empty client set the reset leaves behind
                // would be re-latched by the next departure-shaped
                // event and reset a second time.
                reset_trigger.begin_generation();
                log::info!("reset: new generation installed ({generation:?})");
                // `XdmcpReset` (`xdmcp.c:618`), AFTER the new generation is
                // installed — the cookie the re-query is about to earn
                // belongs to this generation, and binding it to the old one
                // would refuse the very session it is fetching.
                if let Some(service) = xdmcp.as_mut() {
                    service.restart(&auth, generation);
                }
            }
        }
    }
}

/// The loop-body tail: service time-based backend work, drain due Present
/// work, then kick the compose path. Extracted so the drain-before-compose
/// ordering (see the comment on the `drain_present_completions` call below)
/// is independently testable via `RecordingBackend` without spinning up the
/// full `run` poll loop.
pub(crate) fn run_iteration_tail(state: &mut ServerState, backend: &mut dyn Backend) {
    // Damage can also originate outside a directly-dispatched request (for
    // example deferred Present execution). Preserve the same write-before-
    // observer boundary before the next poll can drain client output.
    if std::mem::take(&mut state.damage_notify_flush_pending) {
        backend.flush_before_damage_notify();
    }

    // Service time-based backend work that is not tied to an fd edge. The
    // backend reports its cadence via `next_wakeup`.
    backend.poll_deferred_input(state);

    // Pointer motion and other input-driven sprite changes.
    crate::core_loop::process_request::emit_xfixes_cursor_notify(state, backend);

    // Drain-before-compose (spec "Loop-order and clock contract" item 1):
    // an entry executed here must be visible to THIS iteration's
    // `maybe_composite`, or it slips a full period whenever unrelated
    // damage exists.
    drain_present_completions(state, backend);

    // Wake the composite path back up if the backend went dormant
    // after the previous pageflip-complete (because nothing was
    // dirty) and fresh damage has since arrived. No-op for
    // backends that don't drive their own composite loop, and
    // no-op if a flip is still in flight on the KMS path.
    if let Err(e) = backend.maybe_composite() {
        log::warn!("core_loop::run: maybe_composite failed: {e}");
    }

    arm_present_idle_vblanks(state, backend);
}

/// Invoke the production loop-body tail from an external test harness.
///
/// The core loop calls [`run_iteration_tail`] after readiness handling and
/// request processing. KMS integration tests use this entry to keep their
/// bounded driver on the same tail path without exposing the core loop's
/// private request queues.
#[doc(hidden)]
pub fn run_iteration_tail_for_tests(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    delivery_trace: &mut Vec<CoreEntryDelivery>,
) {
    run_iteration_tail(state, backend);
    TEST_CORE_RANDR_GATE.with(|gate| {
        drain_requesterless_publications(
            state,
            backend,
            &mut gate.borrow_mut(),
            true,
            Some(delivery_trace),
        );
    });
}

/// Hold the existing core-entry test gate for a forced reprobe while a KMS
/// fixture drives backend entries. This lets the shared driver verify that
/// topology events remain queued behind the request's gate turn.
#[doc(hidden)]
pub fn hold_forced_reprobe_turn_for_tests(token: CrtcConfigToken) {
    TEST_CORE_RANDR_GATE.with(|gate| gate.borrow_mut().hold_forced_reprobe_turn_for_tests(token));
}

/// Complete the test-only forced turn after the backend's ready token has
/// been consumed, then let the next core-entry iteration deliver queued events.
#[doc(hidden)]
pub fn release_forced_reprobe_turn_for_tests(token: CrtcConfigToken) {
    TEST_CORE_RANDR_GATE.with(|gate| gate.borrow_mut().finish_pending(token));
}

thread_local! {
    // The existing KMS core-entry driver calls this tail repeatedly on one
    // test thread. Keep the same mutation gate across those iterations so
    // its backend entries observe the production EpisodeBegin/grant/End path.
    static TEST_CORE_RANDR_GATE: std::cell::RefCell<RandrMutationGate> =
        std::cell::RefCell::new(RandrMutationGate::default());
}

/// Idle vblank arming for parked Present work — MUST run after
/// `maybe_composite`, not folded back into the pre-compose drain. KMS's
/// completion arm hard-gates on `present_completion_is_idle()`
/// (`!has_pending_page_flips() && !scene_wants_compose()`); `mark_dirty()`
/// alone (no output damage) makes `tick_one_output` return
/// `Skipped(EmptyDamage)`, which still clears `scene_wants_compose()`. Arm
/// before compose and that clear hasn't happened yet, so the gate sees a
/// dirty scene, arms nothing (`Ok(0)`), and a parked `CompleteNotify` can
/// starve with no fd left to wake `poll`. Running here, once per iteration,
/// also covers parks made by the epfd-driven drain (`run.rs:1027`, itself
/// pre-compose) in the same iteration — the backend dedups against its
/// per-CRTC armed-target map so a second call per iteration is safe.
pub(crate) fn arm_present_idle_vblanks(state: &mut ServerState, backend: &mut dyn Backend) {
    // Idle vblank arming: if NotifyMSC requests remain parked, ask the
    // backend to schedule a kernel vblank so the clock keeps advancing even
    // when nothing is flipping. A full-screen compositor redirects every
    // window → the scene is a static overlay → no pageflips → MSC never
    // advances → the compositor's `present` clock deadlocks. The backend
    // dedups against its per-CRTC armed-target map, so calling every
    // iteration is safe (no refire storm).
    if !state.present_pending_msc.is_empty() {
        let mut by_domain: std::collections::BTreeMap<
            (u32, u64),
            Vec<crate::backend::PresentSequenceTarget>,
        > = std::collections::BTreeMap::new();
        for pending in &state.present_pending_msc {
            by_domain
                .entry((pending.crtc_id, pending.crtc_epoch))
                .or_default()
                .push(crate::backend::PresentSequenceTarget {
                    consumer: pending.sequence_consumer,
                    target: pending.target_msc,
                });
        }
        for ((crtc_id, crtc_epoch), targets) in by_domain {
            if backend.present_crtc_clock_epoch(crtc_id) != crtc_epoch {
                continue;
            }
            match backend.arm_idle_vblanks_for_consumers(crtc_id, crtc_epoch, &targets) {
                Ok(armed) => {
                    if armed > 0 {
                        log::debug!(
                            "PRESENT-DBG: arm_idle_vblanks crtc=0x{crtc_id:x} pending={} -> armed={armed}",
                            targets.len()
                        );
                    }
                }
                Err(e) => log::warn!(
                    "PRESENT-DBG: arm_idle_vblanks crtc=0x{crtc_id:x} pending={} -> ERR {e}",
                    targets.len()
                ),
            }
        }
    }
    if !state.present_pending_complete.is_empty() {
        let mut by_domain: std::collections::BTreeMap<
            (u32, u64),
            Vec<crate::backend::PresentSequenceTarget>,
        > = std::collections::BTreeMap::new();
        for pending in &state.present_pending_complete {
            by_domain
                .entry((pending.event.crtc_id, pending.event.crtc_epoch))
                .or_default()
                .push(crate::backend::PresentSequenceTarget {
                    consumer: pending.event.present_id,
                    target: pending.effective_target_msc,
                });
        }
        for ((crtc_id, crtc_epoch), targets) in by_domain {
            if backend.present_crtc_clock_epoch(crtc_id) != crtc_epoch {
                continue;
            }
            // A page flip in flight is not sufficient as the only wake
            // source: arm the selected CRTC independently.
            let result = if backend.present_absolute_vblank_arm_supported(crtc_id) {
                backend.arm_present_absolute_vblank_for_consumers(crtc_id, crtc_epoch, &targets)
            } else {
                backend.arm_present_completion_idle_vblanks_for_consumers(
                    crtc_id, crtc_epoch, &targets,
                )
            };
            match result {
                Ok(armed) => {
                    if armed > 0 {
                        log::debug!(
                            "PRESENT-DBG: arm_present_completion_vblanks crtc=0x{crtc_id:x} pending={} -> armed={armed}",
                            targets.len()
                        );
                    }
                }
                Err(e) => log::warn!(
                    "PRESENT-DBG: arm_present_completion_vblanks crtc=0x{crtc_id:x} pending={} -> ERR {e}",
                    targets.len()
                ),
            }
        }
    }

    // Third arming call site (spec §msc-due, future-target fallback rung
    // 1): parked msc-due entries whose target is more than one vblank out
    // get an absolute per-target sequence arm here, alongside the other
    // two idle arms above — placement matches the spec's own wording
    // ("a third arming call site in run.rs, alongside present_pending_msc
    // ... and present_pending_complete ...", spec §msc-due future-target
    // bullet), not folded into the pre-compose due-pass
    // (`drain_due_present_pending_exec`): this call arms a kernel event,
    // it doesn't decide an execution, and every other arming call site in
    // this codebase already lives in this post-compose function. Must
    // NOT route through `arm_present_completion_idle_vblanks` — its
    // idle-only gate would suppress the arm during any activity.
    {
        // `(present_id, eff - 1)` for every still-parked, source-ready,
        // genuinely future-target entry. The `-1` is CORE-SIDE: `eff` is
        // the vblank at which the compose carrying this copy must already
        // have been submitted, so the copy itself is due one vblank
        // earlier, at `eff - 1`. `arm_present_absolute_vblank` arms
        // exactly the values it receives (Task 3) — it does not itself
        // subtract. `wrapping_sub`: `eff` is a wrapped MSC value (u64
        // wraparound is a documented, tested case throughout this
        // module), so a plain `eff - 1` would debug-panic when `eff == 0`.
        let mut by_domain: std::collections::BTreeMap<
            (u32, u64),
            Vec<crate::backend::PresentSequenceTarget>,
        > = std::collections::BTreeMap::new();
        for (&pid, entry) in &state.present_pending_exec {
            if !entry.source_ready {
                continue;
            }
            let crtc_id = entry.pending.crtc_id;
            let crtc_epoch = entry.pending.crtc_epoch;
            if backend.present_crtc_clock_epoch(crtc_id) != crtc_epoch
                || !backend.present_absolute_vblank_arm_supported(crtc_id)
            {
                continue;
            }
            let clock_msc = crate::core_loop::process_request::cached_present_crtc_clock(
                state, crtc_id, crtc_epoch,
            )
            .msc;
            if let Some(eff) = entry.pending.effective_target_msc
                && crate::present_scheduler::msc_is_after(eff, clock_msc.wrapping_add(1))
            {
                by_domain.entry((crtc_id, crtc_epoch)).or_default().push(
                    crate::backend::PresentSequenceTarget {
                        consumer: pid,
                        target: eff.wrapping_sub(1),
                    },
                );
            }
        }
        for ((crtc_id, crtc_epoch), future_parked) in by_domain {
            let targets = future_parked;
            // Full coverage required, not just `> 0`: the trait contract
            // (`arm_present_absolute_vblank`'s doc comment) allows a
            // partial `Ok(n)` — some targets newly armed or already
            // covered, others not (e.g. a CRTC set change mid-call).
            // Treating any partial result as success would leave the
            // uncovered subset parked with no wake source at all.
            // Unreachable against today's KMS impl (Task 3): it arms
            // every target on every connected CRTC or trips the
            // EOPNOTSUPP latch and returns `Err`, so it's all-or-`Err`
            // in practice — this guard is a contract-level guarantee,
            // not a dead branch removal candidate.
            match backend.arm_present_absolute_vblank_for_consumers(crtc_id, crtc_epoch, &targets) {
                Ok(covered) if covered == targets.len() => {
                    log::debug!(
                        "PRESENT-DBG: arm_present_absolute_vblank crtc=0x{crtc_id:x} pending={} -> armed={covered}",
                        targets.len()
                    );
                }
                other => {
                    // `Ok(0)` (nothing covered — including the iteration
                    // where an EOPNOTSUPP latch first trips), a partial
                    // `Ok(n < targets.len())`, or `Err`: the caller must
                    // not park the uncovered entries on this mechanism.
                    // Execute ALL of them immediately in this same pass
                    // (trigger=idle_fallback) rather than leave any
                    // subset parked with no wake source. This runs
                    // post-compose (this function, per the call-site
                    // placement above), so a latch-trip execution here
                    // misses THIS iteration's compose and lands in the
                    // next one instead — `mark_dirty` still guarantees
                    // the wake for it; accepted as a rare, one-iteration-
                    // latency path.
                    match other {
                        Ok(covered) => log::debug!(
                            "PRESENT-DBG: arm_present_absolute_vblank crtc=0x{crtc_id:x} pending={} -> covered={covered}, \
                             executing immediately",
                            targets.len()
                        ),
                        Err(e) => log::warn!(
                            "PRESENT-DBG: arm_present_absolute_vblank crtc=0x{crtc_id:x} pending={} -> ERR {e}",
                            targets.len()
                        ),
                    }
                    let ids: Vec<u64> = targets.iter().map(|t| t.consumer).collect();
                    crate::core_loop::process_request::execute_parked_present_ids(
                        state,
                        backend,
                        &ids,
                        "idle_fallback",
                    );
                }
            }
        }
    }
}

fn drain_present_completions(state: &mut ServerState, backend: &mut dyn Backend) {
    // Producer readiness precedes copy submission, which in turn precedes the
    // existing GPU-completion queue below. Keeping both on the same stable
    // backend wake fd avoids blocking request dispatch on client GPU work.
    crate::core_loop::process_request::drain_ready_present_pixmaps(state, backend);

    // msc-due-pass (spec §msc-due; Task 7): re-classify every msc-parked
    // source-ready entry against the fresh general clock and execute
    // whatever is now due, plus the idle-display and blackout fallback
    // rungs (the absolute-vblank-arm rung is a call-site match for the
    // other two arms below and lives in `arm_present_idle_vblanks`,
    // post-compose). Runs here, at the top of this pre-compose drain
    // (Task 4), so an entry executed here is visible to THIS iteration's
    // compose.
    crate::core_loop::process_request::drain_due_present_pending_exec(state, backend);

    let completed = backend.drain_completed_present_events();
    for entry in completed {
        if !crate::core_loop::process_request::present_event_window_is_current(state, &entry) {
            state.present_complete_gate.remove(&entry.present_id);
            crate::core_loop::process_request::discard_stale_present_event(
                state, backend, &entry, false,
            );
            continue;
        }
        let completion_clock =
            crate::core_loop::process_request::refresh_present_crtc_completion_clock(
                state,
                backend,
                entry.crtc_id,
                entry.crtc_epoch,
                entry.completion_clock,
            );
        // Pace: if this completion recorded a future target-msc gate, park the
        // whole thing (wake NOT signalled yet) until that vblank. Otherwise
        // (no clock / target already reached) complete now.
        // The epoch-qualified cache here is the previous iteration's value;
        // the refresh + per-domain sweep below release anything due now.
        match state.present_complete_gate.remove(&entry.present_id) {
            Some(gate)
                if backend.present_crtc_clock_epoch(gate.crtc_id) == gate.crtc_epoch
                    && crate::present_scheduler::msc_is_after(
                        gate.effective_target_msc,
                        completion_clock.msc,
                    ) =>
            {
                let mode = entry.completion_mode;
                let emit_idle = entry.emit_idle;
                log::debug!(
                    target: "present_pace",
                    "PACE-INSTR t={} pid={} stage=drained_parked eff={} kernel_msc={}",
                    crate::core_loop::process_request::pace_instr_ms(),
                    entry.present_id,
                    gate.effective_target_msc,
                    completion_clock.msc
                );
                state
                    .present_pending_complete
                    .push(crate::server::PendingPresentComplete {
                        event: entry,
                        effective_target_msc: gate.effective_target_msc,
                        mode,
                        emit_idle,
                    });
            }
            Some(gate) => {
                let mode = entry.completion_mode;
                let emit_idle = entry.emit_idle;
                // Due now against the completion clock, but still routed
                // through the ordered queue (spec §Ordered completion
                // delivery item 2) rather than fired here directly: a
                // Skip parked earlier at scrap (request-arrival) time can
                // have a *smaller* present_id than this entry's, and
                // firing this Copy immediately would let it overtake that
                // Skip in the client's per-window CompleteNotify stream.
                // `fire_due_present_completions`, called later in this
                // same drain pass, delivers in per-window present_id
                // order instead of raw arrival order.
                log::debug!(
                    target: "present_pace",
                    "PACE-INSTR t={} pid={} stage=drained_due completion_msc={} source={:?}",
                    crate::core_loop::process_request::pace_instr_ms(),
                    entry.present_id,
                    completion_clock.msc,
                    completion_clock.source
                );
                state
                    .present_pending_complete
                    .push(crate::server::PendingPresentComplete {
                        event: entry,
                        effective_target_msc: gate.effective_target_msc,
                        mode,
                        emit_idle,
                    });
            }
            None => {
                log::debug!(
                    target: "present_pace",
                    "PACE-INSTR t={} pid={} stage=drained_immediate kernel_msc={}",
                    crate::core_loop::process_request::pace_instr_ms(),
                    entry.present_id,
                    completion_clock.msc
                );
                // Async completions sit outside the per-window hold-back
                // by design (spec round-4 F6) and fire here immediately —
                // but flush anything already due-and-unblocked in the
                // queue FIRST, or this inline fire would itself create a
                // backward serial against a same-window gated Copy that
                // is due but hasn't been swept yet (that Copy was pushed
                // into the queue by the `Some(gate)` arm above, earlier
                // in this same `completed` loop, for exactly this
                // reason). Held-back entries are unaffected — they stay
                // held regardless of how many times the sweep runs.
                crate::core_loop::process_request::fire_due_present_completions_for_domain(
                    state,
                    backend,
                    entry.crtc_id,
                    entry.crtc_epoch,
                    completion_clock,
                );
                crate::core_loop::process_request::complete_present_now(state, backend, &entry);
            }
        }
    }

    // Direct Present completion and source-idle are different retirements.
    // A replacement frame idles the previous source without completing it a
    // second time.
    for event in backend.drain_retired_present_idle_events() {
        if crate::core_loop::process_request::present_event_window_is_current(state, &event) {
            crate::core_loop::process_request::retire_present_idle(state, backend, &event);
        } else {
            crate::core_loop::process_request::discard_stale_present_event(
                state, backend, &event, true,
            );
        }
    }

    // Refresh every domain that still owns parked work. Epoch-qualified
    // caches preserve old clocks across stable-XID remaps; stale rows fail
    // open against that old cache and are never compared/armed against the
    // replacement physical counter.
    let mut domains: Vec<(u32, u64)> = Vec::new();
    domains.extend(
        state
            .present_pending_msc
            .iter()
            .map(|p| (p.crtc_id, p.crtc_epoch)),
    );
    domains.extend(
        state
            .present_pending_complete
            .iter()
            .map(|p| (p.event.crtc_id, p.event.crtc_epoch)),
    );
    domains.extend(
        state
            .present_pending_exec
            .values()
            .map(|p| (p.pending.crtc_id, p.pending.crtc_epoch)),
    );
    domains.sort_unstable();
    domains.dedup();

    for (crtc_id, crtc_epoch) in domains {
        let epoch_current = backend.present_crtc_clock_epoch(crtc_id) == crtc_epoch;
        let general = if epoch_current {
            crate::core_loop::process_request::refresh_present_crtc_general_clock(
                state, backend, crtc_id, crtc_epoch,
            )
        } else {
            crate::core_loop::process_request::cached_present_crtc_clock(state, crtc_id, crtc_epoch)
        };
        crate::core_loop::process_request::fire_due_present_notify_msc_for_domain(
            state,
            backend,
            crtc_id,
            crtc_epoch,
            general.msc,
            general.ust,
            !epoch_current,
        );
        let completion = crate::core_loop::process_request::refresh_present_crtc_completion_clock(
            state, backend, crtc_id, crtc_epoch, None,
        );
        crate::core_loop::process_request::fire_due_present_completions_for_domain(
            state, backend, crtc_id, crtc_epoch, completion,
        );
    }
}

/// F2: pop every pending host event off the backend and fan it out
/// to nested clients. Runs at the outer-loop boundary so a host
/// request issued inside fanout (CreateWindow forwarding,
/// SetClipRectangles, etc.) cannot recursively re-dispatch — the new
/// request's reply lands in `pending_replies` and the next
/// outer-loop iteration drains anything `wait_for_reply` re-enqueued.
pub(crate) fn dispatch_pending_host_events(
    state: &mut ServerState,
    backend: &mut dyn Backend,
) -> bool {
    let mut any = false;
    while let Some(event) = backend.pop_pending_host_event() {
        any = true;
        // The fanout helpers borrow `xid_map` immutably — clone the
        // map up-front so we can release the immutable borrow on
        // backend before mutating `state`'s per-client outbound
        // buffers. The map is a few hundred entries even on a busy
        // session.
        let xid_map = backend.xid_map().clone();
        match event {
            HostEvent::Pointer(ev) => {
                use crate::core_loop::pointer_fanout::pointer_event_fanout_to_state;
                let _dropped =
                    pointer_event_fanout_to_state(state, backend, &xid_map, ev, true, false);
            }
            HostEvent::Expose(ev) => {
                use crate::core_loop::fanout::expose_event_fanout_to_state;
                let _dropped = expose_event_fanout_to_state(state, &xid_map, ev);
            }
            HostEvent::Key(ev) => {
                use crate::core_loop::key_fanout::key_event_fanout_to_state;
                crate::core_loop::record::record_device_event(
                    state,
                    crate::core_loop::record::RecordedDeviceEvent {
                        event_type: if ev.pressed { 2 } else { 3 },
                        detail: ev.keycode,
                        repeat: false,
                        time: ev.time,
                        root_x: ev.root_x,
                        root_y: ev.root_y,
                        state: ev.state,
                    },
                );
                let _dropped = key_event_fanout_to_state(state, backend, ev);
            }
            HostEvent::Configure(ev) => {
                if backend.window_id() == ev.host_xid {
                    handle_host_container_resize(state, backend, ev);
                }
            }
            HostEvent::Closed => {
                log::info!("host container window destroyed; shutting down");
                // Triggering shutdown via a flag is awkward without
                // sender access here — return Ok from run_core via
                // host_socket_eof check on next iteration.
            }
        }
    }
    any
}

pub(crate) fn handle_host_container_resize(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    ev: crate::host_x11::HostConfigureEvent,
) {
    if ev.width == 0
        || ev.height == 0
        || (state.randr.screen_width == ev.width && state.randr.screen_height == ev.height)
    {
        return;
    }
    let timestamp = state.timestamp_now();
    state.randr.resize(timestamp, ev.width, ev.height);

    // The nested path resizes the single output, so it genuinely changes
    // CRTC geometry — pass the output as changed so CrtcChangeNotify fires.
    let changed: Vec<(u32, u32, u32)> = state
        .randr
        .outputs
        .first()
        .map(|o| (o.output_id, o.crtc_id, o.mode_id))
        .into_iter()
        .collect();
    apply_screen_size_side_effects(state, backend, ev.width, ev.height, &changed);
}

/// Tell RANDR subscribers the layout changed without the screen resizing.
///
/// Xorg treats a primary-output change as a layout change, not a geometry one:
/// `RRSetPrimaryOutput` marks the affected outputs via `RROutputChanged`, sets
/// `layoutChanged`, and calls `RRTellChanged` (randr/rroutput.c), which fans out
/// ScreenChangeNotify plus one OutputChangeNotify per changed output. No
/// CrtcChangeNotify: no CRTC moved. Without this, panels and desktop shells
/// never learn the primary moved, because polling `GetOutputPrimary` is not how
/// they are written — they wait for the notify.
///
/// Screen dimensions come straight from `state.randr`, so this stays correct if
/// it is ever called after a resize.
pub(crate) fn notify_randr_layout_changed(state: &mut ServerState, changed_outputs: &[u32]) {
    use std::sync::atomic::Ordering;
    use yserver_protocol::x11::{SequenceNumber, randr as x11randr};

    const RANDR_FIRST_EVENT: u8 = 89;

    let timestamp = state.randr.timestamp;
    let config_timestamp = state.randr.config_timestamp;
    let (screen_rotation, width, height, width_mm, height_mm) = state.randr.screen_change_fields();
    // Resolve each changed output's current crtc/mode for the notify payload.
    let changed: Vec<(u32, u32, u32, u8, u16)> = changed_outputs
        .iter()
        .filter_map(|id| {
            state
                .randr
                .outputs
                .iter()
                .find(|o| o.output_id == *id)
                .map(|o| {
                    (
                        o.output_id,
                        if o.mode_id != 0 { o.crtc_id } else { 0 },
                        o.mode_id,
                        if o.connected {
                            x11randr::CONNECTION_CONNECTED
                        } else {
                            x11randr::CONNECTION_DISCONNECTED
                        },
                        if o.mode_id != 0 {
                            o.rotation
                        } else {
                            crate::randr::RR_ROTATE_0
                        },
                    )
                })
        })
        .collect();

    let subscribers: Vec<(u32, yserver_protocol::x11::ResourceId, u16)> = state
        .randr_select_masks
        .iter()
        .map(|((owner, window), mask)| (*owner, *window, *mask))
        .collect();
    for (owner, request_window, mask) in subscribers {
        let Some(client) = state.clients.get_mut(&owner) else {
            continue;
        };
        let sequence = SequenceNumber(client.last_sequence.load(Ordering::Relaxed));
        if mask & x11randr::NOTIFY_MASK_SCREEN_CHANGE != 0 {
            let event = x11randr::encode_screen_change_notify_event(
                client.byte_order,
                RANDR_FIRST_EVENT,
                sequence,
                x11randr::ScreenChangeNotify {
                    rotation: screen_rotation,
                    timestamp,
                    config_timestamp,
                    root: crate::resources::ROOT_WINDOW.0,
                    request_window: request_window.0,
                    width,
                    height,
                    width_mm,
                    height_mm,
                },
            );
            crate::core_loop::fanout::record_outbound_telemetry(
                yserver_protocol::x11::ClientId(owner),
                client.byte_order,
                &event,
            );
            let _ = client_io::write_or_buffer(client, &event);
        }
        if mask & x11randr::NOTIFY_MASK_OUTPUT_CHANGE != 0 {
            for &(output, crtc, mode, connection, rotation) in &changed {
                let event = x11randr::encode_output_change_notify_event(
                    client.byte_order,
                    RANDR_FIRST_EVENT,
                    sequence,
                    x11randr::OutputChangeNotify {
                        timestamp,
                        config_timestamp,
                        request_window: request_window.0,
                        output,
                        crtc,
                        mode,
                        rotation,
                        connection,
                    },
                );
                crate::core_loop::fanout::record_outbound_telemetry(
                    yserver_protocol::x11::ClientId(owner),
                    client.byte_order,
                    &event,
                );
                let _ = client_io::write_or_buffer(client, &event);
            }
        }
    }
}

/// Fan out `RRNotify_ProviderChange` after an output-source relationship
/// actually changes. Xorg marks and announces the initiating provider only;
/// the peer's reciprocal association is observable through `GetProviderInfo`
/// without a second event.
pub(crate) fn notify_randr_provider_changed(state: &mut ServerState, provider: u32) {
    use std::sync::atomic::Ordering;
    use yserver_protocol::x11::{SequenceNumber, randr as x11randr};

    const RANDR_FIRST_EVENT: u8 = 89;

    let subscribers: Vec<(u32, yserver_protocol::x11::ResourceId, u16)> = state
        .randr_select_masks
        .iter()
        .map(|((owner, window), mask)| (*owner, *window, *mask))
        .collect();
    for (owner, request_window, mask) in subscribers {
        if mask & x11randr::NOTIFY_MASK_PROVIDER_CHANGE == 0 {
            continue;
        }
        let Some(client) = state.clients.get_mut(&owner) else {
            continue;
        };
        let sequence = SequenceNumber(client.last_sequence.load(Ordering::Relaxed));
        let event = x11randr::encode_provider_change_notify_event(
            client.byte_order,
            RANDR_FIRST_EVENT,
            sequence,
            x11randr::ProviderChangeNotify {
                timestamp: state.randr.timestamp,
                request_window: request_window.0,
                provider,
            },
        );
        crate::core_loop::fanout::record_outbound_telemetry(
            yserver_protocol::x11::ClientId(owner),
            client.byte_order,
            &event,
        );
        let _ = client_io::write_or_buffer(client, &event);
    }
}

/// Fans out `RRNotify_OutputProperty` (randr/rrproperty.c
/// `RRDeliverPropertyEvent`) to every client that selected
/// `NOTIFY_MASK_OUTPUT_PROPERTY` via `RRSelectInput`. Unlike
/// `notify_randr_layout_changed`, this is not gated on
/// `NOTIFY_MASK_SCREEN_CHANGE`/`NOTIFY_MASK_OUTPUT_CHANGE` — property
/// changes are a distinct notify sub-type in the real protocol.
pub(crate) fn notify_randr_output_property_changed(
    state: &mut ServerState,
    output: u32,
    atom: yserver_protocol::x11::AtomId,
    property_state: u8,
) {
    use std::sync::atomic::Ordering;
    use yserver_protocol::x11::{SequenceNumber, randr as x11randr};

    const RANDR_FIRST_EVENT: u8 = 89;

    // Xorg stamps property notifies with the current time and leaves
    // lastSetTime alone (`rrproperty.c:75`): mutter/muffin compare
    // lastSetTime with their own SetCrtcConfig reply to tell their
    // configuration from an external one.
    let timestamp = state.timestamp_now();
    let subscribers: Vec<(u32, yserver_protocol::x11::ResourceId, u16)> = state
        .randr_select_masks
        .iter()
        .map(|((owner, window), mask)| (*owner, *window, *mask))
        .collect();
    for (owner, request_window, mask) in subscribers {
        if mask & x11randr::NOTIFY_MASK_OUTPUT_PROPERTY == 0 {
            continue;
        }
        let Some(client) = state.clients.get_mut(&owner) else {
            continue;
        };
        let sequence = SequenceNumber(client.last_sequence.load(Ordering::Relaxed));
        let event = x11randr::encode_output_property_notify_event(
            client.byte_order,
            RANDR_FIRST_EVENT,
            sequence,
            x11randr::OutputPropertyNotify {
                request_window: request_window.0,
                output,
                atom: atom.0,
                timestamp,
                state: property_state,
            },
        );
        crate::core_loop::fanout::record_outbound_telemetry(
            yserver_protocol::x11::ClientId(owner),
            client.byte_order,
            &event,
        );
        let _ = client_io::write_or_buffer(client, &event);
    }
}

/// Common side-effects of a logical-screen-size change: update root +
/// overlay window records, emit ConfigureNotify / Present ConfigureNotify,
/// fan out RANDR notifies (ScreenChange always; Crtc/Output only for entries
/// in `changed`), and re-clamp/warp the pointer. `changed` is empty for a pure
/// RRSetScreenSize (CRTCs unchanged).
pub(crate) fn apply_screen_size_side_effects(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    width: u16,
    height: u16,
    changed: &[(u32, u32, u32)],
) {
    if let Some(root) = state.resources.window_mut(crate::resources::ROOT_WINDOW) {
        root.width = width;
        root.height = height;
    }
    if let Some(overlay) = state
        .resources
        .window_mut(crate::resources::COMPOSITE_OVERLAY_WINDOW)
    {
        overlay.width = width;
        overlay.height = height;
    }

    emit_screen_resize_window_notifications(state, width, height);
    emit_randr_change_notifications(state, changed);

    // Pointer: clamp into [0,w)×[0,h); if the screen shrank below the
    // current cursor position, warp it inside (Xorg
    // RRPointerScreenConfigured / ScreenRestructured). The KMS motion
    // clamp only applies on the NEXT motion, so the explicit warp is
    // required to avoid a stranded off-screen cursor.
    let (px, py) = state.pointer_root;
    let cx = i32::from(px).clamp(0, i32::from(width.saturating_sub(1)));
    let cy = i32::from(py).clamp(0, i32::from(height.saturating_sub(1)));
    if cx != i32::from(px) || cy != i32::from(py) {
        let prev = state.barrier_bypass;
        state.barrier_bypass = true;
        backend.warp_pointer_root(state, cx, cy);
        state.barrier_bypass = prev;
    }
}

/// Emit the window-size notifications that clients normally get from
/// `ConfigureWindow`, for screen-sized server windows that RandR resizes
/// out-of-band. This intentionally does not mutate the resource geometry:
/// callers must update root/COW first, then use this to wake clients that
/// cache drawable or Present buffer sizes.
pub(crate) fn emit_screen_resize_window_notifications(
    state: &mut ServerState,
    width: u16,
    height: u16,
) {
    use yserver_protocol::x11;

    let root_geometry = x11::Geometry {
        root: crate::resources::ROOT_WINDOW,
        x: 0,
        y: 0,
        width,
        height,
        border_width: 0,
        depth: 24,
    };

    // Core ConfigureNotify on root for non-RANDR-aware clients
    // selecting StructureNotifyMask. Spec-correct ordering: emit this
    // before the RANDR fanout so non-RANDR-aware clients (panels,
    // "fill the screen" apps) reflow at the same point in the event
    // stream that RANDR-aware toolkits see screen-change.
    let _dropped = crate::core_loop::fanout::emit_window_event_to_state(
        state,
        crate::resources::ROOT_WINDOW,
        0x0002_0000, // StructureNotifyMask
        |buf, seq, order| {
            x11::encode_configure_notify_event(
                buf,
                seq,
                order,
                crate::resources::ROOT_WINDOW,
                crate::resources::ROOT_WINDOW,
                None,
                root_geometry,
                false,
            );
        },
    );
    fire_present_configure_notify_for_window(state, crate::resources::ROOT_WINDOW, root_geometry);

    if let Some((parent, geometry, override_redirect)) = state
        .resources
        .window(crate::resources::COMPOSITE_OVERLAY_WINDOW)
        .map(|overlay| {
            (
                overlay.parent,
                x11::Geometry {
                    root: crate::resources::ROOT_WINDOW,
                    x: overlay.x,
                    y: overlay.y,
                    width: overlay.width,
                    height: overlay.height,
                    border_width: overlay.border_width,
                    depth: overlay.depth,
                },
                overlay.override_redirect,
            )
        })
    {
        let above_sibling = state
            .resources
            .configure_notify_above_sibling(crate::resources::COMPOSITE_OVERLAY_WINDOW);
        let _dropped = crate::core_loop::fanout::emit_window_event_to_state(
            state,
            crate::resources::COMPOSITE_OVERLAY_WINDOW,
            0x0002_0000, // StructureNotifyMask
            |buf, seq, order| {
                x11::encode_configure_notify_event(
                    buf,
                    seq,
                    order,
                    crate::resources::COMPOSITE_OVERLAY_WINDOW,
                    crate::resources::COMPOSITE_OVERLAY_WINDOW,
                    above_sibling,
                    geometry,
                    override_redirect,
                );
            },
        );
        let _dropped = crate::core_loop::fanout::emit_window_event_to_state(
            state,
            parent,
            0x0008_0000, // SubstructureNotifyMask
            |buf, seq, order| {
                x11::encode_configure_notify_event(
                    buf,
                    seq,
                    order,
                    parent,
                    crate::resources::COMPOSITE_OVERLAY_WINDOW,
                    above_sibling,
                    geometry,
                    override_redirect,
                );
            },
        );
        fire_present_configure_notify_for_window(
            state,
            crate::resources::COMPOSITE_OVERLAY_WINDOW,
            geometry,
        );
    }
}

/// `RRSetCrtcConfig` can complete the physical modeset after a compositor
/// already issued `RRSetScreenSize` and received its immediate configure
/// notifications. When the active-output bbox then changes and catches up
/// with the logical screen, re-emit root/COW notifications so clients observe
/// the size again after the modeset. An unchanged bbox (for example, a
/// refresh-rate-only change) needs no window-size notification.
pub(crate) fn emit_screen_resize_window_notifications_if_outputs_caught_up(
    state: &mut ServerState,
    previous_bbox: Option<(u16, u16)>,
) {
    let Some((bbox_w, bbox_h)) = enabled_output_bbox(state) else {
        return;
    };
    if previous_bbox != Some((bbox_w, bbox_h))
        && bbox_w == state.randr.screen_width
        && bbox_h == state.randr.screen_height
    {
        emit_screen_resize_window_notifications(state, bbox_w, bbox_h);
    }
}

pub(crate) fn enabled_output_bbox(state: &ServerState) -> Option<(u16, u16)> {
    let mut any = false;
    let mut max_x = 0i32;
    let mut max_y = 0i32;
    for output in state.randr.outputs.iter().filter(|o| o.mode_id != 0) {
        any = true;
        let (width, height) = output.footprint();
        max_x = max_x.max(i32::from(output.x).saturating_add(i32::from(width)));
        max_y = max_y.max(i32::from(output.y).saturating_add(i32::from(height)));
    }
    any.then(|| {
        (
            u16::try_from(max_x.max(0)).unwrap_or(u16::MAX),
            u16::try_from(max_y.max(0)).unwrap_or(u16::MAX),
        )
    })
}

/// Fan out RANDR change notifications for a topology/geometry change.
pub fn emit_randr_change_notifications(state: &mut ServerState, changed: &[(u32, u32, u32)]) {
    emit_randr_change_notifications_split(state, changed, changed);
}

/// Fan out a connector-registry change while allowing Output-only changes to
/// remain distinct from changes to current CRTC assignment or geometry.
/// The dirty sets are independent: a recompact can move a surviving CRTC
/// without changing its output association, while a mode-list or connection
/// refresh can dirty only an Output.
pub fn emit_randr_connector_change_notifications(
    state: &mut ServerState,
    crtc_changed: &[(u32, u32, u32)],
    output_changed: &[(u32, u32, u32)],
) {
    emit_randr_change_notifications_split(state, crtc_changed, output_changed);
}

fn emit_randr_change_notifications_split(
    state: &mut ServerState,
    crtc_changed: &[(u32, u32, u32)],
    output_changed: &[(u32, u32, u32)],
) {
    use std::sync::atomic::Ordering;
    use yserver_protocol::x11::{SequenceNumber, randr as x11randr};

    const RANDR_FIRST_EVENT: u8 = 89;

    let timestamp = state.randr.timestamp;
    let config_timestamp = state.randr.config_timestamp;
    let (screen_rotation, width, height, width_mm, height_mm) = state.randr.screen_change_fields();
    // Per-CRTC geometry (position AND mode size). CrtcChangeNotify must
    // report each CRTC's own mode dimensions — NOT the logical screen
    // size — or a multi-monitor client sees every CRTC as e.g. 5120×1440
    // instead of its real 2560×1440. An off CRTC (no mode) reports 0×0.
    let crtc_geom: std::collections::HashMap<u32, (i16, i16, u16, u16, u16)> = state
        .randr
        .outputs
        .iter()
        .map(|o| (o.crtc_id, (o.x, o.y, o.width, o.height, o.rotation)))
        .collect();
    let output_states: std::collections::HashMap<u32, (u8, u32, u16)> = state
        .randr
        .outputs
        .iter()
        .map(|output| {
            (
                output.output_id,
                (
                    if output.connected {
                        x11randr::CONNECTION_CONNECTED
                    } else {
                        x11randr::CONNECTION_DISCONNECTED
                    },
                    if output.mode_id != 0 {
                        output.crtc_id
                    } else {
                        0
                    },
                    if output.mode_id != 0 {
                        output.rotation
                    } else {
                        crate::randr::RR_ROTATE_0
                    },
                ),
            )
        })
        .collect();

    let subscribers: Vec<(u32, yserver_protocol::x11::ResourceId, u16)> = state
        .randr_select_masks
        .iter()
        .map(|((owner, window), mask)| (*owner, *window, *mask))
        .collect();
    for (owner, request_window, mask) in subscribers {
        let Some(client) = state.clients.get_mut(&owner) else {
            continue;
        };
        let sequence = SequenceNumber(client.last_sequence.load(Ordering::Relaxed));
        if mask & x11randr::NOTIFY_MASK_SCREEN_CHANGE != 0 {
            let event = x11randr::encode_screen_change_notify_event(
                client.byte_order,
                RANDR_FIRST_EVENT,
                sequence,
                x11randr::ScreenChangeNotify {
                    rotation: screen_rotation,
                    timestamp,
                    config_timestamp,
                    root: crate::resources::ROOT_WINDOW.0,
                    request_window: request_window.0,
                    width,
                    height,
                    width_mm,
                    height_mm,
                },
            );
            crate::core_loop::fanout::record_outbound_telemetry(
                yserver_protocol::x11::ClientId(owner),
                client.byte_order,
                &event,
            );
            let _ = client_io::write_or_buffer(client, &event);
        }
        // Xorg fans out all dirty CRTCs before all dirty outputs for each
        // subscriber; do not interleave the two event classes per output.
        if mask & x11randr::NOTIFY_MASK_CRTC_CHANGE != 0 {
            for &(_output, crtc, mode) in crtc_changed {
                let (x, y, crtc_w, crtc_h, rotation) = crtc_geom.get(&crtc).copied().unwrap_or((
                    0,
                    0,
                    0,
                    0,
                    crate::randr::RR_ROTATE_0,
                ));
                let event = x11randr::encode_crtc_change_notify_event(
                    client.byte_order,
                    RANDR_FIRST_EVENT,
                    sequence,
                    x11randr::CrtcChangeNotify {
                        timestamp,
                        request_window: request_window.0,
                        crtc,
                        mode,
                        rotation,
                        x,
                        y,
                        width: crtc_w,
                        height: crtc_h,
                    },
                );
                crate::core_loop::fanout::record_outbound_telemetry(
                    yserver_protocol::x11::ClientId(owner),
                    client.byte_order,
                    &event,
                );
                let _ = client_io::write_or_buffer(client, &event);
            }
        }
        if mask & x11randr::NOTIFY_MASK_OUTPUT_CHANGE != 0 {
            for &(output, projected_crtc, projected_mode) in output_changed {
                let (connection, current_crtc, rotation) =
                    output_states.get(&output).copied().unwrap_or((
                        x11randr::CONNECTION_CONNECTED,
                        projected_crtc,
                        crate::randr::RR_ROTATE_0,
                    ));
                let event = x11randr::encode_output_change_notify_event(
                    client.byte_order,
                    RANDR_FIRST_EVENT,
                    sequence,
                    x11randr::OutputChangeNotify {
                        timestamp,
                        config_timestamp,
                        request_window: request_window.0,
                        output,
                        crtc: current_crtc,
                        mode: projected_mode,
                        rotation,
                        connection,
                    },
                );
                crate::core_loop::fanout::record_outbound_telemetry(
                    yserver_protocol::x11::ClientId(owner),
                    client.byte_order,
                    &event,
                );
                let _ = client_io::write_or_buffer(client, &event);
            }
        }
    }
}

/// I2: re-arm `WRITABLE` interest on each client's writer fd to track
/// `outbound` state. Called once per outer poll iteration so per-event
/// processing doesn't have to thread the registry through every
/// fanout helper.
/// Drain any buffered outbound, then reconcile each client's poller
/// interest with whether it still has bytes pending. Returns the ids of
/// clients whose drain attempts surfaced peer-gone errors so the caller
/// can run `process_disconnect`.
///
/// The proactive drain is load-bearing: mio uses edge-triggered epoll on
/// Linux, so when `write_or_buffer` partial-writes and buffers the tail,
/// the kernel can transition the fd writable *before* this function
/// re-registers WRITABLE interest. Without an immediate drain attempt
/// we'd register for an edge that has already passed and the buffered
/// tail would never go out — clients see truncated replies and stall.
fn reconcile_client_writable_interest(
    registry: &mio::Registry,
    state: &mut ServerState,
) -> Vec<yserver_protocol::x11::ClientId> {
    let mut to_disconnect = Vec::new();
    for (id, client) in state.clients.iter_mut() {
        if !client.outbound.is_empty() {
            match client_io::drain_outbound(client) {
                Ok(WriteOutcome::Done | WriteOutcome::WouldBlock) => {}
                Ok(WriteOutcome::Disconnect) | Err(_) => {
                    to_disconnect.push(yserver_protocol::x11::ClientId(*id));
                    continue;
                }
            }
        }
        let needs_writable = !client.outbound.is_empty();
        if needs_writable == client.watching_writable {
            continue;
        }
        let raw = std::os::fd::AsRawFd::as_raw_fd(&*client.writer.lock().unwrap());
        let interest = if needs_writable {
            Interest::READABLE | Interest::WRITABLE
        } else {
            Interest::READABLE
        };
        match registry.reregister(
            &mut SourceFd(&raw),
            client_token(yserver_protocol::x11::ClientId(*id)),
            interest,
        ) {
            Ok(()) => client.watching_writable = needs_writable,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // fd already deregistered (disconnect path); nothing
                // to track.
            }
            Err(err) => {
                warn!("reregister client {} writable interest: {err}", id);
            }
        }
    }
    to_disconnect
}

fn handle_setup_allocate(
    state: &mut ServerState,
    id: yserver_protocol::x11::ClientId,
    response_tx: crossbeam_channel::Sender<SetupAllocateResponse>,
) {
    let _ = id;
    let response = match state.id_allocator.allocate() {
        Some((base, mask)) => SetupAllocateResponse {
            resource_id_base: base,
            resource_id_mask: mask,
            screen_width_px: state.randr.screen_width,
            screen_height_px: state.randr.screen_height,
            screen_width_mm: u16::try_from(state.randr.width_mm).unwrap_or(u16::MAX),
            screen_height_mm: u16::try_from(state.randr.height_mm).unwrap_or(u16::MAX),
            current_input_masks: state
                .clients
                .values()
                .filter_map(|c| c.event_masks.get(&crate::resources::ROOT_WINDOW).copied())
                .fold(0u32, |a, b| a | b),
        },
        None => SetupAllocateResponse {
            resource_id_base: 0,
            resource_id_mask: 0,
            screen_width_px: 0,
            screen_height_px: 0,
            screen_width_mm: 0,
            screen_height_mm: 0,
            current_input_masks: 0,
        },
    };
    let _ = response_tx.send(response);
}

/// Process a real host input event: arm/refresh/clear the auto-repeat
/// timer before fanning the event out via `backend.on_host_input`.
///
/// This is the single entry point for input that originated from a
/// user (libinput, host-X11 forwarded events, XTEST). Synthetic
/// release+press pairs emitted by [`fire_pending_repeats`] must NOT
/// route through here — they call `backend.on_host_input` directly so
/// the synthetic release doesn't re-enter [`update_repeat_state`] and
/// clear the armed key.
///
/// Public so backend-owned input dispatch paths can route through the
/// repeat-state wrapper instead of calling `backend.on_host_input`
/// directly.
pub fn handle_host_input(state: &mut ServerState, backend: &mut dyn Backend, ev: HostInputEvent) {
    update_repeat_state(state, &ev);
    backend.on_host_input(state, ev);
}

/// Arm / refresh / clear repeat state for the origin of an incoming host
/// input event. Each keyboard view has one current repeat key, matching
/// Xorg's per-device XKB repeat state: a key press from another keyboard
/// cannot replace it. A matching release clears that origin's timer.
fn update_repeat_state(state: &mut ServerState, ev: &HostInputEvent) {
    use crate::core_loop::{
        InputOrigin,
        message::HostInputEvent::{DeviceRemoved, DeviceSuspended, Key},
    };
    let Key(key) = ev else {
        let source_id = match ev {
            DeviceRemoved { source_id } | DeviceSuspended { source_id } => *source_id,
            _ => return,
        };
        state.key_repeats.remove(&InputOrigin::Physical(source_id));
        return;
    };
    if !crate::core_loop::key_fanout::keyboard_origin_is_live(state, key.origin) {
        state.key_repeats.remove(&key.origin);
        return;
    }
    if key.pressed {
        if crate::core_loop::key_fanout::keyboard_key_is_down(state, key.origin, key.keycode) {
            // A duplicate press is not a new XKB press and does not restart
            // this origin's repeat delay.
            return;
        }
        // ChangeKeyboardControl gate: global auto-repeat off disables all
        // repeat; otherwise the per-key bitmap decides. A non-repeating key
        // replaces the current repeat key for this origin only.
        if !state.keyboard_control.key_auto_repeats(key.keycode) {
            state.key_repeats.remove(&key.origin);
            return;
        }
        state.key_repeats.insert(
            key.origin,
            KeyRepeatState {
                event: *key,
                next_fire: Instant::now() + REPEAT_INITIAL_DELAY,
            },
        );
    } else if state
        .key_repeats
        .get(&key.origin)
        .is_some_and(|repeat| repeat.event.keycode == key.keycode)
    {
        state.key_repeats.remove(&key.origin);
    }
}

/// Fire any auto-repeat events whose `next_fire` has elapsed. Loops
/// in case the poll wake was delayed past more than one period
/// (under load) so we don't drop events. Each fire emits a
/// KeyRelease + KeyPress pair through the same host-input fan-out
/// path the original press took, matching classic X11 auto-repeat
/// (every client handles it without opting into XKB
/// DetectableAutoRepeat).
/// Returns `true` iff a repeat was actually fanned out this call. The
/// caller uses this to decide whether to poke the compositor: a call
/// that merely observes an armed-but-not-yet-due key (or disarms a
/// no-longer-repeating one) produces no events and must NOT re-dirty
/// the scene — doing so unconditionally every loop iteration while a
/// key is held (or while a phantom key is stuck armed) busy-spins the
/// compositor at the iteration rate instead of the repeat rate
/// (idle free-run, [[project_idle_compositor_redraw_loop]] cut 2a).
pub fn fire_pending_repeats(state: &mut ServerState, backend: &mut dyn Backend) -> bool {
    let mut origins: Vec<_> = state.key_repeats.keys().copied().collect();
    origins.sort_by_key(|origin| match origin {
        crate::core_loop::InputOrigin::Physical(source) => (0, source.0),
        crate::core_loop::InputOrigin::XTest(device_id) => (1, u64::from(*device_id)),
        crate::core_loop::InputOrigin::NestedHost => (2, 0),
    });

    // A drained source may lose its held key before its timer fires. Do not
    // synthesize a new press for that key; the origin's held set is the
    // authority for whether repeat remains active.
    let stale_origins: Vec<_> = origins
        .iter()
        .copied()
        .filter(|origin| {
            let Some(armed) = state.key_repeats.get(origin) else {
                return true;
            };
            !crate::core_loop::key_fanout::keyboard_origin_is_live(state, *origin)
                || !crate::core_loop::key_fanout::keyboard_key_is_down(
                    state,
                    *origin,
                    armed.event.keycode,
                )
                || !state.keyboard_control.key_auto_repeats(armed.event.keycode)
        })
        .collect();
    for origin in stale_origins {
        state.key_repeats.remove(&origin);
    }

    let now = Instant::now();
    let due: Vec<_> = origins
        .into_iter()
        .filter_map(|origin| {
            state
                .key_repeats
                .get(&origin)
                .filter(|armed| armed.next_fire <= now)
                .copied()
                .map(|armed| (origin, armed))
        })
        .collect();
    if due.is_empty() {
        return false;
    }

    for (origin, armed) in due {
        let mut next_fire = armed.next_fire;
        while now >= next_fire {
            next_fire += REPEAT_PERIOD;
        }
        // Update the timer first so any reentrant arming during fan-out
        // doesn't double-fire.
        if let Some(repeat) = state.key_repeats.get_mut(&origin) {
            repeat.next_fire = next_fire;
        }
        let mut release = armed.event;
        release.pressed = false;
        let mut press = armed.event;
        press.pressed = true;
        backend.on_host_input(state, HostInputEvent::KeyRepeat(release));
        backend.on_host_input(state, HostInputEvent::KeyRepeat(press));
    }
    true
}

/// Post-poll screen-saver evaluator. Drives idle activation and the
/// periodic Cycle event re-fire. Extracted from the outer loop body
/// so unit tests can drive it directly with pre-armed state.
///
/// Mirrors the DPMS cascade evaluator above it in the loop:
/// compute the deadline, check `now >= deadline`, drive the helper.
// nested-if matches the DPMS evaluator's shape for readability symmetry
#[allow(clippy::collapsible_if)]
pub(crate) fn evaluate_screen_saver_post_poll(state: &mut ServerState, backend: &mut dyn Backend) {
    // SS: idle activation. Mirrors Xorg WaitFor.c:441 timing.
    // `screensaver_idle_deadline` returns None when DPMS is blanked
    // (power_level != 0), so this branch is already suppressed under
    // DPMS blanking — Xorg WaitFor.c:457 parity.
    if let Some(deadline) = state.screensaver_idle_deadline() {
        if Instant::now() >= deadline {
            crate::core_loop::process_request::apply_screen_saver_transition(
                state,
                backend,
                crate::server::ScreenSaverActive::On,
                /*forced=*/ false,
            );
        }
    }
    // SS: cycle re-fire. Mirrors Xorg WaitFor.c:470-476.
    if let Some(deadline) = state.screensaver_cycle_deadline() {
        let now = Instant::now();
        if now >= deadline {
            crate::core_loop::process_request::emit_screen_saver_notify(
                state,
                crate::server::ScreenSaverActive::Cycle,
                /*forced=*/ false,
            );
            state.screensaver.next_cycle =
                Some(now + Duration::from_millis(u64::from(state.screensaver.interval_ms)));
        }
    }
}

/// Post-poll IDLETIME alarm evaluator. For each IDLETIME counter,
/// compute the current idle, walk Active alarms referencing the
/// counter, run the test-type check against the cached
/// `(last_evaluated, current)` pair, and fire via
/// `evaluate_alarms_for_counter` (which handles re-arm + emission).
/// Mirrors Xorg's `IdleTimeBlockHandler` + `IdleTimeWakeupHandler`
/// (sync.c:2647, :2750).
pub(crate) fn evaluate_idletime_alarms_post_poll(
    state: &mut ServerState,
    _backend: &mut dyn crate::backend::Backend,
) {
    use yserver_protocol::x11::sync as x11sync;
    // Suspend gate (Xorg WaitFor.c:519 unified-timer rule) — mirrors
    // `idletime_alarm_deadline`. Skip the whole evaluator when any
    // client holds XScreenSaverSuspend; otherwise an unrelated wake
    // could still fire Positive alarms mid-fullscreen-video.
    if !state.screensaver.suspend_counts.is_empty() {
        return;
    }
    const IDLETIME_COUNTERS: &[u32] = &[
        x11sync::IDLETIME_COUNTER,
        x11sync::IDLETIME_DEVICE_VCP,
        x11sync::IDLETIME_DEVICE_VCK,
    ];
    let now = Instant::now();
    for &counter in IDLETIME_COUNTERS {
        // Skip if no alarm or await references this counter.
        let has_alarm = state
            .sync_alarms
            .values()
            .any(|a| a.counter == counter && a.state == x11sync::ALARM_STATE_ACTIVE);
        if !has_alarm && !crate::core_loop::sync_await::idletime_awaited(state, counter) {
            continue;
        }
        let baseline = state.idletime_baseline(counter);
        #[allow(clippy::cast_possible_truncation)]
        let current_idle = now
            .duration_since(baseline)
            .as_millis()
            .min(u128::from(u32::MAX)) as i64;
        let old_idle = state
            .idletime_last_evaluated
            .get(&counter)
            .copied()
            .unwrap_or(0);
        // Run the existing evaluator helper — it walks Active alarms,
        // calls trigger_fires, applies the Task 2 state-transition fix,
        // emits AlarmNotify, and updates wait_value.
        // Record the new value first: firing an await can re-enter the
        // IDLETIME bookkeeping through a fresh await's baseline.
        state.idletime_last_evaluated.insert(counter, current_idle);
        crate::core_loop::sync_await::counter_changed(state, counter, old_idle, current_idle);
    }
}

/// Wire a freshly-completed setup handshake into the core's bookkeeping:
///   - try_clone the stream for the writer (set non-blocking on the
///     core's clone)
///   - build the (`reader_control_tx`, `reader_control_rx`) channel
///   - install a `ClientState` for `id`
///   - drop the entry from the setup-thread teardown registry (the
///     setup thread is exiting)
///   - register the writer fd with the poller (no interest yet — I2
///     re-registers `WRITABLE` only when there's pending outbound)
///   - spawn the reader thread (the only path that produces
///     `Message::Request` for this client)
#[allow(clippy::too_many_arguments)]
fn handle_client_setup_complete(
    registry: &mio::Registry,
    sender: &CoreSender,
    setup_registry: &SetupRegistry,
    state: &mut ServerState,
    id: yserver_protocol::x11::ClientId,
    generation: crate::core_loop::Generation,
    stream: Transport,
    resource_id_base: u32,
    resource_id_mask: u32,
    byte_order: yserver_protocol::x11::ClientByteOrder,
    is_local: bool,
    fd_passing: bool,
    setup_reply: &[u8],
) -> io::Result<()> {
    use std::sync::{Arc, Mutex, atomic::AtomicU16};
    let writer = stream.try_clone()?;
    writer.set_nonblocking(true)?;
    let writer_fd = writer.as_raw_fd();

    let (reader_control_tx, reader_control_rx) = crossbeam_channel::unbounded();

    state.clients.insert(
        id.0,
        crate::server::ClientState {
            writer: Arc::new(Mutex::new(writer)),
            byte_order,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base,
            resource_id_mask,
            event_masks: std::collections::HashMap::new(),
            save_set: std::collections::HashSet::new(),
            big_requests_enabled: false,
            xi2_masks: std::collections::HashMap::new(),
            xi1_event_classes: std::collections::HashSet::new(),
            xi1_window_event_classes: std::collections::HashMap::new(),
            outbound: std::collections::VecDeque::new(),
            watching_writable: false,
            focused_window: crate::resources::ROOT_WINDOW,
            reader_control: Some(reader_control_tx),
            is_local,
            fd_passing,
        },
    );
    setup_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);

    // Initial interest is READABLE — mio doesn't accept empty interest.
    // I2 reregisters WRITABLE-only when `client.outbound` becomes
    // non-empty and back to READABLE when it drains. The reader thread
    // already polls the peer fd directly, so this registration's only
    // wake-up role today is the eventual WRITABLE-on-drain edge.
    registry.register(
        &mut SourceFd(&writer_fd),
        crate::core_loop::poll_tokens::client_token(id),
        Interest::READABLE,
    )?;

    const BIG_REQUESTS_MAJOR_OPCODE: u8 = 135;
    // Read the transport off the stream before it is moved into the
    // reader. It has to be the transport, not `is_local`: locality is a
    // property of the ADDRESS, so a loopback TCP client is local and
    // would otherwise be reported as "unix" on the same line that says
    // fd passing is off — misleading exactly where same-host XDMCP is
    // being debugged. The branch already had to learn this distinction
    // once, for authorization.
    let transport_label = match &stream {
        Transport::Unix(_) => "unix",
        Transport::Tcp(_) => "TCP",
        #[cfg(test)]
        Transport::Capture(_) => "test capture",
    };
    // The reader inherits the setup thread's binding rather than
    // re-reading the counter, so the connection keeps ONE generation
    // from accept to disconnect.
    crate::core_loop::client_reader::spawn(
        id,
        stream,
        byte_order,
        BIG_REQUESTS_MAJOR_OPCODE,
        reader_control_rx,
        sender.bind_to(generation),
    )?;

    // At INFO deliberately: this is the only place a log says which
    // transport a client arrived on. In an XDMCP deployment that is the
    // first question worth asking, because it decides whether DRI3 and
    // MIT-SHM were available to that client at all — and it should not
    // require raising the log level of a whole session to find out.
    log::info!(
        "client {} established over {} (fd passing {})",
        id.0,
        transport_label,
        if fd_passing { "on" } else { "off" },
    );
    // Xorg's ClientStateRunning callback: FutureClients contexts take the
    // client on, and enabled ones record its setup reply.
    crate::core_loop::record::client_started(state, id, setup_reply);

    // Reaching here is what "ESTABLISHED" means: the poller registration
    // and the reader spawn have both succeeded, so the client can
    // actually participate in the loop. The reset trigger is NOT armed
    // here — see the note at the end of this function — but this is the
    // point the caller's arming decision is about.
    //
    // Xorg's equivalent is `client->clientState = ClientStateRunning`
    // (`dix/dispatch.c:3762`), set only after the setup reply is written
    // and establishment has fully succeeded; `CloseDownClient`
    // (`:3537`) then triggers the last-client reset only for a client
    // that reached Running. A client of ours whose `register` or
    // `spawn` fails cannot participate in the core loop at all — it
    // produces no request and no reader thread — so it is not the
    // analogue of Running. Arming at the insert instead would let the
    // caller's own error path — which disconnects a failed setup — fire a
    // reset for a client that never ran.
    //
    // Everything that ends before this line must arm nothing: a port
    // scan on the TCP listener, a handshake that drops half-way, a
    // connection refused for a bad cookie, and a failed registration or
    // reader spawn.
    //
    // Arming itself is the CALLER's, deliberately. Under XDMCP a setup
    // can complete and then immediately lose a `Refuse` race, and the
    // service disconnects it as orphaned. Arming here would let that
    // drop drain an armed client set and schedule a generation — a
    // spurious reset in the middle of the negotiation's own retry. Only
    // a RETAINED client may arm, so the decision has to sit after
    // XDMCP admission.
    Ok(())
}

/// Is `peer` one of this machine's own addresses?
///
/// Xorg's `xtransLocalClient` (`os/access.c`) treats an AF_UNIX peer as
/// local, and otherwise compares the peer against `selfhosts` — the
/// addresses `DefineSelf` collected from the interfaces. So a TCP
/// connection from the machine's own address is a LOCAL client there, and
/// keeps the locality-gated extensions.
///
/// Queried per accept rather than snapshotted at startup: accepts are
/// rare, `getifaddrs` is cheap, and a cached set goes stale across a
/// hotplug or a DHCP renewal. Xorg snapshots and then patches with
/// `AugmentSelf`; asking each time is simpler and cannot drift.
///
/// Not implemented: Xorg additionally treats a client whose command name
/// is `ssh` as non-local, to catch a forwarded connection. That is a
/// heuristic on `/proc`, and `ssh -X` reaches us over a UNIX socket
/// anyway.
fn address_is_ours(peer: std::net::IpAddr) -> bool {
    if peer.is_loopback() {
        return true;
    }
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `ifap` with an owned list on success; we
    // walk it without retaining anything and free it before returning.
    if unsafe { libc::getifaddrs(&raw mut ifap) } != 0 {
        return false;
    }
    let mut found = false;
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list `getifaddrs` just built, and
        // `ifa_addr` is either null or a valid `sockaddr` for its family.
        let addr = unsafe { (*cur).ifa_addr };
        if !addr.is_null() && unsafe { (*addr).sa_family } == libc::AF_INET as libc::sa_family_t {
            let sin = addr.cast::<libc::sockaddr_in>();
            // SAFETY: family said AF_INET, so the node is a sockaddr_in.
            let raw = unsafe { (*sin).sin_addr.s_addr };
            if std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(raw))) == peer {
                found = true;
                break;
            }
        }
        // SAFETY: as above; `ifa_next` is null at the end of the list.
        cur = unsafe { (*cur).ifa_next };
    }
    // SAFETY: `ifap` is exactly what `getifaddrs` returned and is freed once.
    unsafe { libc::freeifaddrs(ifap) };
    found
}

/// Accept at most this many connections per listener and core iteration.
const ACCEPT_BUDGET: usize = 16;

/// Preserve readiness across budget-limited accepts, and rotate the first
/// listener served each iteration independently of the poller's event order.
struct ListenerReadiness {
    ready: Vec<bool>,
    next: usize,
}

impl ListenerReadiness {
    fn new(count: usize) -> Self {
        Self {
            ready: vec![false; count],
            next: 0,
        }
    }

    fn mark_ready(&mut self, index: usize) {
        if let Some(ready) = self.ready.get_mut(index) {
            *ready = true;
        }
    }

    fn has_pending(&self) -> bool {
        self.ready.iter().any(|ready| *ready)
    }

    fn accept_ready(
        &mut self,
        listeners: &[Listener],
        allocator: &ClientIdAllocator,
        sender: &CoreSender,
        registry: &SetupRegistry,
        auth: &Arc<AuthState>,
    ) {
        let mut first = None;
        for offset in 0..listeners.len() {
            let index = (self.next + offset) % listeners.len();
            if self.ready[index] {
                first.get_or_insert(index);
                self.ready[index] =
                    accept_pending(&listeners[index], allocator, sender, registry, auth);
            }
        }
        if let Some(first) = first {
            self.next = (first + 1) % listeners.len();
        }
    }
}

/// Accept one bounded batch, returning whether readiness must be retained.
/// mio is edge-triggered: after hitting the budget, keep polling this listener
/// without blocking until an accept reaches WouldBlock.
fn accept_pending(
    listener: &Listener,
    client_id_allocator: &ClientIdAllocator,
    sender: &CoreSender,
    registry: &SetupRegistry,
    auth: &Arc<AuthState>,
) -> bool {
    for _ in 0..ACCEPT_BUDGET {
        let accepted = match listener {
            Listener::Unix(listener) => listener
                .accept()
                .map(|(stream, _)| (Transport::Unix(stream), true, true)),
            Listener::Tcp(listener) => listener.accept().map(|(stream, peer)| {
                // `is_local` is an ADDRESS property, `fd_passing` is a
                // TRANSPORT one, and this is the site that must not
                // conflate them. `SCM_RIGHTS` is impossible over TCP
                // whoever the peer is, so fd passing is always off here.
                // Locality is not: Xorg's `xtransLocalClient`
                // (`os/access.c`) answers TRUE for a TCP peer whose
                // address is one of the server's own, which is why a
                // same-machine client keeps MIT-SHM — its `Attach` passes
                // a SysV shmid, an integer on the wire, so shared memory
                // works fine without a descriptor.
                (Transport::Tcp(stream), address_is_ours(peer.ip()), false)
            }),
        };
        match accepted {
            Ok((stream, is_local, fd_passing)) => {
                let id = client_id_allocator.allocate();
                // Bind the connection's producer HERE, at accept: this
                // is the moment that decides which session the client
                // belongs to. Everything it later sends — its setup
                // thread's messages, and its reader thread's, which
                // inherit this binding — is tagged with the generation
                // running now, so a reset retires all of it even if the
                // thread only wakes up on the far side of the boundary.
                if let Err(err) = setup_thread::spawn(
                    id,
                    stream,
                    sender.bind(),
                    registry.clone(),
                    auth.clone(),
                    is_local,
                    fd_passing,
                ) {
                    error!("setup thread spawn failed for client {}: {err}", id.0);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return false,
            // These do not mean the accept queue is empty. Count failed
            // syscalls toward the budget too, so even repeated errors yield.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
                ) => {}
            Err(err) => {
                warn!("accept failed: {err}");
                return false;
            }
        }
    }
    true
}

// Silence unused-import lints when the listener path is only exercised
// indirectly. Concrete uses below.
#[allow(dead_code)]
fn _hint(_: Transport) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_xi_config_test_client(
        state: &mut ServerState,
        client_id: u32,
    ) -> crate::transport::CapturedPeer {
        use crate::server::ClientState;
        use std::sync::{Arc, Mutex, atomic::AtomicU16};
        use yserver_protocol::x11::ClientByteOrder;

        let (transport, peer) = crate::transport::Transport::capture_pair();
        state.clients.insert(
            client_id,
            ClientState {
                writer: Arc::new(Mutex::new(transport)),
                byte_order: ClientByteOrder::LittleEndian,
                last_sequence: Arc::new(AtomicU16::new(0)),
                resource_id_base: 0,
                resource_id_mask: u32::MAX,
                event_masks: HashMap::new(),
                save_set: HashSet::new(),
                big_requests_enabled: false,
                xi2_masks: HashMap::new(),
                xi1_event_classes: HashSet::new(),
                xi1_window_event_classes: HashMap::new(),
                outbound: VecDeque::new(),
                watching_writable: false,
                focused_window: crate::resources::ROOT_WINDOW,
                reader_control: None,
                is_local: true,
                fd_passing: true,
            },
        );
        peer
    }

    fn xi_vt_config_fixture(
        client: u32,
        source: crate::xinput::InputSourceId,
    ) -> (
        ServerState,
        crate::transport::CapturedPeer,
        InputInventory,
        crate::core_loop::DeviceInfo,
        u16,
    ) {
        let mut state = ServerState::new();
        let peer = install_xi_config_test_client(&mut state, client);
        let info = crate::core_loop::DeviceInfo {
            source_id: source,
            enabled: true,
            resume_key: None,
            capabilities: crate::xinput::InputCapabilities {
                pointer: true,
                ..Default::default()
            },
            name: "VT config pointer".into(),
            device_node: format!("/dev/input/event{}", source.0),
            sysname: format!("event{}", source.0),
            vendor_id: 1,
            product_id: 2,
            is_touchpad: false,
            config: crate::core_loop::message::LibinputConfigSnapshot {
                accel: crate::core_loop::message::FloatSetting {
                    available: true,
                    current: 0.0,
                    default: 0.0,
                },
                ..Default::default()
            },
        };
        let device_id = state.xi_register_source(&info)[0];
        let mut inventory = InputInventory::new();
        inventory.add(info.clone());
        (state, peer, inventory, info, device_id)
    }

    fn xi_vt_accel_request(
        client: yserver_protocol::x11::ClientId,
        sequence: u16,
        device_id: u16,
        state: &ServerState,
        speed: f32,
    ) -> DeferredRequest {
        let mut body = Vec::new();
        body.extend_from_slice(&device_id.to_le_bytes());
        body.push(crate::xinput::XI_PROP_MODE_REPLACE);
        body.push(32);
        body.extend_from_slice(
            &state
                .atoms
                .id_for("libinput Accel Speed")
                .expect("acceleration property atom")
                .0
                .to_le_bytes(),
        );
        body.extend_from_slice(&state.float_atom.0.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&speed.to_le_bytes());
        DeferredRequest {
            id: client,
            sequence: yserver_protocol::x11::SequenceNumber(sequence),
            accepted_at: None,
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode: 137,
                data: 57,
                length_units: 6,
            },
            body,
            attached_fd: None,
        }
    }

    fn xi_vt_following_focus_request(client: yserver_protocol::x11::ClientId) -> DeferredRequest {
        DeferredRequest {
            id: client,
            sequence: yserver_protocol::x11::SequenceNumber(2),
            accepted_at: None,
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode: 43,
                data: 0,
                length_units: 1,
            },
            body: Vec::new(),
            attached_fd: None,
        }
    }

    fn send_vt_test_request(
        sender: &crate::core_loop::sender::BoundSender,
        request: DeferredRequest,
    ) {
        sender
            .send(Message::Request {
                id: request.id,
                sequence: request.sequence,
                accepted_at: request.accepted_at,
                header: request.header,
                body: request.body,
                attached_fd: request.attached_fd,
            })
            .expect("queue runner request");
    }

    fn run_core_for_vt_test(
        state: &mut ServerState,
        backend: &mut crate::backend::recording::RecordingBackend,
        enqueue: impl FnOnce(&CoreSender, &crate::core_loop::sender::BoundSender),
    ) -> InputInventory {
        let (poll, sender, receiver) = crate::core_loop::channel().expect("core channel");
        let request_sender = sender.bind();
        enqueue(&sender, &request_sender);
        let mut input_inventory = InputInventory::new();
        run_core_with_inventory(
            poll,
            receiver,
            sender.clone_handle(),
            state,
            backend,
            [],
            &ClientIdAllocator::new(),
            AuthState::new(None),
            ResetPolicy::NoReset,
            None,
            &mut input_inventory,
        )
        .expect("run core through VT release and shutdown messages");
        input_inventory
    }

    fn run_core_for_vt_test_with_timeout(
        mut state: ServerState,
        mut backend: crate::backend::recording::RecordingBackend,
        peer: crate::transport::CapturedPeer,
        timeout: std::time::Duration,
        enqueue: impl FnOnce(&CoreSender, &crate::core_loop::sender::BoundSender) + Send + 'static,
    ) -> Result<
        (
            ServerState,
            crate::backend::recording::RecordingBackend,
            crate::transport::CapturedPeer,
            InputInventory,
        ),
        std::sync::mpsc::RecvTimeoutError,
    > {
        let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(1);
        let runner = std::thread::spawn(move || {
            let input_inventory = run_core_for_vt_test(&mut state, &mut backend, enqueue);
            let _ = finished_tx.send((state, backend, peer, input_inventory));
        });
        drop(runner);
        finished_rx.recv_timeout(timeout)
    }

    #[test]
    fn xi_vt_timeout_cancelled_command_keeps_bad_match_and_state() {
        // Mutation killed: omit the core's cancellation before it fails the
        // still-pending request at the VT barrier timeout.
        use crate::xinput::libinput_props::{
            DeviceConfigError, DeviceConfigStart, DeviceConfigToken,
        };
        use std::io::Read;

        let client = yserver_protocol::x11::ClientId(62);
        let source = crate::xinput::InputSourceId(620);
        let (mut state, peer, _fixture_inventory, info, device) =
            xi_vt_config_fixture(client.0, source);
        state.clients.get_mut(&client.0).unwrap().xi2_masks.insert(
            (crate::resources::ROOT_WINDOW, 0),
            u64::from(crate::xinput::XI2_PROPERTY_EVENT_MASK),
        );
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend.vt_switching_armed = true;
        backend.vt_release_pause_queued = true;
        backend.vt_release_probe_client = Some(client.0);
        backend.vt_release_probe_source = Some(source);
        let release_finished = backend.vt_release_finished.clone();
        backend
            .device_config_start_results
            .push_back(Ok(DeviceConfigStart::Pending(DeviceConfigToken(620))));
        let request = xi_vt_accel_request(client, 1, device, &state, 0.75);
        let registry_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.attached_master,
                )
            })
            .collect();
        let properties_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| (entry.id, entry.properties.clone()))
            .collect();
        let held_before = (
            state.keys_down,
            state.buttons_down,
            state.key_down_by_device.clone(),
        );
        let detached_before = state.xi2_detached_masters.clone();
        let floating_before = state.floating_pointer_positions.clone();
        let selections_before = (
            state.clients[&client.0].xi2_masks.clone(),
            state.clients[&client.0].xi1_event_classes.clone(),
            state.clients[&client.0].xi1_window_event_classes.clone(),
            state.clients[&client.0].event_masks.clone(),
        );

        let (state, backend, mut peer, inventory) = run_core_for_vt_test_with_timeout(
            state,
            backend,
            peer,
            std::time::Duration::from_secs(2),
            move |sender, requests| {
                sender
                    .send(Message::HostInput(HostInputEvent::DeviceAdded(
                        info.clone(),
                    )))
                    .unwrap();
                send_vt_test_request(requests, request);
                sender.send(Message::VtRelease).unwrap();
                sender.send(Message::VtAcquire).unwrap();
                sender
                    .send(Message::HostInput(HostInputEvent::DeviceResumed(info)))
                    .unwrap();
                let late_result_sender = sender.clone_handle();
                drop(std::thread::spawn(move || {
                    std::thread::sleep(
                        VT_INPUT_PAUSE_TIMEOUT + std::time::Duration::from_millis(100),
                    );
                    while !release_finished.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::yield_now();
                    }
                    late_result_sender
                        .send(Message::DeviceConfigResult {
                            token: DeviceConfigToken(620),
                            source,
                            result: Err(DeviceConfigError::Cancelled),
                        })
                        .unwrap();
                    late_result_sender.send(Message::Shutdown).unwrap();
                }));
            },
        )
        .unwrap_or_else(|error| panic!("cancelled VT config timed out: {error:?}"));

        assert!(backend.device_config_cancel_tokens[0].is_cancelled());
        assert_eq!(backend.started_device_configs.len(), 1);
        assert_eq!(inventory.get(source).unwrap().config.accel.current, 0.0);
        assert_eq!(backend.vt_release_inventory_accel_before_finish, Some(0.0));
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.attached_master
                ))
                .collect::<Vec<_>>(),
            registry_before,
        );
        for (id, properties) in properties_before {
            assert_eq!(state.xi_devices.device(id).unwrap().properties, properties);
        }
        let mut error = [0; 32];
        peer.read_exact(&mut error).unwrap();
        assert_eq!(error[0], 0);
        assert_eq!(error[1], yserver_protocol::x11::error::BAD_MATCH);
        peer.set_nonblocking(true).unwrap();
        let mut extra = [0; 1];
        assert!(
            matches!(peer.read(&mut extra), Err(ref err) if err.kind() == io::ErrorKind::WouldBlock),
            "cancelled result emits no property event or second reply",
        );
        assert_eq!(
            (
                state.keys_down,
                state.buttons_down,
                state.key_down_by_device.clone(),
            ),
            held_before,
        );
        assert_eq!(state.xi2_detached_masters, detached_before);
        assert_eq!(state.floating_pointer_positions, floating_before);
        assert_eq!(
            (
                state.clients[&client.0].xi2_masks.clone(),
                state.clients[&client.0].xi1_event_classes.clone(),
                state.clients[&client.0].xi1_window_event_classes.clone(),
                state.clients[&client.0].event_masks.clone(),
            ),
            selections_before,
        );
    }

    #[test]
    fn xi_vt_release_config_write_without_input_thread_does_not_hang() {
        // Mutation killed: wait for InputPaused even though begin_vt_release
        // did not queue an input-thread pause barrier.
        let client = yserver_protocol::x11::ClientId(60);
        let mut state = ServerState::new();
        let peer = install_xi_config_test_client(&mut state, client.0);
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend.vt_switching_armed = true;
        let registry_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.attached_master,
                )
            })
            .collect();
        let properties_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| (entry.id, entry.properties.clone()))
            .collect();
        let held_before = (
            state.keys_down,
            state.buttons_down,
            state.key_down_by_device.clone(),
        );
        let detached_before = state.xi2_detached_masters.clone();
        let floating_before = state.floating_pointer_positions.clone();
        let selections_before = (
            state.clients[&client.0].xi2_masks.clone(),
            state.clients[&client.0].xi1_event_classes.clone(),
            state.clients[&client.0].xi1_window_event_classes.clone(),
            state.clients[&client.0].event_masks.clone(),
        );

        let (state, backend, _peer, _inventory) = run_core_for_vt_test_with_timeout(
            state,
            backend,
            peer,
            std::time::Duration::from_millis(250),
            |sender, _requests| {
                sender.send(Message::VtRelease).unwrap();
                sender.send(Message::Shutdown).unwrap();
            },
        )
        .unwrap_or_else(|error| panic!("VT release without input thread timed out: {error:?}"));

        assert!(
            backend
                .vt_release_finished
                .load(std::sync::atomic::Ordering::SeqCst),
            "VT release must finish without an input-thread barrier",
        );
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.attached_master
                ))
                .collect::<Vec<_>>(),
            registry_before,
        );
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (entry.id, entry.properties.clone()))
                .collect::<Vec<_>>(),
            properties_before,
        );
        assert_eq!(
            (
                state.keys_down,
                state.buttons_down,
                state.key_down_by_device.clone(),
            ),
            held_before,
        );
        assert_eq!(state.xi2_detached_masters, detached_before);
        assert_eq!(state.floating_pointer_positions, floating_before);
        assert_eq!(
            (
                state.clients[&client.0].xi2_masks.clone(),
                state.clients[&client.0].xi1_event_classes.clone(),
                state.clients[&client.0].xi1_window_event_classes.clone(),
                state.clients[&client.0].event_masks.clone(),
            ),
            selections_before,
        );
    }

    #[test]
    fn xi_vt_timeout_late_applied_reconciles_inventory_property_and_event() {
        // Mutation killed: discard a timed-out operation's late Applied
        // result instead of reconciling its confirmed state without a reply.
        use crate::xinput::libinput_props::{DeviceConfigStart, DeviceConfigToken};
        use std::io::Read;

        let client = yserver_protocol::x11::ClientId(61);
        let source = crate::xinput::InputSourceId(610);
        let (mut state, peer, _fixture_inventory, info, device) =
            xi_vt_config_fixture(client.0, source);
        let accel_atom = state.atoms.id_for("libinput Accel Speed").unwrap();
        state.clients.get_mut(&client.0).unwrap().xi2_masks.insert(
            (crate::resources::ROOT_WINDOW, 0),
            u64::from(crate::xinput::XI2_PROPERTY_EVENT_MASK),
        );
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend.vt_switching_armed = true;
        backend.vt_release_pause_queued = true;
        backend.vt_release_probe_client = Some(client.0);
        backend.vt_release_probe_source = Some(source);
        let release_finished = backend.vt_release_finished.clone();
        backend
            .device_config_start_results
            .push_back(Ok(DeviceConfigStart::Pending(DeviceConfigToken(610))));
        let request = xi_vt_accel_request(client, 1, device, &state, 0.75);
        let registry_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                )
            })
            .collect();
        let property_maps_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| (entry.id, entry.properties.clone()))
            .collect();
        let held_before = (
            state.keys_down,
            state.buttons_down,
            state.key_down_by_device.clone(),
            state.xi_devices.device(device).unwrap().buttons_down,
        );
        let detached_before = state.xi2_detached_masters.clone();
        let floating_before = state.floating_pointer_positions.clone();
        let selections_before = (
            state.clients[&client.0].xi2_masks.clone(),
            state.clients[&client.0].xi1_event_classes.clone(),
            state.clients[&client.0].xi1_window_event_classes.clone(),
            state.clients[&client.0].event_masks.clone(),
        );

        let started_at = Instant::now();
        let (state, backend, mut peer, inventory) = run_core_for_vt_test_with_timeout(
            state,
            backend,
            peer,
            std::time::Duration::from_secs(2),
            move |sender, requests| {
                sender
                    .send(Message::HostInput(HostInputEvent::DeviceAdded(
                        info.clone(),
                    )))
                    .unwrap();
                send_vt_test_request(requests, request);
                sender.send(Message::VtRelease).unwrap();
                sender.send(Message::VtAcquire).unwrap();
                sender
                    .send(Message::HostInput(HostInputEvent::DeviceResumed(info)))
                    .unwrap();
                let late_result_sender = sender.clone_handle();
                drop(std::thread::spawn(move || {
                    std::thread::sleep(
                        VT_INPUT_PAUSE_TIMEOUT + std::time::Duration::from_millis(100),
                    );
                    while !release_finished.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::yield_now();
                    }
                    late_result_sender
                        .send(Message::DeviceConfigResult {
                            token: DeviceConfigToken(610),
                            source,
                            result: Ok(()),
                        })
                        .unwrap();
                    late_result_sender.send(Message::Shutdown).unwrap();
                }));
            },
        )
        .unwrap_or_else(|error| panic!("unacknowledged VT pause barrier timed out: {error:?}"));
        assert!(
            started_at.elapsed() >= VT_INPUT_PAUSE_TIMEOUT,
            "a queued pause barrier must receive the full bounded wait",
        );

        assert!(backend.vt_release_wire_visible_before_finish);
        assert!(
            backend
                .vt_release_finished
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert_eq!(backend.started_device_configs.len(), 1);
        assert!(
            backend.device_config_cancel_tokens[0].is_cancelled(),
            "VT timeout shares cancellation with the submitted input command",
        );
        let mut error = [0; 32];
        peer.read_exact(&mut error).unwrap();
        assert_eq!(error[0], 0);
        assert_eq!(error[1], yserver_protocol::x11::error::BAD_MATCH);
        for (id, properties) in property_maps_before {
            let current = &state.xi_devices.device(id).unwrap().properties;
            if id == device {
                assert_eq!(current.len(), properties.len());
                for (property, previous) in properties {
                    let actual = current.get(&property).expect("property remains present");
                    if property == accel_atom {
                        assert_eq!(actual.data, 0.75_f32.to_le_bytes());
                        assert_eq!(actual.format, previous.format);
                        assert_eq!(actual.type_atom, previous.type_atom);
                    } else {
                        assert_eq!(actual, &previous);
                    }
                }
            } else {
                assert_eq!(current, &properties);
            }
        }
        assert_eq!(backend.vt_release_inventory_accel_before_finish, Some(0.0));
        assert_eq!(
            inventory.get(source).unwrap().config.accel.current,
            0.75,
            "confirmed late result updates the process-lifetime inventory",
        );
        assert_eq!(
            state
                .xi_devices
                .source(source)
                .unwrap()
                .config
                .accel
                .current,
            0.75,
            "confirmed late result updates the active XI source snapshot",
        );
        let mut property_event = [0; 32];
        peer.read_exact(&mut property_event).unwrap();
        assert_eq!(property_event[0], 35, "XI2 GenericEvent");
        assert_eq!(
            u16::from_le_bytes([property_event[8], property_event[9]]),
            12,
            "XI_PropertyEvent follows reconciliation",
        );
        assert_eq!(
            u16::from_le_bytes([property_event[10], property_event[11]]),
            device,
            "property event names the current facet",
        );
        assert_eq!(
            u32::from_le_bytes(property_event[16..20].try_into().unwrap()),
            accel_atom.0,
        );
        assert_eq!(property_event[20], 2, "what = Modified");
        peer.set_nonblocking(true).unwrap();
        let mut late_reply = [0; 1];
        assert!(
            matches!(peer.read(&mut late_reply), Err(ref err) if err.kind() == io::ErrorKind::WouldBlock),
            "late reconciliation emits no second reply",
        );
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                ))
                .collect::<Vec<_>>(),
            registry_before,
        );
        assert_eq!(
            (
                state.keys_down,
                state.buttons_down,
                state.key_down_by_device.clone(),
                state.xi_devices.device(device).unwrap().buttons_down,
            ),
            held_before,
        );
        assert_eq!(state.xi2_detached_masters, detached_before);
        assert_eq!(state.floating_pointer_positions, floating_before);
        assert_eq!(
            (
                state.clients[&client.0].xi2_masks.clone(),
                state.clients[&client.0].xi1_event_classes.clone(),
                state.clients[&client.0].xi1_window_event_classes.clone(),
                state.clients[&client.0].event_masks.clone(),
            ),
            selections_before,
        );
    }

    #[test]
    fn xi_config_completion_same_client_request_order_survives_error() {
        use crate::xinput::libinput_props::{
            DeviceConfigError, DeviceConfigStart, DeviceConfigToken,
        };
        use std::io::Read;
        use yserver_protocol::x11::{ClientId, SequenceNumber};

        let client = ClientId(57);
        let mut state = ServerState::new();
        let mut peer = install_xi_config_test_client(&mut state, client.0);
        let info = crate::core_loop::DeviceInfo {
            source_id: crate::xinput::InputSourceId(570),
            enabled: true,
            resume_key: None,
            capabilities: crate::xinput::InputCapabilities {
                pointer: true,
                ..Default::default()
            },
            name: "ordered config pointer".into(),
            device_node: "/dev/input/event570".into(),
            sysname: "event570".into(),
            vendor_id: 1,
            product_id: 2,
            is_touchpad: false,
            config: crate::core_loop::message::LibinputConfigSnapshot {
                accel: crate::core_loop::message::FloatSetting {
                    available: true,
                    current: 0.0,
                    default: 0.0,
                },
                ..Default::default()
            },
        };
        let ids = state.xi_register_source(&info);
        let device_id = ids[0];
        let source = info.source_id;
        let mut inventory = InputInventory::new();
        inventory.add(info);
        let property = state.atoms.id_for("libinput Accel Speed").unwrap();
        let before = state
            .xi_devices
            .device(device_id)
            .unwrap()
            .properties
            .clone();
        let mut body = Vec::new();
        body.extend_from_slice(&device_id.to_le_bytes());
        body.push(crate::xinput::XI_PROP_MODE_REPLACE);
        body.push(32);
        body.extend_from_slice(&property.0.to_le_bytes());
        body.extend_from_slice(&state.float_atom.0.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&0.5_f32.to_le_bytes());
        let first = DeferredRequest {
            id: client,
            sequence: SequenceNumber(1),
            accepted_at: None,
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode: 137,
                data: 57,
                length_units: 6,
            },
            body,
            attached_fd: None,
        };

        let token = DeviceConfigToken(570);
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend
            .device_config_start_results
            .push_back(Ok(DeviceConfigStart::Pending(token)));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut lane = XiConfigLane::default();
        let mut reset_trigger = ResetTrigger::new(ResetPolicy::NoReset);
        let mut requests_this_iter = 0;
        let mut request_budget = 10;
        process_one_request(
            &mut state,
            &mut backend,
            &mut inventory,
            &mut LoopTelemetry::default(),
            &mut pending,
            &mut gate,
            &mut lane,
            &mut reset_trigger,
            Generation::default(),
            &mut requests_this_iter,
            &mut request_budget,
            false,
            first,
        );
        assert!(pending.client_is_blocked(client));
        assert!(!lane.is_empty());
        assert_eq!(backend.started_device_configs.len(), 1);
        assert_eq!(
            state.xi_devices.device(device_id).unwrap().properties,
            before
        );

        let mut later = DeferredRequest {
            id: client,
            sequence: SequenceNumber(2),
            accepted_at: None,
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode: 43, // GetInputFocus reply is easy to identify.
                data: 0,
                length_units: 1,
            },
            body: Vec::new(),
            attached_fd: None,
        };
        let mut deferred = FairRequestQueue::default();
        deferred.push_back(later);
        assert!(deferred.pop_front_unblocked(&pending, &state).is_none());
        peer.set_nonblocking(true).unwrap();
        let mut bytes = [0u8; 64];
        assert!(
            matches!(peer.read(&mut bytes), Err(ref error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        peer.set_nonblocking(false).unwrap();

        dispatch_device_config_result(
            &mut state,
            &mut backend,
            &mut inventory,
            &mut pending,
            &mut gate,
            &mut lane,
            &mut reset_trigger,
            Message::DeviceConfigResult {
                token,
                source,
                result: Err(DeviceConfigError::Invalid),
            },
            Generation::default(),
        );
        assert!(!pending.client_is_blocked(client));
        later = deferred
            .pop_front_unblocked(&pending, &state)
            .expect("later request unblocks after the earlier error");
        process_one_request(
            &mut state,
            &mut backend,
            &mut inventory,
            &mut LoopTelemetry::default(),
            &mut pending,
            &mut gate,
            &mut lane,
            &mut reset_trigger,
            Generation::default(),
            &mut requests_this_iter,
            &mut request_budget,
            false,
            later,
        );

        let mut wire = [0u8; 64];
        peer.read_exact(&mut wire).unwrap();
        assert_eq!(wire[0], 0, "first packet is the earlier request's X_Error");
        assert_eq!(wire[1], yserver_protocol::x11::error::BAD_VALUE);
        assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 1);
        assert_eq!(u16::from_le_bytes([wire[8], wire[9]]), 57);
        assert_eq!(wire[10], 137);
        assert_eq!(wire[32], 1, "second packet answers GetInputFocus");
        assert_eq!(u16::from_le_bytes([wire[34], wire[35]]), 2);
        assert_eq!(
            state.xi_devices.device(device_id).unwrap().properties,
            before
        );
        assert_eq!(inventory.get(source).unwrap().config.accel.current, 0.0);
        assert!(lane.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn xi_vt_release_config_write_applied_reply_precedes_release_completion() {
        // Mutation killed: finish VT release before the pause barrier drains
        // the in-flight XI config, leaving the client blocked across release.
        use crate::xinput::libinput_props::{DeviceConfigStart, DeviceConfigToken};
        use std::io::Read;

        let client = yserver_protocol::x11::ClientId(58);
        let source = crate::xinput::InputSourceId(580);
        let (mut state, mut peer, _fixture_inventory, info, device) =
            xi_vt_config_fixture(client.0, source);
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend.vt_switching_armed = true;
        backend.vt_release_pause_queued = true;
        backend.vt_release_probe_client = Some(client.0);
        backend.vt_release_probe_source = Some(source);
        backend
            .device_config_start_results
            .push_back(Ok(DeviceConfigStart::Pending(DeviceConfigToken(580))));
        let registry_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                )
            })
            .collect();
        let property_maps_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| (entry.id, entry.properties.clone()))
            .collect();
        let held_before = (
            state.keys_down,
            state.buttons_down,
            state.key_down_by_device.clone(),
            state.xi_devices.device(device).unwrap().buttons_down,
        );
        let detached_before = state.xi2_detached_masters.clone();
        let floating_before = state.floating_pointer_positions.clone();
        let selections_before = (
            state.clients[&client.0].xi2_masks.clone(),
            state.clients[&client.0].xi1_event_classes.clone(),
            state.clients[&client.0].xi1_window_event_classes.clone(),
            state.clients[&client.0].event_masks.clone(),
        );
        let request = xi_vt_accel_request(client, 1, device, &state, 0.5);
        run_core_for_vt_test(&mut state, &mut backend, |sender, requests| {
            sender
                .send(Message::HostInput(HostInputEvent::DeviceAdded(info)))
                .unwrap();
            send_vt_test_request(requests, request);
            sender.send(Message::VtRelease).unwrap();
            send_vt_test_request(requests, xi_vt_following_focus_request(client));
            sender
                .send(Message::DeviceConfigResult {
                    token: DeviceConfigToken(580),
                    source,
                    result: Ok(()),
                })
                .unwrap();
            sender.send(Message::InputPaused).unwrap();
            sender.send(Message::Shutdown).unwrap();
        });
        assert!(
            backend.vt_release_wire_visible_before_finish,
            "runner must write the following reply before VT release finishes",
        );
        assert_eq!(
            backend.vt_release_inventory_accel_before_finish,
            Some(0.5),
            "the runner inventory carries the applied value at the release boundary",
        );
        let mut reply = [0; 32];
        peer.read_exact(&mut reply).unwrap();
        assert_eq!(reply[0], 1);
        assert_eq!(u16::from_le_bytes([reply[2], reply[3]]), 2);
        assert!(
            backend
                .vt_release_finished
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert_eq!(backend.started_device_configs.len(), 1);
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                ))
                .collect::<Vec<_>>(),
            registry_before,
        );
        for (id, before) in property_maps_before {
            let after = &state.xi_devices.device(id).unwrap().properties;
            if id != device {
                assert_eq!(*after, before);
                continue;
            }
            assert_eq!(after.len(), before.len());
            for (atom, old_value) in before {
                if atom == state.atoms.id_for("libinput Accel Speed").unwrap() {
                    assert_eq!(after.get(&atom).unwrap().data, 0.5_f32.to_le_bytes());
                } else {
                    assert_eq!(after.get(&atom), Some(&old_value));
                }
            }
        }
        assert_eq!(
            (
                state.keys_down,
                state.buttons_down,
                state.key_down_by_device.clone(),
                state.xi_devices.device(device).unwrap().buttons_down,
            ),
            held_before,
        );
        assert_eq!(state.xi2_detached_masters, detached_before);
        assert_eq!(state.floating_pointer_positions, floating_before);
        assert_eq!(
            (
                state.clients[&client.0].xi2_masks.clone(),
                state.clients[&client.0].xi1_event_classes.clone(),
                state.clients[&client.0].xi1_window_event_classes.clone(),
                state.clients[&client.0].event_masks.clone(),
            ),
            selections_before,
        );
    }

    #[test]
    fn xi_vt_release_config_write_retired_handle_bad_match_has_no_resume_reply() {
        // Mutation killed: park SourceGone until source rebind (or expose it as
        // BadDevice), allowing the pre-pause property write to answer late.
        use crate::xinput::libinput_props::{DeviceConfigStart, DeviceConfigToken};
        use std::io::Read;

        let client = yserver_protocol::x11::ClientId(59);
        let source = crate::xinput::InputSourceId(590);
        let (mut state, mut peer, _fixture_inventory, info, device) =
            xi_vt_config_fixture(client.0, source);
        let mut backend = crate::backend::recording::RecordingBackend::new();
        backend.vt_switching_armed = true;
        backend.vt_release_pause_queued = true;
        backend.vt_release_probe_client = Some(client.0);
        backend.vt_release_probe_source = Some(source);
        backend
            .device_config_start_results
            .push_back(Ok(DeviceConfigStart::Pending(DeviceConfigToken(590))));
        let request = xi_vt_accel_request(client, 1, device, &state, 0.75);
        let property_maps_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| (entry.id, entry.properties.clone()))
            .collect();
        let inventory_accel_before = 0.0;
        let registry_before: Vec<_> = state
            .xi_devices
            .devices()
            .iter()
            .map(|entry| {
                (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                )
            })
            .collect();
        let held_before = (
            state.keys_down,
            state.buttons_down,
            state.key_down_by_device.clone(),
        );
        let detached_before = state.xi2_detached_masters.clone();
        let floating_before = state.floating_pointer_positions.clone();
        let selections_before = (
            state.clients[&client.0].xi2_masks.clone(),
            state.clients[&client.0].xi1_event_classes.clone(),
            state.clients[&client.0].xi1_window_event_classes.clone(),
            state.clients[&client.0].event_masks.clone(),
        );

        run_core_for_vt_test(&mut state, &mut backend, |sender, requests| {
            sender
                .send(Message::HostInput(HostInputEvent::DeviceAdded(
                    info.clone(),
                )))
                .unwrap();
            send_vt_test_request(requests, request);
            sender.send(Message::VtRelease).unwrap();
            sender
                .send(Message::DeviceConfigResult {
                    token: DeviceConfigToken(590),
                    source,
                    result: Err(crate::xinput::libinput_props::DeviceConfigError::SourceGone),
                })
                .unwrap();
            sender.send(Message::InputPaused).unwrap();
            sender.send(Message::VtAcquire).unwrap();
            sender
                .send(Message::HostInput(HostInputEvent::DeviceResumed(info)))
                .unwrap();
            sender.send(Message::Shutdown).unwrap();
        });
        assert!(backend.vt_release_wire_visible_before_finish);
        assert!(
            backend
                .vt_release_finished
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        let mut error = [0; 32];
        peer.read_exact(&mut error).unwrap();
        assert_eq!(error[0], 0);
        assert_eq!(error[1], yserver_protocol::x11::error::BAD_MATCH);
        for (id, properties) in property_maps_before {
            assert_eq!(
                state.xi_devices.device(id).unwrap().properties,
                properties,
                "failed SourceGone does not alter any device property map",
            );
        }
        assert_eq!(
            backend.vt_release_inventory_accel_before_finish,
            Some(inventory_accel_before),
        );
        peer.set_nonblocking(true).unwrap();
        let mut late_reply = [0; 1];
        assert!(
            matches!(peer.read(&mut late_reply), Err(ref err) if err.kind() == io::ErrorKind::WouldBlock),
            "resume must not emit a second or delayed answer for the pre-pause write",
        );
        assert_eq!(
            state
                .xi_devices
                .devices()
                .iter()
                .map(|entry| (
                    entry.id,
                    entry.source_id,
                    entry.enabled,
                    entry.session_enabled,
                    entry.client_disabled,
                    entry.attached_master,
                ))
                .collect::<Vec<_>>(),
            registry_before,
        );
        assert_eq!(
            (
                state.keys_down,
                state.buttons_down,
                state.key_down_by_device.clone(),
            ),
            held_before,
        );
        assert_eq!(state.xi2_detached_masters, detached_before);
        assert_eq!(state.floating_pointer_positions, floating_before);
        assert_eq!(
            (
                state.clients[&client.0].xi2_masks.clone(),
                state.clients[&client.0].xi1_event_classes.clone(),
                state.clients[&client.0].xi1_window_event_classes.clone(),
                state.clients[&client.0].event_masks.clone(),
            ),
            selections_before,
        );
    }

    /// #132: `xrandr --dpi` (RRSetScreenSize with the same pixels, new mm)
    /// reaches NEW clients' setup reply, as Xorg's `pScreen->mmWidth`.
    /// Measured in vng (tools/vng-scenarios/xrandr-dpi.sh): 1280x800 at
    /// `--dpi 108` → 301x188 mm on Xorg 21.1.24 and on yserver.
    #[test]
    fn setup_allocate_reports_randr_screen_mm() {
        let mut state = ServerState::new();
        let (w, h) = (state.randr.screen_width, state.randr.screen_height);
        state.randr.set_logical_size(w, h, 301, 188);
        let (tx, rx) = crossbeam_channel::bounded(1);
        handle_setup_allocate(&mut state, yserver_protocol::x11::ClientId(1), tx);
        let resp = rx.try_recv().expect("setup allocate response");
        assert_eq!((resp.screen_width_mm, resp.screen_height_mm), (301, 188));
    }
    use std::os::unix::net::UnixStream;

    #[test]
    fn listener_accept_budget_leaves_flood_backlog_for_next_turn() {
        let path =
            std::env::temp_dir().join(format!("yserver-accept-budget-{}", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let peers: Vec<_> = (0..33)
            .map(|_| UnixStream::connect(&path).unwrap())
            .collect();
        std::fs::remove_file(path).unwrap();
        let listener = Listener::Unix(listener);
        let alloc = ClientIdAllocator::new();
        let (_poll, sender, _rx) = channel().unwrap();
        let registry = setup_thread::make_registry();

        accept_pending(&listener, &alloc, &sender, &registry, &AuthState::new(None));
        let accepted = alloc.peek().0 - 1;
        setup_thread::shutdown_all(&registry);
        drop(peers);
        assert_eq!(
            accepted, 16,
            "one listener must yield after its accept budget"
        );
    }

    #[test]
    fn ready_listeners_round_robin_under_accept_flood() {
        for tcp_count in [1, 33] {
            let path = std::env::temp_dir().join(format!(
                "yserver-accept-fair-{}-{tcp_count}",
                std::process::id()
            ));
            let unix = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let tcp = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let unix_peers: Vec<_> = (0..33)
                .map(|_| UnixStream::connect(&path).unwrap())
                .collect();
            let tcp_peers: Vec<_> = (0..tcp_count)
                .map(|_| std::net::TcpStream::connect(tcp.local_addr().unwrap()).unwrap())
                .collect();
            std::fs::remove_file(path).unwrap();
            let listeners = [Listener::Unix(unix), Listener::Tcp(tcp)];
            for listener in &listeners {
                listener.set_nonblocking(true).unwrap();
            }
            let alloc = ClientIdAllocator::new();
            let (_poll, sender, _rx) = channel().unwrap();
            let registry = setup_thread::make_registry();
            let mut readiness = ListenerReadiness::new(2);
            readiness.mark_ready(0);
            readiness.mark_ready(1);
            readiness.accept_ready(
                &listeners,
                &alloc,
                &sender,
                &registry,
                &AuthState::new(None),
            );
            {
                let clients = registry.lock().unwrap();
                assert!(matches!(
                    clients[&yserver_protocol::x11::ClientId(1)],
                    Transport::Unix(_)
                ));
                assert!(
                    matches!(
                        clients[&yserver_protocol::x11::ClientId(17)],
                        Transport::Tcp(_)
                    ),
                    "TCP must be accepted within one Unix accept budget, even during a flood"
                );
                assert_eq!(clients.len(), if tcp_count == 1 { 17 } else { 32 });
            }
            // No fresh readiness marks. Queued accepts must persist, and the
            // second round must start at TCP rather than repeat Unix-first.
            readiness.accept_ready(
                &listeners,
                &alloc,
                &sender,
                &registry,
                &AuthState::new(None),
            );
            if tcp_count == 33 {
                let clients = registry.lock().unwrap();
                assert!(matches!(
                    clients[&yserver_protocol::x11::ClientId(33)],
                    Transport::Tcp(_)
                ));
                assert!(matches!(
                    clients[&yserver_protocol::x11::ClientId(49)],
                    Transport::Unix(_)
                ));
            }
            readiness.accept_ready(
                &listeners,
                &alloc,
                &sender,
                &registry,
                &AuthState::new(None),
            );
            assert!(
                !readiness.has_pending(),
                "WouldBlock clears retained readiness"
            );
            assert_eq!(alloc.peek().0 - 1, 33 + tcp_count);
            setup_thread::shutdown_all(&registry);
            drop((unix_peers, tcp_peers));
        }
    }

    #[test]
    fn listener_backlog_completes_without_a_fresh_readiness_edge() {
        use std::io::{Read, Write};
        let path = std::env::temp_dir().join(format!("yserver-accept-edge-{}", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // Queue more than two accept budgets before the listener is registered:
        // there is one readiness edge, with no later connection to wake it.
        let mut peers: Vec<_> = (0..33)
            .map(|_| {
                let mut peer = UnixStream::connect(&path).unwrap();
                peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                peer.write_all(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                    .unwrap();
                peer
            })
            .collect();
        std::fs::remove_file(path).unwrap();
        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let mut backend = RecordingBackend::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                [Listener::Unix(listener)],
                &ClientIdAllocator::new(),
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            )
        });
        let result: io::Result<()> = (|| {
            for peer in &mut peers {
                let mut header = [0; 8];
                peer.read_exact(&mut header)?;
                assert_eq!(header[0], 1);
                let len = usize::from(u16::from_le_bytes([header[6], header[7]])) * 4;
                peer.read_exact(&mut vec![0; len])?;
            }
            Ok(())
        })();
        sender.send(Message::Shutdown).unwrap();
        handle.join().unwrap().unwrap();
        result.expect("all queued connections must finish setup without another accept edge");
    }

    #[test]
    fn randr_change_fanout_orders_screen_then_all_crtcs_then_all_outputs() {
        use crate::server::ClientState;
        use std::{
            collections::{HashMap, HashSet, VecDeque},
            io::Read,
            os::unix::net::UnixStream,
            sync::{Arc, Mutex, atomic::AtomicU16},
        };
        use yserver_protocol::x11::{ClientByteOrder, randr as x11randr};

        let mut state = ServerState::new();
        let first = state.randr.outputs[0].clone();
        let mut second = first.clone();
        second.name = "HDMI-A-1".into();
        second.output_id = first.output_id + 10;
        second.crtc_id = first.crtc_id + 10;
        state.randr.outputs.push(second.clone());

        let (mut peer, writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        state.clients.insert(
            7,
            ClientState {
                writer: Arc::new(Mutex::new(crate::transport::Transport::Unix(writer))),
                byte_order: ClientByteOrder::LittleEndian,
                last_sequence: Arc::new(AtomicU16::new(9)),
                resource_id_base: 0,
                resource_id_mask: 0,
                event_masks: HashMap::new(),
                save_set: HashSet::new(),
                big_requests_enabled: false,
                xi2_masks: HashMap::new(),
                xi1_event_classes: HashSet::new(),
                xi1_window_event_classes: HashMap::new(),
                outbound: VecDeque::new(),
                watching_writable: false,
                focused_window: crate::resources::ROOT_WINDOW,
                reader_control: None,
                is_local: true,
                fd_passing: true,
            },
        );
        state.randr_select_masks.insert(
            (7, crate::resources::ROOT_WINDOW),
            x11randr::NOTIFY_MASK_SCREEN_CHANGE
                | x11randr::NOTIFY_MASK_CRTC_CHANGE
                | x11randr::NOTIFY_MASK_OUTPUT_CHANGE,
        );

        emit_randr_connector_change_notifications(
            &mut state,
            &[(first.output_id, first.crtc_id, first.mode_id)],
            &[(second.output_id, second.crtc_id, second.mode_id)],
        );

        let mut wire = [0; 96];
        peer.read_exact(&mut wire).unwrap();
        const RANDR_FIRST_EVENT: u8 = 89;
        assert_eq!(wire[0] & 0x7f, RANDR_FIRST_EVENT);
        assert_eq!(wire[32] & 0x7f, RANDR_FIRST_EVENT + 1);
        assert_eq!(wire[33], x11randr::NOTIFY_CRTC_CHANGE);
        assert_eq!(wire[64] & 0x7f, RANDR_FIRST_EVENT + 1);
        assert_eq!(wire[65], x11randr::NOTIFY_OUTPUT_CHANGE);
        assert_eq!(
            u32::from_le_bytes(wire[44..48].try_into().unwrap()),
            first.crtc_id
        );
        assert_eq!(
            u32::from_le_bytes(wire[80..84].try_into().unwrap()),
            second.output_id
        );
    }

    /// The count cap alone was sized on an assumption of ~0.25 ms per
    /// request (`MAX_REQUESTS_PER_ITER`'s own doc comment). Measured on
    /// silence under MATE + adapta-nokto during a window drag, single
    /// requests reach 44-50 ms (`longest=op70:44.23ms`), so 32 of them
    /// is ~1.4 s in one iteration — during which host input and backend-fd
    /// readiness sit undelivered and the cursor visibly stalls (`gap_max`
    /// 225-360 ms). These pin the deadline half of the budget.
    #[test]
    fn budget_not_exhausted_when_both_count_and_time_remain() {
        assert!(!budget_exhausted(31, Duration::from_millis(1)));
    }

    #[test]
    fn budget_exhausted_when_count_runs_out() {
        assert!(budget_exhausted(0, Duration::from_millis(0)));
    }

    /// THE FIX: slow requests must stop the drain even with count left.
    #[test]
    fn budget_exhausted_when_deadline_passed_despite_count_remaining() {
        assert!(budget_exhausted(31, REQUEST_TIME_BUDGET));
        assert!(budget_exhausted(
            31,
            REQUEST_TIME_BUDGET + Duration::from_millis(40)
        ));
    }

    /// One 44 ms request must not authorise 31 more. This is the
    /// 1.4 s-iteration case the count-only cap allowed.
    #[test]
    fn budget_stops_after_a_single_overrunning_request() {
        let elapsed_after_one_slow_request = Duration::from_millis(44);
        assert!(budget_exhausted(
            MAX_REQUESTS_PER_ITER - 1,
            elapsed_after_one_slow_request
        ));
    }

    /// Forward progress: the budget is checked before each request with
    /// elapsed measured from the top of the iteration, so the first
    /// request of an iteration always runs. Without this the loop could
    /// livelock without draining anything.
    #[test]
    fn budget_permits_the_first_request_of_an_iteration() {
        assert!(!budget_exhausted(MAX_REQUESTS_PER_ITER, Duration::ZERO));
    }

    /// The fast path must be unchanged: 32 × 0.25 ms = 8 ms, so a
    /// well-behaved burst still exhausts on count, not on time.
    #[test]
    fn typical_fast_requests_still_exhaust_on_count_first() {
        let typical = Duration::from_micros(250);
        let elapsed_at_cap = typical * u32::try_from(MAX_REQUESTS_PER_ITER).unwrap();
        assert!(
            elapsed_at_cap <= REQUEST_TIME_BUDGET,
            "time budget must not bind before the count cap for ~0.25ms requests \
             (elapsed_at_cap={elapsed_at_cap:?}, budget={REQUEST_TIME_BUDGET:?})"
        );
    }

    #[test]
    fn export_holders_report_is_gated_paced_and_rechecks_after_change() {
        let t0 = Instant::now();
        let off = LoopTelemetry::default();
        assert!(!off.export_holders_due(t0));
        let mut on = LoopTelemetry {
            enabled: true,
            ..LoopTelemetry::default()
        };
        assert!(on.export_holders_due(t0));
        on.note_export_holders(t0, true);
        assert!(!on.export_holders_due(t0 + Duration::from_millis(999)));
        assert!(on.export_holders_due(t0 + TELEMETRY_EMIT_INTERVAL));
        assert_eq!(
            on.export_holders_deadline(),
            Some(t0 + TELEMETRY_EMIT_INTERVAL)
        );
        on.note_export_holders(t0 + TELEMETRY_EMIT_INTERVAL, false);
        assert_eq!(on.export_holders_deadline(), None);
    }

    #[test]
    fn loop_telemetry_attributes_burst_depth_age_and_sequence_boundary() {
        let client = yserver_protocol::x11::ClientId(17);
        let other = yserver_protocol::x11::ClientId(23);
        let mut telemetry = LoopTelemetry {
            enabled: true,
            ..LoopTelemetry::default()
        };

        telemetry.record_channel_drain(65_541, &HashMap::from([(client, 65_536), (other, 5)]));
        telemetry.record_request_accepted(client, yserver_protocol::x11::SequenceNumber(0xffff));
        telemetry.record_request_accepted(client, yserver_protocol::x11::SequenceNumber(0x0000));
        telemetry.record_deferred_push(client);
        telemetry.record_deferred_push(client);
        telemetry.record_deferred_push(other);
        telemetry.record_deferred_pop(client);
        telemetry.record_request(
            client,
            133,
            26,
            Duration::from_micros(20),
            Duration::from_millis(1_750),
        );

        assert_eq!(telemetry.channel_request_batch_max, 65_541);
        assert_eq!(telemetry.channel_client_batch_max, (17, 65_536));
        assert_eq!(telemetry.deferred_current, 2);
        assert_eq!(telemetry.max_deferred_depth, 3);
        let client_stats = &telemetry.clients[&client];
        assert_eq!(client_stats.deferred_current, 1);
        assert_eq!(client_stats.deferred_max, 2);
        assert_eq!(client_stats.accepted, 2);
        assert_eq!(client_stats.request_age_max, Duration::from_millis(1_750));
        assert_eq!(client_stats.requests_by_opcode[&(133, Some(26))], 1);
        assert_eq!(client_stats.sequence_ffff, 1);
        assert_eq!(client_stats.sequence_zero, 1);
    }

    fn deferred_request(id: u32, opcode: u8) -> DeferredRequest {
        DeferredRequest {
            id: yserver_protocol::x11::ClientId(id),
            sequence: yserver_protocol::x11::SequenceNumber(1),
            accepted_at: None,
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode,
                data: 0,
                length_units: 1,
            },
            body: Vec::new(),
            attached_fd: None,
        }
    }

    fn c0_randr_request(
        client: u32,
        sequence: u16,
        minor: u8,
        body: Vec<u8>,
        length_units: u32,
    ) -> DeferredRequest {
        DeferredRequest {
            id: yserver_protocol::x11::ClientId(client),
            sequence: yserver_protocol::x11::SequenceNumber(sequence),
            accepted_at: Some(Instant::now()),
            randr_gate_ticket: None,
            randr_gate_expired: false,
            header: yserver_protocol::x11::RequestHeader {
                opcode: 128,
                data: minor,
                length_units,
            },
            body,
            attached_fd: None,
        }
    }

    fn c0_test_publication() -> CrtcConfigPublication {
        CrtcConfigPublication {
            client_id: yserver_protocol::x11::ClientId(1),
            sequence: yserver_protocol::x11::SequenceNumber(1),
            output_id: 1,
            requested_mode: None,
            x: 0,
            y: 0,
            set_time: 0,
            output_bbox_before: None,
            apply_transform: None,
            apply_rotation: None,
            reply_kind: crate::core_loop::process_request::CrtcConfigReplyKind::CrtcConfig,
        }
    }

    fn accept_c0_request(
        backend: &dyn Backend,
        gate: &mut RandrMutationGate,
        queue: &mut FairRequestQueue,
        mut request: DeferredRequest,
    ) {
        gate.register_request(&mut request, backend);
        queue.push_back(request);
    }

    fn make_c0_randr_state() -> ServerState {
        use crate::randr::{RandrMode, RandrOutput, RandrState};

        let output = RandrOutput {
            name: "HDMI-2".to_string(),
            output_id: 1,
            crtc_id: 2,
            mode_id: 3,
            connected: true,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            vrefresh: 60,
            timing: None,
            mm_width: 0,
            mm_height: 0,
            mode_ids: vec![3, 4],
            num_preferred: 1,
            rotation: crate::randr::RR_ROTATE_0,
            pending_transform: Default::default(),
            current_transform: Default::default(),
        };
        let modes = vec![
            RandrMode {
                mode_id: 3,
                width: 1920,
                height: 1080,
                vrefresh: 60,
                timing: None,
            },
            RandrMode {
                mode_id: 4,
                width: 1280,
                height: 720,
                vrefresh: 60,
                timing: None,
            },
        ];
        let mut state = ServerState::new();
        state.randr = RandrState::from_outputs_with_modes(1, vec![output], modes);
        state
    }

    fn install_c0_client(state: &mut ServerState, id: u32) -> UnixStream {
        use crate::{resources::ROOT_WINDOW, server::ClientState, transport::Transport};
        use std::{
            collections::{HashMap, HashSet, VecDeque},
            sync::{Arc, Mutex, atomic::AtomicU16},
        };
        use yserver_protocol::x11::ClientByteOrder;

        let (writer, peer) = UnixStream::pair().unwrap();
        state.clients.insert(
            id,
            ClientState {
                writer: Arc::new(Mutex::new(Transport::Unix(writer))),
                byte_order: ClientByteOrder::LittleEndian,
                last_sequence: Arc::new(AtomicU16::new(0)),
                resource_id_base: 0,
                resource_id_mask: u32::MAX,
                event_masks: HashMap::new(),
                save_set: HashSet::new(),
                big_requests_enabled: false,
                xi2_masks: HashMap::new(),
                xi1_event_classes: HashSet::new(),
                xi1_window_event_classes: HashMap::new(),
                outbound: VecDeque::new(),
                watching_writable: false,
                focused_window: ROOT_WINDOW,
                reader_control: None,
                is_local: true,
                fd_passing: true,
            },
        );
        peer
    }

    fn set_crtc_body(mode: u32) -> Vec<u8> {
        let mut body = Vec::with_capacity(28);
        body.extend_from_slice(&2u32.to_le_bytes()); // crtc
        body.extend_from_slice(&0u32.to_le_bytes()); // timestamp
        body.extend_from_slice(&0u32.to_le_bytes()); // config timestamp
        body.extend_from_slice(&0i16.to_le_bytes()); // x
        body.extend_from_slice(&0i16.to_le_bytes()); // y
        body.extend_from_slice(&mode.to_le_bytes());
        body.extend_from_slice(&1u16.to_le_bytes()); // RR_Rotate_0
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(&1u32.to_le_bytes()); // output
        body
    }

    fn c0_gate_request_deadline_in(
        mut request: DeferredRequest,
        remaining: Duration,
    ) -> DeferredRequest {
        assert!(remaining <= RANDR_GATE_QUEUE_TIMEOUT);
        request.accepted_at = Some(Instant::now() - (RANDR_GATE_QUEUE_TIMEOUT - remaining));
        request
    }

    fn c0_expired_gate_request(mut request: DeferredRequest) -> DeferredRequest {
        request.accepted_at =
            Some(Instant::now() - RANDR_GATE_QUEUE_TIMEOUT - Duration::from_millis(1));
        request
    }

    fn set_screen_size_body(width: u16, height: u16) -> Vec<u8> {
        let mut body = Vec::with_capacity(16);
        body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        body.extend_from_slice(&width.to_le_bytes());
        body.extend_from_slice(&height.to_le_bytes());
        body.extend_from_slice(&500u32.to_le_bytes());
        body.extend_from_slice(&300u32.to_le_bytes());
        body
    }

    fn set_crtc_body_at(mode: u32, set_time: u32) -> Vec<u8> {
        let mut body = set_crtc_body(mode);
        body[4..8].copy_from_slice(&set_time.to_le_bytes());
        body
    }

    fn set_c0_output_mode(state: &mut ServerState, mode_id: u32) {
        let mode = state
            .randr
            .mode_table
            .iter()
            .find(|mode| mode.mode_id == mode_id)
            .cloned()
            .expect("test mode exists");
        let output = &mut state.randr.outputs[0];
        output.mode_id = mode_id;
        output.width = mode.width;
        output.height = mode.height;
        output.vrefresh = mode.vrefresh;
    }

    fn c0_requesterless_publication(
        mode_id: u32,
        config_changed: bool,
    ) -> RequesterlessPublication {
        RequesterlessPublication::new(
            config_changed,
            move |state| set_c0_output_mode(state, mode_id),
            move |state, output_bbox_before| {
                emit_randr_change_notifications(state, &[(1, 2, mode_id)]);
                emit_screen_resize_window_notifications_if_outputs_caught_up(
                    state,
                    output_bbox_before,
                );
            },
        )
    }

    fn install_c0_kill_target(state: &mut ServerState, owner: u32) -> u32 {
        let resource = 0x2000_0000 | owner;
        state.resources.create_window(
            yserver_protocol::x11::ClientId(owner),
            yserver_protocol::x11::CreateWindowRequest {
                depth: 24,
                window: yserver_protocol::x11::ResourceId(resource),
                parent: crate::resources::ROOT_WINDOW,
                width: 64,
                height: 64,
                class: 1,
                visual: crate::resources::ROOT_VISUAL,
                ..Default::default()
            },
        );
        resource
    }

    fn process_c0_kill_client(
        state: &mut ServerState,
        backend: &mut dyn Backend,
        pending: &mut PendingBackendRequests,
        gate: &mut RandrMutationGate,
        reset: &mut ResetTrigger,
        killer: u32,
        target: u32,
    ) {
        let mut request = deferred_request(killer, 113);
        request.header.length_units = 2;
        request.body = target.to_le_bytes().to_vec();
        let mut telemetry = LoopTelemetry::default();
        let mut requests_this_iter = 0;
        let mut request_budget = 8;
        process_one_request(
            state,
            backend,
            &mut InputInventory::default(),
            &mut telemetry,
            pending,
            gate,
            &mut XiConfigLane::default(),
            reset,
            Generation::default(),
            &mut requests_this_iter,
            &mut request_budget,
            false,
            request,
        );
    }

    fn drain_c0_requests(
        state: &mut ServerState,
        backend: &mut dyn Backend,
        pending: &mut PendingBackendRequests,
        gate: &mut RandrMutationGate,
        queue: &mut FairRequestQueue,
    ) {
        let mut telemetry = LoopTelemetry::default();
        let mut reset = ResetTrigger::new(ResetPolicy::NoReset);
        let mut server_grab_waiters = VecDeque::new();
        let mut requests_this_iter = 0;
        let mut request_budget = 64;
        drain_pending_requests(
            state,
            backend,
            &mut InputInventory::default(),
            &mut telemetry,
            pending,
            gate,
            &mut XiConfigLane::default(),
            &mut reset,
            Generation::default(),
            queue,
            &mut server_grab_waiters,
            &mut requests_this_iter,
            &mut request_budget,
            Instant::now(),
        );
        assert!(server_grab_waiters.is_empty());
    }

    fn read_c0_available(peer: &mut UnixStream) -> Vec<u8> {
        use std::io::Read;

        peer.set_nonblocking(true).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            match peer.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read test client output: {error}"),
            }
        }
        out
    }

    fn read_c0_until(peer: &mut UnixStream, expected: usize, timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        while output.len() < expected && Instant::now() < deadline {
            output.extend(read_c0_available(peer));
            if output.len() < expected {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        output
    }

    #[test]
    fn c0_3bii_every_opcode_classified() {
        use yserver_protocol::x11::randr as rr;

        let mutations = [
            rr::RR_SET_SCREEN_CONFIG,
            rr::RR_SET_SCREEN_SIZE,
            rr::RR_SET_CRTC_CONFIG,
            rr::RR_SET_OUTPUT_PRIMARY,
            rr::RR_SET_PROVIDER_OUTPUT_SOURCE,
            rr::RR_SET_PROVIDER_OFFLOAD_SINK,
            rr::RR_SET_PANNING,
            rr::RR_SET_CRTC_TRANSFORM,
            rr::RR_SET_MONITOR,
            rr::RR_DELETE_MONITOR,
        ];
        for minor in 0..=47 {
            let expected = if minor == rr::RR_GET_SCREEN_RESOURCES {
                RandrRequestClass::ForcedReprobe
            } else if mutations.contains(&minor) {
                RandrRequestClass::Mutation
            } else {
                RandrRequestClass::NonGate
            };
            assert_eq!(classify_randr_minor(minor), expected, "minor {minor}");
        }
        assert_eq!(
            classify_randr_minor(rr::RR_SET_CRTC_GAMMA),
            RandrRequestClass::NonGate,
            "gamma is a color request, outside the configuration gate"
        );
        assert_eq!(
            classify_randr_minor(rr::RR_CHANGE_OUTPUT_PROPERTY),
            RandrRequestClass::NonGate
        );
        assert_eq!(
            classify_randr_minor(rr::RR_CHANGE_PROVIDER_PROPERTY),
            RandrRequestClass::NonGate
        );
    }

    #[test]
    fn c0_3bii_one_mutation_in_flight() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut backend = RecordingBackend::new();
        backend.crtc_config_is_install_capable = true;
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, Vec::new(), 7),
        );
        let first = queue
            .pop_front_runnable(
                &PendingBackendRequests::default(),
                &ServerState::new(),
                &gate,
                &backend,
            )
            .unwrap();
        assert!(gate.admit(&first, &backend));
        let token = CrtcConfigToken(101);
        gate.mark_pending(token, c0_test_publication());

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, Vec::new(), 7),
        );
        assert!(
            queue
                .pop_front_runnable(
                    &PendingBackendRequests::default(),
                    &ServerState::new(),
                    &gate,
                    &backend,
                )
                .is_none()
        );
        assert!(gate.is_busy());
        gate.finish_pending(token);
        let second = queue
            .pop_front_runnable(
                &PendingBackendRequests::default(),
                &ServerState::new(),
                &gate,
                &backend,
            )
            .unwrap();
        assert_eq!(second.id.0, 2);
        assert!(gate.admit(&second, &backend));
    }

    #[test]
    fn c0_3bii_gate_is_fifo_by_arrival() {
        use crate::backend::recording::RecordingBackend;

        let mut backend = RecordingBackend::new();
        backend.crtc_config_is_install_capable = true;
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut first_arrival = c0_randr_request(3, 1, 21, Vec::new(), 7);
        let mut second_arrival = c0_randr_request(2, 1, 21, Vec::new(), 7);
        gate.register_request(&mut first_arrival, &backend);
        gate.register_request(&mut second_arrival, &backend);
        queue.push_back(second_arrival);
        queue.push_back(first_arrival);

        let admitted = queue
            .pop_front_runnable(
                &PendingBackendRequests::default(),
                &ServerState::new(),
                &gate,
                &backend,
            )
            .expect("the first gate arrival is runnable");
        assert_eq!(
            admitted.id.0, 3,
            "ready-ring order must not replace gate FIFO"
        );
        assert!(gate.admit(&admitted, &backend));
    }

    #[test]
    fn c0_3bii_gate_blocks_only_the_mutating_client() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let backend = RecordingBackend::new();
        let mut gate = RandrMutationGate::default();
        let mut active = c0_randr_request(1, 1, 21, Vec::new(), 7);
        gate.register_request(&mut active, &backend);
        assert!(gate.admit(&active, &backend));
        gate.mark_pending(CrtcConfigToken(102), c0_test_publication());

        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, Vec::new(), 7),
        );
        queue.push_back(c0_randr_request(
            2,
            2,
            25,
            crate::resources::ROOT_WINDOW.0.to_le_bytes().to_vec(),
            2,
        )); // GetScreenResourcesCurrent behind the mutation
        queue.push_back(c0_randr_request(
            4,
            1,
            25,
            crate::resources::ROOT_WINDOW.0.to_le_bytes().to_vec(),
            2,
        )); // unrelated client's query

        let runnable = queue
            .pop_front_runnable(
                &PendingBackendRequests::default(),
                &ServerState::new(),
                &gate,
                &backend,
            )
            .expect("an unrelated client's query remains runnable");
        assert_eq!(runnable.id.0, 4);
        assert!(
            queue
                .pop_front_runnable(
                    &PendingBackendRequests::default(),
                    &ServerState::new(),
                    &gate,
                    &backend,
                )
                .is_none()
        );
    }

    #[test]
    fn c0_3bii_queries_read_published_state() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let published_mode = state.randr.outputs[0].mode_id;
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(CrtcConfigToken(103));
        backend.crtc_config_is_install_capable = true;
        backend
            .crtc_config_results
            .insert(CrtcConfigToken(103), Ok(true));
        let mut installed_output = state.randr.outputs[0].clone();
        installed_output.mode_id = 4;
        installed_output.width = 1280;
        installed_output.height = 720;
        backend.randr_outputs = vec![installed_output];
        let mut gate = RandrMutationGate::default();
        let mut pending = PendingBackendRequests::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        let mut body = Vec::new();
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        queue.push_back(c0_randr_request(2, 2, 20, body, 3));
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let reply = read_c0_available(&mut b);
        assert_eq!(reply.len(), 40);
        assert_eq!(
            u32::from_le_bytes(reply[20..24].try_into().unwrap()),
            published_mode
        );
        assert_eq!(state.randr.outputs[0].mode_id, published_mode);
    }

    #[test]
    fn c0_3cii_core_forced_reprobe_parks_the_reply() {
        use crate::backend::{CrtcConfigToken, ForcedReprobeResult, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut reprobe_peer = install_c0_client(&mut state, 1);
        let mut mutation_peer = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (1, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let token = CrtcConfigToken(0xc050);
        let mut backend = RecordingBackend::new();
        backend.pending_forced_reprobe = Some(token);
        backend
            .forced_reprobe_results
            .insert(token, Ok(ForcedReprobeResult::Applied));
        backend.reprobe_connectors_changes_state = true;
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_GET_SCREEN_RESOURCES, resources_body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy(), "the forced reprobe owns its gate turn");
        assert!(read_c0_available(&mut reprobe_peer).is_empty());

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(read_c0_available(&mut mutation_peer).is_empty());

        backend.ready_crtc_configs.push(token);
        let mut reset = ResetTrigger::new(ResetPolicy::NoReset);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut reset,
        );
        let reply_and_events = read_c0_available(&mut reprobe_peer);
        assert!(reply_and_events.len() > 32);
        assert_eq!(
            reply_and_events[0] & 0x7f,
            90,
            "publication event precedes reply"
        );
        assert_eq!(reply_and_events[32], 1, "reply follows publication");
        assert!(!gate.is_busy(), "the request turn ends after its reply");
        assert!(read_c0_available(&mut mutation_peer).is_empty());

        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(read_c0_available(&mut mutation_peer).len(), 32);
        assert!(
            state
                .randr
                .outputs
                .iter()
                .any(|output| output.output_id == 1),
            "the expected live RANDR output remains present"
        );
    }

    #[test]
    fn c0_3cii_core_forced_reprobe_requester_gone() {
        use crate::backend::{CrtcConfigToken, ForcedReprobeResult, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut departed_peer = install_c0_client(&mut state, 1);
        let mut survivor_peer = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(0xc051);
        let mut backend = RecordingBackend::new();
        backend.pending_forced_reprobe = Some(token);
        backend
            .forced_reprobe_results
            .insert(token, Ok(ForcedReprobeResult::Expired));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_GET_SCREEN_RESOURCES, resources_body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        disconnect_with_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
            yserver_protocol::x11::ClientId(1),
        );
        assert!(pending.forced_reprobe_by_token.contains_key(&token));
        assert!(gate.is_busy(), "disconnect detaches only the reply owner");
        assert!(!state.clients.contains_key(&1));

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert!(
            !gate.is_busy(),
            "the token's terminal result releases the turn"
        );
        assert!(backend.finished_forced_reprobes.contains(&token));

        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(read_c0_available(&mut survivor_peer).len(), 32);
        assert!(read_c0_available(&mut departed_peer).is_empty());
        assert!(
            state
                .randr
                .outputs
                .iter()
                .any(|output| output.output_id == 1)
        );
    }

    #[test]
    fn c0_3cii_core_forced_reprobe_failure_is_badalloc() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut peer = install_c0_client(&mut state, 1);
        let token = CrtcConfigToken(0xc052);
        let mut backend = RecordingBackend::new();
        backend.pending_forced_reprobe = Some(token);
        backend
            .forced_reprobe_results
            .insert(token, Err(std::io::ErrorKind::Other));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut body = Vec::new();
        body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_GET_SCREEN_RESOURCES, body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            read_c0_available(&mut peer).is_empty(),
            "failure remains parked until ready"
        );
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let error = read_c0_available(&mut peer);
        assert_eq!(error.len(), 32);
        assert_eq!(error[0], 0, "X11 error packet");
        assert_eq!(error[1], yserver_protocol::x11::error::BAD_ALLOC);
        assert_eq!(error[2], 1, "original request sequence");
        assert!(!gate.is_busy(), "failed token releases the forced turn");
        assert!(backend.finished_forced_reprobes.contains(&token));
        assert!(
            state
                .randr
                .outputs
                .iter()
                .any(|output| output.output_id == 1)
        );
    }

    #[test]
    fn c0_3cii_core_expired_forced_reprobe_releases_next_mutation() {
        use crate::backend::{CrtcConfigToken, ForcedReprobeResult, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut query_peer = install_c0_client(&mut state, 1);
        let mut mutation_peer = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(0xc053);
        let mut backend = RecordingBackend::new();
        backend.pending_forced_reprobe = Some(token);
        backend
            .forced_reprobe_results
            .insert(token, Ok(ForcedReprobeResult::Expired));
        backend.apply_crtc_configs = true;
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_GET_SCREEN_RESOURCES, resources_body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(read_c0_available(&mut query_peer).is_empty());
        assert!(read_c0_available(&mut mutation_peer).is_empty());

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let expired_reply = read_c0_available(&mut query_peer);
        assert!(expired_reply.len() >= 32);
        assert_eq!(
            expired_reply[0], 1,
            "Expired replies from the published state"
        );
        assert!(!gate.is_busy());
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(read_c0_available(&mut mutation_peer).len(), 32);
        assert!(backend.calls().iter().any(|call| matches!(
            call,
            crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
        )));
        assert!(
            state
                .randr
                .outputs
                .iter()
                .any(|output| output.output_id == 1)
        );
    }

    #[test]
    fn c0_3cii_legacy_only_reprobe_unchanged() {
        use crate::backend::recording::RecordingBackend;
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut peer = install_c0_client(&mut state, 1);
        let mut backend = RecordingBackend::new();
        backend.reprobe_connectors_changes_state = true;
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_GET_SCREEN_RESOURCES, resources_body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        let reply = read_c0_available(&mut peer);
        assert!(reply.len() >= 32, "synchronous reprobe replies immediately");
        assert_eq!(reply[0], 1, "GetScreenResources reply");
        assert_eq!(backend.reprobe_connectors_calls, 1);
        assert!(pending.is_empty());
        assert!(!gate.is_busy());
        assert!(
            !state.randr.outputs[0].connected,
            "the synchronous reprobe publishes through the established call"
        );
    }

    #[test]
    fn c0_3bii_mate_reassert_behind_a_change() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(CrtcConfigToken(104));
        backend.crtc_config_is_install_capable = true;
        backend
            .crtc_config_results
            .insert(CrtcConfigToken(104), Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, set_crtc_body(3), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            read_c0_available(&mut b).is_empty(),
            "B must remain unprocessed"
        );
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            1
        );

        // Simulate the state rebuild that completes A's publication before
        // allowing B to validate its re-assert against the newly published
        // 1280x720 mode.
        let output = &mut state.randr.outputs[0];
        output.mode_id = 4;
        output.width = 1280;
        output.height = 720;
        backend.ready_crtc_configs.push(CrtcConfigToken(104));
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let calls = backend.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            2,
            "B's 1920x1080 re-assert is a real change after A publishes"
        );
        assert_eq!(read_c0_available(&mut a).len(), 32);
        assert_eq!(read_c0_available(&mut b).len(), 32);
    }

    #[test]
    fn c0_3bii_forced_reprobe_waits_for_the_gate() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(CrtcConfigToken(105));
        backend.reprobe_connectors_changes_state = true;
        backend
            .crtc_config_results
            .insert(CrtcConfigToken(105), Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 8, resources_body, 2),
        );
        // A backend may move a pending token from a cancellable prerequisite
        // into an install-capable dispatched modeset after `Pending` was
        // returned. The gate reads this signal at the forced reprobe's head.
        backend.crtc_config_is_install_capable = true;
        let mut current_body = Vec::new();
        current_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        queue.push_back(c0_randr_request(3, 1, 25, current_body, 2));
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(backend.reprobe_connectors_calls, 0);
        assert!(state.randr.outputs[0].connected);
        assert_eq!(read_c0_available(&mut c).len(), 124);
        assert!(read_c0_available(&mut b).is_empty());

        backend.ready_crtc_configs.push(CrtcConfigToken(105));
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(backend.reprobe_connectors_calls, 1);
        assert!(!state.randr.outputs[0].connected);
        assert_eq!(read_c0_available(&mut a).len(), 32);
        assert!(read_c0_available(&mut b).len() >= 32);
    }

    #[test]
    fn c0_3bii_forced_reprobe_does_not_wait_for_a_prime_probe() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(CrtcConfigToken(106));
        backend.reprobe_connectors_changes_state = true;
        backend
            .crtc_config_results
            .insert(CrtcConfigToken(106), Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 8, resources_body, 2),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(backend.reprobe_connectors_calls, 1);
        assert!(!state.randr.outputs[0].connected);
        assert_eq!(read_c0_available(&mut b).len(), 124);
        assert!(gate.is_busy(), "the PRIME probe remains pending");
    }

    #[test]
    fn c0_3bii_disconnect_after_dispatch_still_publishes() {
        use crate::backend::{CrtcConfigToken, RequesterAbandon, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        let mut waiter = install_c0_client(&mut state, 3);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_SCREEN_CHANGE
                | rr::NOTIFY_MASK_CRTC_CHANGE
                | rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );

        let token = CrtcConfigToken(201);
        let next_token = CrtcConfigToken(202);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_requester_abandon = RequesterAbandon::ContinuesWithoutRequester;
        backend.crtc_config_results.insert(token, Ok(true));
        backend.crtc_config_results.insert(next_token, Ok(false));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 123), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(3, 1, 21, set_crtc_body(3), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        disconnect_with_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
            yserver_protocol::x11::ClientId(1),
        );
        assert!(gate.is_busy(), "dispatched installation remains gate-owned");
        assert!(backend.cancelled_crtc_configs.is_empty());

        set_c0_output_mode(&mut state, 4);
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let events = read_c0_available(&mut listener);
        assert_eq!(events.len(), 96, "Screen, CRTC and Output notifications");
        assert_eq!(events[0] & 0x7f, 89);
        assert_eq!(events[32] & 0x7f, 90);
        assert_eq!(events[32 + 1], rr::NOTIFY_CRTC_CHANGE);
        assert_eq!(events[64] & 0x7f, 90);
        assert_eq!(events[64 + 1], rr::NOTIFY_OUTPUT_CHANGE);
        assert!(
            read_c0_available(&mut requester).is_empty(),
            "no reply to A"
        );
        assert_eq!(state.randr.timestamp, 123);

        backend.pending_crtc_config = Some(next_token);
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            gate.is_busy(),
            "the next waiter is admitted after publication"
        );
        assert_eq!(backend.finished_crtc_configs, [token]);
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            2
        );
        assert!(read_c0_available(&mut waiter).is_empty());
    }

    #[test]
    fn c0_3bii_disconnect_before_dispatch_cancels() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _requester = install_c0_client(&mut state, 1);
        let mut waiter = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(203);
        let next_token = CrtcConfigToken(204);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_results.insert(token, Ok(true));
        backend.crtc_config_results.insert(next_token, Ok(false));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 77), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, set_crtc_body(4), 7),
        );

        disconnect_with_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
            yserver_protocol::x11::ClientId(1),
        );
        assert!(!gate.is_busy(), "Cancelled releases the gate immediately");
        assert_eq!(backend.cancelled_crtc_configs, [token]);
        assert_eq!(state.randr.timestamp, 1);

        backend.pending_crtc_config = Some(next_token);
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy(), "the next waiter can now enter");
        assert!(backend.finished_crtc_configs.is_empty());
        assert!(read_c0_available(&mut waiter).is_empty());
    }

    #[test]
    fn c0_3bii_waiting_head_disconnects() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let first = CrtcConfigToken(205);
        let next = CrtcConfigToken(206);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(first);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(first, Ok(false));
        backend.crtc_config_results.insert(next, Ok(false));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, set_crtc_body(4), 7),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(3, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(
            gate.waiting
                .iter()
                .map(|waiter| waiter.client_id.0)
                .collect::<Vec<_>>(),
            [2, 3]
        );

        disconnect_with_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
            yserver_protocol::x11::ClientId(2),
        );
        assert_eq!(
            gate.waiting
                .iter()
                .map(|waiter| waiter.client_id.0)
                .collect::<Vec<_>>(),
            [3]
        );
        backend.pending_crtc_config = Some(next);
        backend.ready_crtc_configs.push(first);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());
        assert_eq!(backend.finished_crtc_configs, [first]);
        assert!(read_c0_available(&mut b).is_empty());
        assert!(read_c0_available(&mut c).is_empty());
    }

    #[test]
    fn c0_3bii_xdmcp_termination_waits_for_an_install_capable_mutation() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let token = CrtcConfigToken(207);
        let mut backend = RecordingBackend::new();
        backend.crtc_config_is_install_capable = true;
        let mut gate = RandrMutationGate {
            in_flight: Some(RandrGateFlight {
                kind: RandrGateFlightKind::Mutation,
                token: Some(token),
                publication: Some(c0_test_publication()),
            }),
            ..RandrMutationGate::default()
        };
        assert!(
            !xdmcp_termination_can_finish(true, &gate, &backend),
            "orderly XDMCP termination stays pending while the token may install"
        );
        gate.finish_pending(token);
        assert!(xdmcp_termination_can_finish(true, &gate, &backend));
    }

    #[test]
    fn c0_3bii_reset_waits_for_an_install_capable_mutation() {
        use crate::backend::{CrtcConfigToken, RequesterAbandon, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _requester = install_c0_client(&mut state, 1);
        let token = CrtcConfigToken(208);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_requester_abandon = RequesterAbandon::ContinuesWithoutRequester;
        backend.crtc_config_results.insert(token, Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut trigger = ResetTrigger::new(ResetPolicy::Reset);
        trigger.note_client_established();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 9, 21, set_crtc_body_at(4, 91), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        disconnect_with_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
            yserver_protocol::x11::ClientId(1),
        );
        let mut deferred = None;
        assert_eq!(
            defer_reset_action_for_install_capable_mutation(
                &gate,
                &backend,
                &mut deferred,
                trigger.take_pending(),
            ),
            None
        );
        assert_eq!(deferred, Some(ResetAction::Reset));

        set_c0_output_mode(&mut state, 4);
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate_policy(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
            false,
        );
        assert_eq!(backend.finished_crtc_configs, [token]);
        assert_eq!(
            defer_reset_action_for_install_capable_mutation(
                &gate,
                &backend,
                &mut deferred,
                trigger.take_pending(),
            ),
            Some(ResetAction::Reset),
            "the reset boundary becomes runnable only after publication is terminal"
        );

        backend.randr_outputs = state.randr.outputs.clone();
        backend.randr_modes = state.randr.mode_table.clone();
        let poll = mio::Poll::new().expect("reset test poller");
        let generations = crate::core_loop::GenerationCounter::new();
        let setup_registry = crate::core_loop::setup_thread::make_registry();
        let inventory = crate::core_loop::InputInventory::new();
        let mut deferred_requests = FairRequestQueue::default();
        let mut server_grab_waiters = VecDeque::new();
        let mut telemetry = LoopTelemetry::default();
        let generation = reset_generation(
            &mut state,
            &mut backend,
            poll.registry(),
            &generations,
            &setup_registry,
            &inventory,
            GenerationLocals {
                deferred_requests: &mut deferred_requests,
                server_grab_waiters: &mut server_grab_waiters,
                pending_backend_requests: &mut pending,
                randr_mutation_gate: &mut gate,
                telemetry: &mut telemetry,
            },
        )
        .expect("terminal install permits the reset boundary");
        assert_eq!(generations.current(), generation);
        assert_eq!(state.randr.outputs[0].mode_id, 4);
        assert!(!gate.is_busy());
        assert!(pending.is_empty());
    }

    #[test]
    fn c0_3bii_killclient_of_a_dispatched_requester_still_publishes() {
        use crate::backend::{CrtcConfigToken, RequesterAbandon, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        let _killer = install_c0_client(&mut state, 4);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_SCREEN_CHANGE
                | rr::NOTIFY_MASK_CRTC_CHANGE
                | rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let resource = install_c0_kill_target(&mut state, 1);
        let token = CrtcConfigToken(209);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_requester_abandon = RequesterAbandon::ContinuesWithoutRequester;
        backend.crtc_config_results.insert(token, Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut trigger = ResetTrigger::new(ResetPolicy::NoReset);
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 100), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        process_c0_kill_client(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
            4,
            resource,
        );
        assert!(!state.clients.contains_key(&1));
        assert!(gate.is_busy());
        set_c0_output_mode(&mut state, 4);
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
        );
        let events = read_c0_available(&mut listener);
        assert_eq!(events.len(), 96);
        assert!(read_c0_available(&mut requester).is_empty());
        assert!(backend.cancelled_crtc_configs.is_empty());
    }

    #[test]
    fn c0_3bii_killclient_removes_a_waiting_head() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let _killer = install_c0_client(&mut state, 4);
        let resource = install_c0_kill_target(&mut state, 2);
        let first = CrtcConfigToken(210);
        let next = CrtcConfigToken(211);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(first);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(first, Ok(false));
        backend.crtc_config_results.insert(next, Ok(false));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut trigger = ResetTrigger::new(ResetPolicy::NoReset);
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body(4), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, 21, set_crtc_body(4), 7),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(3, 1, 21, set_crtc_body(4), 7),
        );
        process_c0_kill_client(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
            4,
            resource,
        );
        assert_eq!(
            gate.waiting
                .iter()
                .map(|waiter| waiter.client_id.0)
                .collect::<Vec<_>>(),
            [3]
        );
        backend.pending_crtc_config = Some(next);
        backend.ready_crtc_configs.push(first);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());
        assert_eq!(backend.finished_crtc_configs, [first]);
        assert!(read_c0_available(&mut b).is_empty());
        assert!(read_c0_available(&mut c).is_empty());
    }

    #[test]
    fn c0_3bii_failed_publishes_nothing() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        state.randr.timestamp = 777;
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_SCREEN_CHANGE
                | rr::NOTIFY_MASK_CRTC_CHANGE
                | rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let token = CrtcConfigToken(212);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend
            .crtc_config_results
            .insert(token, Err(std::io::ErrorKind::Other));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 333), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let reply = read_c0_available(&mut requester);
        assert_eq!(reply.len(), 32);
        assert_eq!(reply[1], 3);
        assert_eq!(u32::from_le_bytes(reply[8..12].try_into().unwrap()), 777);
        assert!(read_c0_available(&mut listener).is_empty());
        assert_eq!(state.randr.timestamp, 777);
    }

    #[test]
    fn c0_3ci_core_deadline_failure_replies_to_the_parked_request() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        state.randr.timestamp = 777;
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        let mut next_requester = install_c0_client(&mut state, 3);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            yserver_protocol::x11::randr::NOTIFY_MASK_SCREEN_CHANGE
                | yserver_protocol::x11::randr::NOTIFY_MASK_CRTC_CHANGE
                | yserver_protocol::x11::randr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let first = CrtcConfigToken(230);
        let next = CrtcConfigToken(231);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(first);
        backend
            .crtc_config_results
            .insert(first, Err(std::io::ErrorKind::Other));
        backend.crtc_config_results.insert(next, Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut trigger = ResetTrigger::new(ResetPolicy::NoReset);

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 333), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy(), "first mutation owns the gate");

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(3, 2, 21, set_crtc_body_at(3, 334), 7),
        );
        assert_eq!(queue.len, 1, "the second mutation waits behind the token");
        backend.ready_crtc_configs.push(first);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
        );
        let failed_reply = read_c0_available(&mut requester);
        assert_eq!(
            failed_reply.len(),
            32,
            "the parked request receives a reply"
        );
        assert_eq!(failed_reply[1], 3, "the deadline result is Failed");
        assert!(read_c0_available(&mut listener).is_empty());
        assert_eq!(state.randr.timestamp, 777, "failure publishes no state");
        assert!(
            !gate.is_busy(),
            "failed ready-token completion releases the gate"
        );
        assert_eq!(backend.pending_crtc_config, None);
        assert_eq!(backend.finished_crtc_configs, [first]);

        backend.pending_crtc_config = Some(next);
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            gate.is_busy(),
            "the queued mutation takes the released gate: pending={:?}, queue_len={}, finished={:?}",
            backend.pending_crtc_config,
            queue.len,
            backend.finished_crtc_configs
        );
        assert_eq!(
            backend.pending_crtc_config, None,
            "the queued token was taken"
        );
        assert_eq!(queue.len, 0, "the queued mutation is dispatched");
        backend.ready_crtc_configs.push(next);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut trigger,
        );
        assert!(
            !gate.is_busy(),
            "the next mutation releases the gate normally"
        );
        assert_eq!(state.randr.timestamp, 334, "the queued mutation publishes");
        let success_reply = read_c0_available(&mut next_requester);
        assert_eq!(success_reply.len(), 32);
        assert_eq!(success_reply[1], 0);
        assert!(
            !read_c0_available(&mut listener).is_empty(),
            "only the succeeding queued mutation publishes notifications"
        );
    }

    #[test]
    fn c0_3bii_last_set_time_can_go_backward() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};

        let mut state = make_c0_randr_state();
        let _client = install_c0_client(&mut state, 1);
        let first = CrtcConfigToken(213);
        let second = CrtcConfigToken(214);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(first);
        backend.crtc_config_results.insert(first, Ok(true));
        backend.crtc_config_results.insert(second, Ok(true));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, 21, set_crtc_body_at(4, 500), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        set_c0_output_mode(&mut state, 4);
        backend.ready_crtc_configs.push(first);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(state.randr.timestamp, 500);

        backend.pending_crtc_config = Some(second);
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 2, 21, set_crtc_body_at(3, 400), 7),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        set_c0_output_mode(&mut state, 3);
        backend.ready_crtc_configs.push(second);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(state.randr.timestamp, 400);
    }

    #[test]
    fn c0_3bii_waiter_expires_at_q() {
        use crate::{
            backend::{CrtcConfigToken, recording::RecordingBackend},
            core_loop::{message::Message, sender::channel},
        };

        let mut state = make_c0_randr_state();
        state.randr.timestamp = 777;
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(220);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(token, Ok(true));
        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let client_sender = sender.bind();
        let handle = std::thread::spawn(move || {
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        let mut b_request = c0_randr_request(2, 1, 21, set_crtc_body(4), 8);
        b_request.accepted_at =
            Some(Instant::now() - (RANDR_GATE_QUEUE_TIMEOUT - Duration::from_millis(100)));
        for request in [c0_randr_request(1, 1, 21, set_crtc_body(4), 8), b_request] {
            client_sender
                .send(Message::Request {
                    id: request.id,
                    sequence: request.sequence,
                    accepted_at: request.accepted_at,
                    header: request.header,
                    body: request.body,
                    attached_fd: None,
                })
                .unwrap();
        }

        // No further I/O is sent. The queued request must be answered by the
        // gate deadline waking mio.
        let reply = read_c0_until(&mut b, 32, Duration::from_secs(2));
        sender.send(Message::Shutdown).unwrap();
        let (result, backend) = handle.join().unwrap();
        result.unwrap();
        assert_eq!(reply.len(), 32, "gate timer did not wake the core loop");
        assert_eq!(reply[1], 3, "expired SetCrtcConfig returns Failed");
        assert_eq!(
            u32::from_le_bytes(reply[8..12].try_into().unwrap()),
            777,
            "expiry reports the current published timestamp"
        );
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            1,
            "only A reaches the backend"
        );
        assert!(read_c0_available(&mut a).is_empty());
    }

    #[test]
    fn c0_3bii_expiry_is_stateless() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::{error, randr as rr};

        let mut state = make_c0_randr_state();
        state.randr.timestamp = 777;
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(221);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let invalid_against_published_state = c0_expired_gate_request(c0_randr_request(
            2,
            1,
            rr::RR_SET_CRTC_CONFIG,
            set_crtc_body(99),
            8,
        ));
        assert_eq!(
            state
                .randr
                .validate_set_crtc_config(2, 99, &[1])
                .unwrap_err()
                .0,
            error::BAD_MATCH,
            "the queued request would fail state-dependent validation"
        );
        let mut after_predecessor = make_c0_randr_state();
        after_predecessor.randr.outputs[0].mode_ids.push(99);
        after_predecessor
            .randr
            .mode_table
            .push(crate::randr::RandrMode {
                mode_id: 99,
                width: 1280,
                height: 720,
                vrefresh: 60,
                timing: None,
            });
        assert!(
            after_predecessor
                .randr
                .validate_set_crtc_config(2, 99, &[1])
                .is_ok()
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            invalid_against_published_state,
        );
        let mut malformed = set_crtc_body(4);
        malformed.pop();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_expired_gate_request(c0_randr_request(2, 2, rr::RR_SET_CRTC_CONFIG, malformed, 8)),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let failed = read_c0_available(&mut b);
        assert_eq!(failed.len(), 64);
        assert_eq!((failed[0], failed[1]), (1, 3));
        assert_eq!(u32::from_le_bytes(failed[8..12].try_into().unwrap()), 777);
        assert_eq!(
            (failed[32], failed[33]),
            (0, error::BAD_LENGTH),
            "the malformed request still gets its stateless BadLength"
        );
        assert_eq!(state.randr.timestamp, 777);
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            1
        );
        assert!(read_c0_available(&mut a).is_empty());
    }

    #[test]
    fn c0_3bii_sync_mutation_is_never_expired() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(222);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_results.insert(token, Ok(false));
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_expired_gate_request(c0_randr_request(
                2,
                1,
                rr::RR_SET_SCREEN_SIZE,
                set_screen_size_body(1920, 1080),
                5,
            )),
        );
        assert_eq!(gate.next_deadline(), None);
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(read_c0_available(&mut b).is_empty());

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(backend.calls().contains(
            &crate::backend::recording::RecordedCall::SetLogicalScreenSize {
                width: 1920,
                height: 1080,
            }
        ));
        assert!(read_c0_available(&mut b).is_empty());
        assert_eq!(read_c0_available(&mut a).len(), 32);
    }

    #[test]
    fn c0_3bii_deadlines_first_after_a_stalled_sync_mutation() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let token = CrtcConfigToken(223);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_results.insert(token, Ok(false));
        backend.set_logical_screen_size_delay = Duration::from_millis(150);
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(
                2,
                1,
                rr::RR_SET_SCREEN_SIZE,
                set_screen_size_body(1920, 1080),
                5,
            ),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_gate_request_deadline_in(
                c0_randr_request(3, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
                Duration::from_millis(30),
            ),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let reply = read_c0_available(&mut c);
        assert_eq!(reply.len(), 32);
        assert_eq!(reply[1], 3, "C expires after B's synchronous work");
        let apply_calls = backend
            .calls()
            .iter()
            .filter(|call| {
                matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                )
            })
            .count();
        assert_eq!(apply_calls, 1, "C is answered before a second admission");
        assert_eq!(read_c0_available(&mut a).len(), 32);
        assert!(read_c0_available(&mut b).is_empty());
    }

    #[test]
    fn c0_3bii_deadlines_first_after_a_stalled_reprobe() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let token = CrtcConfigToken(224);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(token, Ok(false));
        backend.reprobe_connectors_delay = Duration::from_millis(150);
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        let mut resources_body = Vec::new();
        resources_body.extend_from_slice(&crate::resources::ROOT_WINDOW.0.to_le_bytes());
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 1, rr::RR_GET_SCREEN_RESOURCES, resources_body, 2),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_gate_request_deadline_in(
                c0_randr_request(3, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
                Duration::from_millis(30),
            ),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );

        let reply = read_c0_available(&mut c);
        assert_eq!(reply.len(), 32);
        assert_eq!(reply[1], 3, "C expires after the forced reprobe returns");
        assert_eq!(backend.reprobe_connectors_calls, 1);
        let apply_calls = backend
            .calls()
            .iter()
            .filter(|call| {
                matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                )
            })
            .count();
        assert_eq!(apply_calls, 1, "C is answered before a second admission");
        assert_eq!(read_c0_available(&mut a).len(), 32);
        assert!(!read_c0_available(&mut b).is_empty());
    }

    #[test]
    fn c0_3bii_two_waiters_expire_in_order() {
        use crate::backend::{CrtcConfigToken, recording::RecordingBackend};
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut a = install_c0_client(&mut state, 1);
        let mut b = install_c0_client(&mut state, 2);
        let mut c = install_c0_client(&mut state, 3);
        let token = CrtcConfigToken(225);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_expired_gate_request(c0_randr_request(
                2,
                1,
                rr::RR_SET_CRTC_CONFIG,
                set_crtc_body(4),
                8,
            )),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_expired_gate_request(c0_randr_request(
                3,
                1,
                rr::RR_SET_CRTC_CONFIG,
                set_crtc_body(4),
                8,
            )),
        );
        queue.ready.swap(0, 1);
        let expired_order = gate.expired_waiter_order(Instant::now());
        assert_eq!(
            expired_order
                .iter()
                .map(|waiter| waiter.client_id.0)
                .collect::<Vec<_>>(),
            [2, 3],
            "gate expiry uses arrival order, not the ready-ring order"
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(read_c0_available(&mut b)[1], 3);
        assert_eq!(read_c0_available(&mut c)[1], 3);
        assert_eq!(
            backend
                .calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
                ))
                .count(),
            1,
            "both expired waiters stay out of the backend"
        );
        assert!(read_c0_available(&mut a).is_empty());
    }

    #[test]
    fn c0_3bii_requesterless_waits_behind_a_dispatched_modeset() {
        use crate::{
            backend::{CrtcConfigToken, RequesterAbandon, recording::RecordingBackend},
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE,
        );
        let token = CrtcConfigToken(226);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_requester_abandon = RequesterAbandon::ContinuesWithoutRequester;
        backend.crtc_config_results.insert(token, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let producer = backend.requesterless_publication_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body_at(4, 555), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());
        abandon_client_randr_requests(
            &mut backend,
            &mut pending,
            &mut gate,
            yserver_protocol::x11::ClientId(1),
        );
        abandon_client_randr_requests(
            &mut backend,
            &mut pending,
            &mut gate,
            yserver_protocol::x11::ClientId(1),
        );

        producer
            .enqueue(c0_requesterless_publication(3, true))
            .unwrap();
        // The publication wake is serviced while the dispatched mutation is
        // still live, but its event remains behind that mutation.
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert!(read_c0_available(&mut listener).is_empty());
        assert_eq!(gate.requesterless_publications.len(), 1);

        // The modeset installs 1280×720 and publishes first. The backend
        // publication then restores 1920×1080 and emits its own event.
        set_c0_output_mode(&mut state, 4);
        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let events = read_c0_available(&mut listener);
        assert_eq!(events.len(), 64);
        assert_eq!(
            (events[1], events[33]),
            (rr::NOTIFY_CRTC_CHANGE, rr::NOTIFY_CRTC_CHANGE)
        );
        assert_eq!(
            [
                u16::from_le_bytes(events[28..30].try_into().unwrap()),
                u16::from_le_bytes(events[60..62].try_into().unwrap()),
            ],
            [1280, 1920],
            "the requester-less publication follows the installed modeset publication"
        );
        assert!(read_c0_available(&mut requester).is_empty());
        assert_eq!(state.randr.timestamp, 555);
    }

    #[test]
    fn c0_3bii_requesterless_does_not_wait_for_a_superseded_one() {
        use crate::{
            backend::{CrtcConfigToken, RequesterlessPublication, recording::RecordingBackend},
            core_loop::channel,
        };
        use std::{
            io::Read,
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let requester_probe = requester.try_clone().unwrap();
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE,
        );
        let token = CrtcConfigToken(227);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = false;
        backend
            .crtc_config_results
            .insert(token, Err(io::ErrorKind::Other));
        let (_poll, sender, _rx) = channel().unwrap();
        let producer = backend.requesterless_publication_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy());

        // This publication represents the REC-4 transition that supersedes
        // the still-undispatched probe. It must publish on its wake, without
        // waiting for that token's terminal Failed result.
        let reply_seen_during_publication = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&reply_seen_during_publication);
        let mut requester_probe = requester_probe;
        requester_probe.set_nonblocking(true).unwrap();
        producer
            .enqueue(RequesterlessPublication::new(
                true,
                |state| set_c0_output_mode(state, 4),
                move |state, output_bbox_before| {
                    let mut byte = [0; 1];
                    match requester_probe.read(&mut byte) {
                        Ok(0) => {}
                        Ok(_) => seen.store(true, Ordering::SeqCst),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("probe requester output during publication: {error}"),
                    }
                    emit_randr_change_notifications(state, &[(1, 2, 4)]);
                    emit_screen_resize_window_notifications_if_outputs_caught_up(
                        state,
                        output_bbox_before,
                    );
                },
            ))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert!(!reply_seen_during_publication.load(Ordering::SeqCst));
        assert!(gate.is_busy(), "the CRTC result has not arrived yet");
        let published = read_c0_available(&mut listener);
        assert_eq!(published.len(), 32, "publication is immediate on its wake");

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let reply = read_c0_available(&mut requester);
        assert_eq!(reply.len(), 32);
        assert_eq!(reply[1], 3, "the superseded CRTC request fails");
        assert!(read_c0_available(&mut listener).is_empty());
    }

    #[test]
    fn c0_3bii_requesterless_on_an_idle_gate() {
        use crate::{
            backend::{RequesterlessPublication, recording::RecordingBackend},
            core_loop::{Message, channel},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        state.randr.timestamp = 777;
        state.randr.config_timestamp = 888;
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE,
        );
        let mut backend = RecordingBackend::new();
        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let producer = backend.requesterless_publication_producer(sender.clone_handle());
        let config_time_at_notification = Arc::new(AtomicU32::new(u32::MAX));
        let config_time_capture = Arc::clone(&config_time_at_notification);
        let handle = std::thread::spawn(move || {
            let mut state = state;
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &ClientIdAllocator::new(),
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, state, backend)
        });

        producer
            .enqueue(RequesterlessPublication::new(
                true,
                |state| set_c0_output_mode(state, 4),
                move |state, output_bbox_before| {
                    config_time_capture.store(state.randr.config_timestamp, Ordering::SeqCst);
                    emit_randr_change_notifications(state, &[(1, 2, 4)]);
                    emit_screen_resize_window_notifications_if_outputs_caught_up(
                        state,
                        output_bbox_before,
                    );
                },
            ))
            .unwrap();
        let event = read_c0_until(&mut listener, 32, Duration::from_secs(2));
        sender.send(Message::Shutdown).unwrap();
        let (result, state, backend) = handle.join().unwrap();
        result.unwrap();

        assert_eq!(
            event.len(),
            32,
            "the wake publishes with no CRTC token ready"
        );
        assert_eq!(event[1], rr::NOTIFY_CRTC_CHANGE);
        assert_eq!(u16::from_le_bytes(event[28..30].try_into().unwrap()), 1280);
        assert_eq!(state.randr.timestamp, 777, "lastSetTime is unchanged");
        assert_ne!(state.randr.config_timestamp, 888, "lastConfigTime advances");
        assert_eq!(
            state.randr.config_timestamp,
            config_time_at_notification.load(Ordering::SeqCst),
            "notifications carry the updated lastConfigTime"
        );
        assert!(backend.finished_crtc_configs.is_empty());
    }

    #[test]
    fn c0_3ci_core_withdrawal_bypasses_the_gate() {
        use crate::{
            backend::{CrtcConfigToken, RequesterlessPublication, recording::RecordingBackend},
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE | rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let token = CrtcConfigToken(229);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(token, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let producer = backend.urgent_withdrawal_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(gate.is_busy(), "the client mutation owns the gate turn");

        producer
            .enqueue(RequesterlessPublication::urgent_withdrawal(
                vec![1],
                vec![2],
            ))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        let removal = read_c0_available(&mut listener);
        assert!(
            !removal.is_empty(),
            "withdrawal notifies before the CRTC reply"
        );
        assert!(read_c0_available(&mut requester).is_empty());
        assert!(
            gate.is_busy(),
            "urgent withdrawal does not release the gate"
        );

        backend.ready_crtc_configs.push(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        let reply = read_c0_available(&mut requester);
        assert_eq!(
            reply.len(),
            32,
            "the client mutation replies after withdrawal"
        );
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            []
        );
    }

    #[test]
    fn c0_3ci_core_withdrawal_only_subtracts() {
        use crate::{
            backend::{CrtcConfigToken, RequesterlessPublication, recording::RecordingBackend},
            core_loop::channel,
            randr::{RandrMode, RandrState},
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut surviving = state.randr.outputs[0].clone();
        surviving.output_id = 5;
        surviving.crtc_id = 6;
        surviving.mode_id = 7;
        surviving.name = "DP-1".to_string();
        surviving.mode_ids = vec![7, 8];
        let old_mode = RandrMode {
            mode_id: 7,
            width: 1600,
            height: 900,
            vrefresh: 60,
            timing: None,
        };
        let new_mode = RandrMode {
            mode_id: 8,
            width: 1280,
            height: 720,
            vrefresh: 60,
            timing: None,
        };
        state.randr = RandrState::from_outputs_with_modes(
            1,
            vec![state.randr.outputs[0].clone(), surviving.clone()],
            vec![
                RandrMode {
                    mode_id: 3,
                    width: 1920,
                    height: 1080,
                    vrefresh: 60,
                    timing: None,
                },
                old_mode,
                new_mode,
            ],
        );
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE | rr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let token = CrtcConfigToken(230);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        let (_poll, sender, _rx) = channel().unwrap();
        let publications = backend.requesterless_publication_producer(sender.clone_handle());
        let urgent = backend.urgent_withdrawal_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let mut gate_request = c0_randr_request(9, 1, rr::RR_SET_CRTC_CONFIG, Vec::new(), 1);
        gate.register_request(&mut gate_request, &backend);
        assert!(gate.admit(&gate_request, &backend));
        gate.mark_pending(token, c0_test_publication());

        // The backend's later full projection already contains the promoted
        // mode change. The core must not expose it as part of the urgent
        // withdrawal while this unrelated install-capable turn is held.
        let withdrawn_output = state.randr.outputs[0].clone();
        let mut promoted_survivor = surviving;
        promoted_survivor.mode_id = 8;
        promoted_survivor.width = 1280;
        promoted_survivor.height = 720;
        publications
            .enqueue(RequesterlessPublication::new(
                true,
                move |state| {
                    state.randr = RandrState::from_outputs_with_modes(
                        1,
                        vec![withdrawn_output, promoted_survivor],
                        vec![
                            RandrMode {
                                mode_id: 3,
                                width: 1920,
                                height: 1080,
                                vrefresh: 60,
                                timing: None,
                            },
                            old_mode,
                            new_mode,
                        ],
                    );
                },
                |state, output_bbox_before| {
                    emit_randr_change_notifications(state, &[(5, 6, 8)]);
                    emit_screen_resize_window_notifications_if_outputs_caught_up(
                        state,
                        output_bbox_before,
                    );
                },
            ))
            .unwrap();
        urgent
            .enqueue(RequesterlessPublication::urgent_withdrawal(
                vec![1],
                vec![2],
            ))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [5]
        );
        let mut mode_query = Vec::new();
        mode_query.extend_from_slice(&6u32.to_le_bytes());
        mode_query.extend_from_slice(&0u32.to_le_bytes());
        queue.push_back(c0_randr_request(1, 2, rr::RR_GET_CRTC_INFO, mode_query, 3));
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        let old_reply = read_c0_available(&mut requester);
        assert_eq!(u32::from_le_bytes(old_reply[20..24].try_into().unwrap()), 7);
        assert_eq!(state.randr.outputs[0].mode_id, 7);
        assert!(!read_c0_available(&mut listener).is_empty());

        gate.finish_pending(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [5]
        );
        assert_eq!(state.randr.outputs[0].mode_id, 8);
        let mut second_mode_query = Vec::new();
        second_mode_query.extend_from_slice(&6u32.to_le_bytes());
        second_mode_query.extend_from_slice(&0u32.to_le_bytes());
        queue.push_back(c0_randr_request(
            1,
            3,
            rr::RR_GET_CRTC_INFO,
            second_mode_query,
            3,
        ));
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        let new_reply = read_c0_available(&mut requester);
        assert_eq!(u32::from_le_bytes(new_reply[20..24].try_into().unwrap()), 8);
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [5]
        );
    }

    #[test]
    fn c0_3ci_core_queued_publication_cannot_restore_withdrawn() {
        use crate::{
            backend::{CrtcConfigToken, RequesterlessPublication, recording::RecordingBackend},
            core_loop::channel,
            randr::RandrState,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let mut state = make_c0_randr_state();
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            yserver_protocol::x11::randr::NOTIFY_MASK_CRTC_CHANGE
                | yserver_protocol::x11::randr::NOTIFY_MASK_OUTPUT_CHANGE,
        );
        let token = CrtcConfigToken(231);
        let mut backend = RecordingBackend::new();
        backend.crtc_config_is_install_capable = true;
        let (_poll, sender, _rx) = channel().unwrap();
        let publications = backend.requesterless_publication_producer(sender.clone_handle());
        let urgent = backend.urgent_withdrawal_producer(sender.clone_handle());
        let observed_projection = Arc::new(AtomicBool::new(false));
        let projection_at_notification = Arc::clone(&observed_projection);
        let mut stale_output = state.randr.outputs[0].clone();
        stale_output.mode_id = 4;
        stale_output.width = 1280;
        stale_output.height = 720;
        let stale_modes = state.randr.mode_table.clone();
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut gate_request = c0_randr_request(
            9,
            1,
            yserver_protocol::x11::randr::RR_SET_CRTC_CONFIG,
            Vec::new(),
            1,
        );
        gate.register_request(&mut gate_request, &backend);
        assert!(gate.admit(&gate_request, &backend));
        gate.mark_pending(token, c0_test_publication());

        // This closure was queued before the urgent event and captured the
        // old backend projection, including the output being withdrawn.
        publications
            .enqueue(RequesterlessPublication::new(
                true,
                move |state| {
                    state.randr =
                        RandrState::from_outputs_with_modes(1, vec![stale_output], stale_modes);
                },
                move |state, output_bbox_before| {
                    projection_at_notification.store(
                        state
                            .randr
                            .outputs
                            .iter()
                            .any(|output| output.output_id == 1),
                        Ordering::SeqCst,
                    );
                    emit_randr_change_notifications(state, &[(1, 2, 4)]);
                    emit_screen_resize_window_notifications_if_outputs_caught_up(
                        state,
                        output_bbox_before,
                    );
                },
            ))
            .unwrap();
        urgent
            .enqueue(RequesterlessPublication::urgent_withdrawal(
                vec![1],
                vec![2],
            ))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert!(state.randr.outputs.is_empty());

        gate.finish_pending(token);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert!(!observed_projection.load(Ordering::SeqCst));
        assert!(state.randr.outputs.is_empty());
        assert!(!read_c0_available(&mut listener).is_empty());
        assert!(
            state.randr.outputs.is_empty(),
            "expected live output ids: []"
        );
    }

    #[test]
    fn c0_3ci_core_episode_holds_the_turn() {
        use crate::{
            backend::{
                CrtcConfigToken, RequesterlessPublication, TopologyEpisodeEvent,
                recording::RecordingBackend,
            },
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut query_client = install_c0_client(&mut state, 2);
        let token = CrtcConfigToken(232);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(token, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(52))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        let mut crtc_info = Vec::new();
        crtc_info.extend_from_slice(&2u32.to_le_bytes());
        crtc_info.extend_from_slice(&0u32.to_le_bytes());
        queue.push_back(c0_randr_request(2, 1, rr::RR_GET_CRTC_INFO, crtc_info, 3));
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            backend.pending_crtc_config.is_some(),
            "mutation is parked by episode"
        );
        assert!(!backend.calls.lock().unwrap().iter().any(|call| matches!(
            call,
            crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
        )));
        let query_reply = read_c0_available(&mut query_client);
        assert_eq!(
            u32::from_le_bytes(query_reply[20..24].try_into().unwrap()),
            3
        );
        assert!(read_c0_available(&mut requester).is_empty());

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(
                52,
                Some(RequesterlessPublication::new(
                    true,
                    |state| set_c0_output_mode(state, 4),
                    |state, output_bbox_before| {
                        emit_randr_change_notifications(state, &[(1, 2, 4)]);
                        emit_screen_resize_window_notifications_if_outputs_caught_up(
                            state,
                            output_bbox_before,
                        );
                    },
                )),
            ))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            backend.pending_crtc_config.is_none(),
            "queued mutation runs after EpisodeEnd"
        );
        assert!(backend.calls.lock().unwrap().iter().any(|call| matches!(
            call,
            crate::backend::recording::RecordedCall::ApplyCrtcConfig { .. }
        )));
        assert_eq!(
            state.randr.outputs[0].mode_id, 4,
            "EpisodeEnd publishes its generation before the queued mutation starts"
        );
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn c0_3cii_core_episode_begin_waits_for_in_flight_mutation() {
        use crate::{
            backend::{
                CrtcConfigToken, TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut first_client = install_c0_client(&mut state, 1);
        let mut later_client = install_c0_client(&mut state, 2);
        let mutation = CrtcConfigToken(234);
        let episode_id = 54;
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(mutation);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(mutation, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(
            gate.in_flight.as_ref().and_then(|flight| flight.token),
            Some(mutation)
        );

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(episode_id))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert!(
            !backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id)),
            "the core must not notify the backend while the earlier mutation is in flight"
        );

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(2, 2, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| matches!(call, RecordedCall::ApplyCrtcConfig { .. }))
                .count(),
            1,
            "a later mutation remains queued behind the requested episode"
        );

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(episode_id, None))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert!(
            gate.requested_topology_episodes.is_empty(),
            "EpisodeEnd(None) withdraws the ungranted request"
        );
        backend.ready_crtc_configs.push(mutation);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert!(
            !backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id)),
            "a withdrawn episode is never granted when the earlier mutation finishes"
        );
        assert!(!read_c0_available(&mut first_client).is_empty());
        assert!(read_c0_available(&mut later_client).is_empty());
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(gate.topology_episode, None);
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| matches!(call, RecordedCall::ApplyCrtcConfig { .. }))
                .count(),
            2,
            "withdrawing the episode before grant releases the later mutation"
        );
        assert!(!read_c0_available(&mut later_client).is_empty());
    }

    fn c0_3cii_assert_modeset_precedes_hotplug_grant(
        episode_id: u64,
        first_client_id: u32,
        later_client_id: u32,
        label: &str,
    ) {
        use crate::{
            backend::{
                CrtcConfigToken, TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, first_client_id);
        let mut later_requester = install_c0_client(&mut state, later_client_id);
        let mutation = CrtcConfigToken(235 + episode_id);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(mutation);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(mutation, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        // This accepted SetCrtcConfig models the client modeset already in
        // flight on A. The gate is server-wide, so a hotplug episode for A or
        // B must both wait for its terminal result.
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(
                first_client_id,
                1,
                rr::RR_SET_CRTC_CONFIG,
                set_crtc_body(4),
                8,
            ),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(
            gate.in_flight.as_ref().and_then(|flight| flight.token),
            Some(mutation)
        );

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(episode_id))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None, "{label}: no early grant");
        assert!(
            !backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id)),
            "{label}: the hotplug backend has not been called before modeset completion"
        );
        assert!(read_c0_available(&mut requester).is_empty());
        assert_eq!(state.randr.outputs[0].mode_id, 3);
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [1],
            "{label}: expected live output IDs while the modeset is in flight: [1]"
        );

        backend.ready_crtc_configs.push(mutation);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(gate.topology_episode, Some(episode_id));
        assert!(
            backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id))
        );
        assert_eq!(state.randr.outputs[0].mode_id, 3);
        assert!(!read_c0_available(&mut requester).is_empty());

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(episode_id, None))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert_eq!(state.randr.outputs[0].mode_id, 3);
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [1],
            "{label}: expected live output IDs at terminal state: [1]"
        );
        assert!(read_c0_available(&mut later_requester).is_empty());
    }

    #[test]
    fn c0_3cii_same_device_hotplug_waits_for_modeset_gate() {
        c0_3cii_assert_modeset_precedes_hotplug_grant(
            19,
            1,
            2,
            "same-device hotplug waits for the dispatched modeset",
        );
    }

    #[test]
    fn c0_3cii_cross_device_hotplug_waits_for_modeset_gate() {
        c0_3cii_assert_modeset_precedes_hotplug_grant(
            53,
            3,
            4,
            "device B hotplug waits for device A's dispatched modeset",
        );
    }

    #[test]
    fn c0_3cii_release_withdraws_hotplug_waiting_for_modeset_gate() {
        use crate::{
            backend::{
                CrtcConfigToken, TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mutation = CrtcConfigToken(287);
        let episode_id = 52;
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(mutation);
        backend.crtc_config_is_install_capable = true;
        backend.crtc_config_results.insert(mutation, Ok(true));
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(episode_id))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert!(gate.requested_topology_episodes.contains(&episode_id));

        // VT release invalidates the hotplug participant before the core
        // grants its requested turn. EpisodeEnd(None) is the backend's
        // withdrawal notification; the outstanding client modeset remains
        // independently serialized by the same gate.
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(episode_id, None))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(gate.topology_episode, None);
        assert!(gate.requested_topology_episodes.is_empty());
        assert!(
            !backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id))
        );

        backend.ready_crtc_configs.push(mutation);
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert_eq!(gate.topology_episode, None);
        assert_eq!(state.randr.outputs[0].mode_id, 3);
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [1],
            "release leaves the client's output as the expected live set: [1]"
        );
        assert!(!read_c0_available(&mut requester).is_empty());
    }

    #[test]
    fn c0_3cii_acquire_episode_granted_at_once() {
        use crate::{
            backend::{
                TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut client = install_c0_client(&mut state, 1);
        let episode_id = 59;
        let mut backend = RecordingBackend::new();
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();

        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(episode_id))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        assert_eq!(gate.topology_episode, Some(episode_id));
        assert!(
            backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id))
        );
        assert!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !matches!(call, RecordedCall::ApplyCrtcConfig { .. })),
            "the acquire turn is granted before already accepted client requests drain"
        );
        assert!(read_c0_available(&mut client).is_empty());
    }

    #[test]
    fn c0_3cii_episode_end_none_withdraws_before_grant() {
        use crate::{
            backend::{
                TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::channel,
        };

        let mut state = make_c0_randr_state();
        let mut backend = RecordingBackend::new();
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let episode_id = 64;

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(episode_id))
            .unwrap();
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(episode_id, None))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        assert_eq!(gate.topology_episode, None);
        assert!(gate.requested_topology_episodes.is_empty());
        assert!(
            !backend
                .calls
                .lock()
                .unwrap()
                .contains(&RecordedCall::TopologyEpisodeGranted(episode_id))
        );
    }

    #[test]
    fn c0_3ci_core_episode_abort_releases_the_turn() {
        use crate::{
            backend::{CrtcConfigToken, TopologyEpisodeEvent, recording::RecordingBackend},
            core_loop::channel,
        };
        use yserver_protocol::x11::randr as rr;

        let mut state = make_c0_randr_state();
        let mut requester = install_c0_client(&mut state, 1);
        let mut listener = install_c0_client(&mut state, 2);
        state.randr_select_masks.insert(
            (2, crate::resources::ROOT_WINDOW),
            rr::NOTIFY_MASK_CRTC_CHANGE,
        );
        let token = CrtcConfigToken(233);
        let mut backend = RecordingBackend::new();
        backend.pending_crtc_config = Some(token);
        backend.crtc_config_is_install_capable = true;
        let (_poll, sender, _rx) = channel().unwrap();
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        let mut pending = PendingBackendRequests::default();
        let mut gate = RandrMutationGate::default();
        let mut queue = FairRequestQueue::default();
        let config_time_before = state.randr.config_timestamp;
        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeBegin(53))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        accept_c0_request(
            &backend,
            &mut gate,
            &mut queue,
            c0_randr_request(1, 1, rr::RR_SET_CRTC_CONFIG, set_crtc_body(4), 8),
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(backend.pending_crtc_config.is_some());

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(53, None))
            .unwrap();
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );
        assert_eq!(state.randr.config_timestamp, config_time_before);
        assert!(
            read_c0_available(&mut listener).is_empty(),
            "abort publishes no notifications"
        );
        drain_c0_requests(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut queue,
        );
        assert!(
            backend.pending_crtc_config.is_none(),
            "abort releases the queued mutation"
        );
        assert!(read_c0_available(&mut requester).is_empty());
        assert_eq!(state.randr.outputs[0].mode_id, 3);
        assert_eq!(
            state
                .randr
                .outputs
                .iter()
                .map(|output| output.output_id)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn c0_3ci_acquire_reserves_the_turn_before_pending_requests() {
        use std::{
            io::{Read, Write},
            os::unix::net::{UnixListener, UnixStream},
            time::Duration,
        };

        use crate::{
            backend::{
                TopologyEpisodeEvent,
                recording::{RecordedCall, RecordingBackend},
            },
            core_loop::{Message, channel},
        };

        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "yserver-c0-vt-episode-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let listener = UnixListener::bind(&path).expect("bind core-loop test listener");
        listener
            .set_nonblocking(true)
            .expect("make core-loop test listener nonblocking");
        let (poll, sender, receiver) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let mut state = make_c0_randr_state();
        let mut backend = RecordingBackend::new();
        backend.crtc_config_is_install_capable = true;
        let token = crate::backend::CrtcConfigToken(773);
        backend.pending_crtc_config = Some(token);
        let calls = std::sync::Arc::clone(&backend.calls);
        let episodes = backend.topology_episode_producer(sender.clone_handle());
        backend.arm_vt_acquire_episode_for_tests(episodes.clone(), 77);
        let core = std::thread::spawn(move || {
            let result = run_core(
                poll,
                receiver,
                sender_for_core,
                &mut state,
                &mut backend,
                [Listener::Unix(listener)],
                &ClientIdAllocator::new(),
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        let establish_client = || {
            let mut peer = UnixStream::connect(&path).expect("connect core-loop test client");
            peer.set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set core-loop test client timeout");
            peer.write_all(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                .expect("send X11 setup request");
            let mut setup_header = [0; 8];
            peer.read_exact(&mut setup_header)
                .expect("read X11 setup reply");
            assert_eq!(setup_header[0], 1, "X11 setup succeeded");
            let setup_body_len =
                usize::from(u16::from_le_bytes([setup_header[6], setup_header[7]])) * 4;
            peer.read_exact(&mut vec![0; setup_body_len])
                .expect("read X11 setup body");
            peer.write_all(&[43, 0, 1, 0])
                .expect("send setup GetInputFocus");
            let mut setup_query_reply = [0; 32];
            peer.read_exact(&mut setup_query_reply)
                .expect("read setup GetInputFocus reply");
            peer
        };
        let mut mutation_peer = establish_client();
        let mut query_peer = establish_client();

        sender.send(Message::VtAcquire).unwrap();
        let mut set_request = vec![128, yserver_protocol::x11::randr::RR_SET_CRTC_CONFIG, 8, 0];
        set_request.extend_from_slice(&set_crtc_body(3));
        mutation_peer
            .write_all(&set_request)
            .expect("send SetCrtcConfig");
        // The round trip proves the core has processed the preceding
        // mutation request. It bypasses the RANDR gate, while the mutation
        // remains parked behind the episode begun by VtAcquire.
        query_peer
            .write_all(&[43, 0, 1, 0])
            .expect("send GetInputFocus");
        let mut before_end = [0; 32];
        query_peer
            .read_exact(&mut before_end)
            .expect("query replies while acquire episode is open");
        assert_eq!(before_end[0], 1, "query received an X11 reply");
        assert_eq!(
            u16::from_le_bytes(before_end[2..4].try_into().unwrap()),
            2,
            "query replies while SetCrtcConfig stays parked"
        );
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, RecordedCall::ApplyCrtcConfig { .. })),
            "SetCrtcConfig must not dispatch before EpisodeEnd"
        );

        episodes
            .enqueue(TopologyEpisodeEvent::EpisodeEnd(77, None))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline
            && !calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, RecordedCall::ApplyCrtcConfig { .. }))
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, RecordedCall::ApplyCrtcConfig { .. })),
            "EpisodeEnd releases the queued mutation"
        );
        sender.send(Message::Shutdown).unwrap();
        let (result, _backend) = core.join().unwrap();
        result.unwrap();
        drop(mutation_peer);
        drop(query_peer);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn server_grab_blocks_only_non_owner_requests() {
        let mut state = ServerState::new();
        state.server_grab_owner = Some(yserver_protocol::x11::ClientId(7));

        assert!(!blocked_by_server_grab(&state, &deferred_request(7, 127)));
        assert!(blocked_by_server_grab(&state, &deferred_request(8, 127)));
        state.server_grab_owner = None;
        assert!(!blocked_by_server_grab(&state, &deferred_request(8, 127)));
    }

    #[test]
    fn released_server_grab_waiters_join_round_robin_in_waiter_order() {
        let mut deferred = FairRequestQueue::default();
        deferred.push_back(deferred_request(9, 90));
        let mut waiters = VecDeque::from([deferred_request(2, 20), deferred_request(3, 30)]);

        release_server_grab_waiters(&mut deferred, &mut waiters, &mut LoopTelemetry::default());

        let mut order = Vec::new();
        while let Some(req) = deferred.pop_front() {
            order.push((req.id.0, req.header.opcode));
        }
        assert_eq!(order, [(9, 90), (2, 20), (3, 30)]);
        assert!(waiters.is_empty());
    }

    #[test]
    fn released_server_grab_prefix_stays_ahead_of_same_client_suffix() {
        let mut deferred = FairRequestQueue::default();
        // Requests 16 and 17 were popped and parked while another client held
        // GrabServer. Requests 18 and 19 were already the remaining suffix in
        // the fair queue. The old release path appended the parked prefix and
        // dispatched 18,19,16,17, corrupting Xlib/XCB sequence tracking.
        deferred.push_back(deferred_request(66, 18));
        deferred.push_back(deferred_request(66, 19));
        deferred.push_back(deferred_request(12, 90));
        let mut waiters = VecDeque::from([deferred_request(66, 16), deferred_request(66, 17)]);

        release_server_grab_waiters(&mut deferred, &mut waiters, &mut LoopTelemetry::default());

        let mut client_66_order = Vec::new();
        while let Some(req) = deferred.pop_front() {
            if req.id.0 == 66 {
                client_66_order.push(req.header.opcode);
            }
        }
        assert_eq!(client_66_order, [16, 17, 18, 19]);
        assert!(waiters.is_empty());
    }

    #[test]
    fn fair_queue_round_robins_clients_and_preserves_each_clients_order() {
        let mut queue = FairRequestQueue::default();
        queue.push_back(deferred_request(57, 1));
        queue.push_back(deferred_request(57, 2));
        queue.push_back(deferred_request(12, 10));
        queue.push_back(deferred_request(57, 3));
        queue.push_back(deferred_request(12, 11));

        let mut order = Vec::new();
        while let Some(req) = queue.pop_front() {
            order.push((req.id.0, req.header.opcode));
        }
        assert_eq!(order, [(57, 1), (12, 10), (57, 2), (12, 11), (57, 3)]);
        assert!(queue.is_empty());
    }

    #[test]
    fn pending_backend_request_blocks_only_its_clients_fifo() {
        use yserver_protocol::x11::ClientByteOrder;

        let blocked = yserver_protocol::x11::ClientId(57);
        let other = yserver_protocol::x11::ClientId(12);
        let token = CrtcConfigToken(91);
        let mut pending = PendingBackendRequests::default();
        pending
            .park_crtc(ParkedCrtcConfig {
                token,
                client_id: blocked,
                sequence: yserver_protocol::x11::SequenceNumber(1),
                reply: CrtcConfigReply {
                    byte_order: ClientByteOrder::LittleEndian,
                },
                request_wire_bytes: 28,
            })
            .unwrap();

        let state = ServerState::new();
        let mut queue = FairRequestQueue::default();
        queue.push_back(deferred_request(blocked.0, 2));
        queue.push_back(deferred_request(other.0, 10));
        queue.push_back(deferred_request(blocked.0, 3));

        let runnable = queue.pop_front_unblocked(&pending, &state).unwrap();
        assert_eq!((runnable.id, runnable.header.opcode), (other, 10));
        assert!(
            queue.pop_front_unblocked(&pending, &state).is_none(),
            "later requests from the pending client must stay parked"
        );
        assert!(
            !queue.has_runnable(&pending, &state),
            "a blocked-only queue must not force a zero-timeout poll spin"
        );

        pending.take_crtc(token).unwrap();
        let first = queue.pop_front_unblocked(&pending, &state).unwrap();
        let second = queue.pop_front_unblocked(&pending, &state).unwrap();
        assert_eq!((first.header.opcode, second.header.opcode), (2, 3));
    }

    /// A client suspended by SYNC Await (Xorg `IgnoreClient`) keeps its
    /// later requests queued in order while other clients run, the queue
    /// does not spin the poll on it, and resuming releases them in order.
    #[test]
    fn sync_await_suspends_only_the_awaiting_client() {
        let awaiting = yserver_protocol::x11::ClientId(57);
        let other = yserver_protocol::x11::ClientId(12);
        let pending = PendingBackendRequests::default();
        let mut state = ServerState::new();
        state
            .sync_awaits
            .insert(awaiting.0, crate::server::SyncAwait::default());

        let mut queue = FairRequestQueue::default();
        queue.push_back(deferred_request(awaiting.0, 2));
        queue.push_back(deferred_request(other.0, 10));
        queue.push_back(deferred_request(awaiting.0, 3));

        let runnable = queue.pop_front_unblocked(&pending, &state).unwrap();
        assert_eq!((runnable.id, runnable.header.opcode), (other, 10));
        assert!(queue.pop_front_unblocked(&pending, &state).is_none());
        assert!(!queue.has_runnable(&pending, &state));

        state.sync_awaits.remove(&awaiting.0);
        assert!(queue.has_runnable(&pending, &state));
        let first = queue.pop_front_unblocked(&pending, &state).unwrap();
        let second = queue.pop_front_unblocked(&pending, &state).unwrap();
        assert_eq!((first.header.opcode, second.header.opcode), (2, 3));
    }

    /// A RECORD data connection whose stream write failed is no longer
    /// recording, but its pipelined requests must not run before the core
    /// loop disconnects it.
    #[test]
    fn failed_record_client_keeps_its_queued_requests_parked() {
        let failed = yserver_protocol::x11::ClientId(57);
        let other = yserver_protocol::x11::ClientId(12);
        let pending = PendingBackendRequests::default();
        let mut state = ServerState::new();
        state.record.fail_recorder_for_test(failed);
        assert!(!crate::core_loop::record::client_is_recording(
            &state, failed
        ));

        let mut queue = FairRequestQueue::default();
        queue.push_back(deferred_request(failed.0, 2));
        queue.push_back(deferred_request(other.0, 10));

        let runnable = queue.pop_front_unblocked(&pending, &state).unwrap();
        assert_eq!((runnable.id, runnable.header.opcode), (other, 10));
        assert!(queue.pop_front_unblocked(&pending, &state).is_none());
        assert!(!queue.has_runnable(&pending, &state));
        assert_eq!(
            crate::core_loop::record::take_failed_recorders(&mut state),
            [failed]
        );
    }

    #[test]
    fn ready_crtc_completion_replies_unblocks_and_returns_reader_credit() {
        use crate::{backend::recording::RecordingBackend, server::ClientState};
        use std::{
            collections::{HashMap, HashSet, VecDeque},
            io::Read,
            os::unix::net::UnixStream,
            sync::{Arc, Mutex, atomic::AtomicU16},
        };
        use yserver_protocol::x11::{ClientByteOrder, ClientId, SequenceNumber};

        let client_id = ClientId(57);
        let token = CrtcConfigToken(92);
        let (mut peer, writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let (control_tx, control_rx) = crossbeam_channel::unbounded();
        let mut state = ServerState::new();
        state.clients.insert(
            client_id.0,
            ClientState {
                writer: Arc::new(Mutex::new(crate::transport::Transport::Unix(writer))),
                byte_order: ClientByteOrder::LittleEndian,
                last_sequence: Arc::new(AtomicU16::new(0)),
                resource_id_base: 0,
                resource_id_mask: u32::MAX,
                event_masks: HashMap::new(),
                save_set: HashSet::new(),
                big_requests_enabled: false,
                xi2_masks: HashMap::new(),
                xi1_event_classes: HashSet::new(),
                xi1_window_event_classes: HashMap::new(),
                outbound: VecDeque::new(),
                watching_writable: false,
                focused_window: crate::resources::ROOT_WINDOW,
                reader_control: Some(control_tx),
                is_local: true,
                fd_passing: true,
            },
        );

        let publication = CrtcConfigPublication {
            client_id,
            sequence: SequenceNumber(9),
            output_id: state.randr.outputs[0].output_id,
            requested_mode: None,
            x: 0,
            y: 0,
            set_time: 123,
            output_bbox_before: enabled_output_bbox(&state),
            apply_transform: None,
            apply_rotation: None,
            reply_kind: crate::core_loop::process_request::CrtcConfigReplyKind::CrtcConfig,
        };
        let reply = CrtcConfigReply {
            byte_order: ClientByteOrder::LittleEndian,
        };
        let mut pending = PendingBackendRequests::default();
        pending
            .park_crtc(ParkedCrtcConfig {
                token,
                client_id,
                sequence: SequenceNumber(9),
                reply,
                request_wire_bytes: 28,
            })
            .unwrap();
        let mut gate = RandrMutationGate {
            in_flight: Some(RandrGateFlight {
                kind: RandrGateFlightKind::Mutation,
                token: Some(token),
                publication: Some(publication),
            }),
            ..RandrMutationGate::default()
        };
        let mut backend = RecordingBackend::new();
        backend.ready_crtc_configs.push(token);
        backend.crtc_config_results.insert(token, Ok(false));
        drain_ready_crtc_configs_with_gate(
            &mut state,
            &mut backend,
            &mut pending,
            &mut gate,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
        );

        let mut reply = [0_u8; 32];
        peer.read_exact(&mut reply).unwrap();
        assert_eq!((reply[0], reply[1]), (1, 0), "success reply, status=0");
        assert!(!pending.client_is_blocked(client_id));
        assert_eq!(backend.finished_crtc_configs, [token]);
        assert!(backend.cancelled_crtc_configs.is_empty());
        assert!(matches!(
            control_rx.try_recv(),
            Ok(crate::server::ReaderControl::GrantRequestBytes(28))
        ));

        // Disconnecting a still-pending client cancels its backend token and
        // never waits for the worker to finish.
        let cancel_token = CrtcConfigToken(93);
        pending
            .park_crtc(ParkedCrtcConfig {
                token: cancel_token,
                client_id,
                sequence: SequenceNumber(10),
                reply: CrtcConfigReply {
                    byte_order: ClientByteOrder::LittleEndian,
                },
                request_wire_bytes: 28,
            })
            .unwrap();
        let mut xi_config_lane = XiConfigLane::default();
        disconnect_with_xi_pending_cleanup(
            &mut state,
            &mut backend,
            &mut pending,
            &mut RandrMutationGate::default(),
            &mut xi_config_lane,
            &mut ResetTrigger::new(ResetPolicy::NoReset),
            client_id,
        );
        assert_eq!(backend.cancelled_crtc_configs, [cancel_token]);
        assert!(!state.clients.contains_key(&client_id.0));
    }

    /// The grab owner can be dropped by paths that carry no release check of
    /// their own — `process_disconnect` runs at two sites outside the message
    /// loop (a failed outbound write, and the writable-interest reconcile).
    /// The loop therefore re-checks once per iteration. This pins the state
    /// that made that necessary: waiters parked while `deferred_requests` is
    /// EMPTY, because the poll timeout keys off `deferred_requests` alone, so
    /// a waiter left in the side queue would strand its client until
    /// unrelated traffic happened to wake the loop.
    #[test]
    fn owner_disconnect_outside_the_message_loop_still_frees_waiters() {
        let mut state = ServerState::new();
        state.server_grab_owner = Some(yserver_protocol::x11::ClientId(1));
        let mut deferred = FairRequestQueue::default();
        let mut waiters = VecDeque::from([deferred_request(2, 20)]);

        // While the grab is held, a waiter must stay parked.
        if state.server_grab_owner.is_none() {
            release_server_grab_waiters(&mut deferred, &mut waiters, &mut LoopTelemetry::default());
        }
        assert_eq!(waiters.len(), 1, "grab still held: waiter stays parked");
        assert!(deferred.is_empty(), "nothing runnable while grabbed");

        // Owner reaped by a path with no release check of its own (this is
        // what process_disconnect does at run.rs' two non-message sites).
        state.server_grab_owner = None;

        // The per-iteration re-check must pick it up on its own.
        if state.server_grab_owner.is_none() {
            release_server_grab_waiters(&mut deferred, &mut waiters, &mut LoopTelemetry::default());
        }
        assert!(waiters.is_empty(), "released grab must free its waiters");
        assert_eq!(deferred.pop_front().map(|r| r.id.0), Some(2));
        assert!(deferred.is_empty());
    }
    use crate::{
        backend::recording::RecordingBackend,
        core_loop::sender::channel,
        server::{ScreenSaverActive, ServerState},
    };
    use std::time::Duration;

    /// I5 test: `reconcile_client_writable_interest` toggles a client's
    /// `watching_writable` flag in lock-step with `outbound`'s emptiness,
    /// and is a no-op when nothing changed. Tests against a real
    /// `mio::Registry` so the reregister error path is also exercised.
    #[test]
    fn reconcile_writable_interest_tracks_outbound_state() {
        use crate::server::ClientState;
        use mio::{Interest, Poll, unix::SourceFd};
        use std::{
            collections::{HashMap, HashSet, VecDeque},
            io::{Read, Write},
            os::{fd::AsRawFd, unix::net::UnixStream},
            sync::{Arc, Mutex, atomic::AtomicU16},
        };
        use yserver_protocol::x11::{ClientByteOrder, ClientId as Cid};

        let poll = Poll::new().unwrap();
        // We just need a real fd registered with the poller.
        let (mut peer, writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let writer_arc = Arc::new(Mutex::new(crate::transport::Transport::Unix(writer)));
        let raw = writer_arc.lock().unwrap().as_raw_fd();
        let token = client_token(Cid(7));
        poll.registry()
            .register(&mut SourceFd(&raw), token, Interest::READABLE)
            .unwrap();

        let mut state = ServerState::new();
        state.clients.insert(
            7,
            ClientState {
                writer: writer_arc,
                byte_order: ClientByteOrder::LittleEndian,
                last_sequence: Arc::new(AtomicU16::new(0)),
                resource_id_base: 0,
                resource_id_mask: 0,
                event_masks: HashMap::new(),
                save_set: HashSet::new(),
                big_requests_enabled: false,
                xi2_masks: HashMap::new(),
                xi1_event_classes: HashSet::new(),
                xi1_window_event_classes: HashMap::new(),
                outbound: VecDeque::new(),
                watching_writable: false,
                focused_window: crate::resources::ROOT_WINDOW,
                reader_control: None,
                is_local: true,
                fd_passing: true,
            },
        );

        // outbound is empty, watching_writable is false → no-op.
        let disc = reconcile_client_writable_interest(poll.registry(), &mut state);
        assert!(disc.is_empty());
        assert!(!state.clients[&7].watching_writable);

        // Outbound becomes non-empty AND the peer doesn't read → reconcile's
        // proactive drain attempt cannot empty it, so watching_writable
        // flips on.
        //
        // Fill the kernel buffer first so any drain attempt returns
        // WouldBlock instead of writing through to `peer`.
        //
        // Fill until the kernel actually reports WouldBlock rather than
        // writing one fixed-size buffer: the capacity is a tunable the
        // test cannot assume. On the Linux box this was reported from,
        // `net.core.wmem_default` was the stock 212992 (~228 KiB
        // absorbed), so a single 256 KiB write cleared it by only ~11%
        // and was swallowed whole where that sysctl had been raised;
        // other platforms size it differently again. The drain
        // inside reconcile then succeeded, `outbound` emptied, and
        // `watching_writable` never flipped on — #107.
        //
        // SO_SNDBUF is also raised, best-effort, so a machine with the
        // stock sysctl still exercises the large-buffer case. It is only
        // an amplifier: the kernel may clamp it (Linux) or refuse it
        // (FreeBSD ENOBUFS), and the loop below is correct either way,
        // so the result is deliberately not asserted on.
        unsafe {
            let sz: libc::c_int = 512 * 1024;
            libc::setsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                std::ptr::addr_of!(sz).cast(),
                u32::try_from(std::mem::size_of::<libc::c_int>()).unwrap(),
            );
        }
        let chunk = vec![0xABu8; 64 * 1024];
        let mut filled = false;
        // Bounded so a kernel that never reports WouldBlock fails the
        // assertion below instead of spinning.
        for _ in 0..1024 {
            match state.clients[&7].writer.lock().unwrap().write(&chunk) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    filled = true;
                    break;
                }
                // Anything else (EPIPE, EINTR…) is a broken fixture, not
                // a full buffer — surface it rather than folding it into
                // the generic "never reported WouldBlock" failure.
                Err(err) => panic!("unexpected error filling the send buffer: {err}"),
            }
        }
        assert!(filled, "kernel send buffer never reported WouldBlock");
        state
            .clients
            .get_mut(&7)
            .unwrap()
            .outbound
            .extend([1u8, 2, 3]);
        let disc = reconcile_client_writable_interest(poll.registry(), &mut state);
        assert!(disc.is_empty());
        assert!(state.clients[&7].watching_writable);

        // Peer drains → kernel buffer empties → drain succeeds inside reconcile,
        // outbound goes empty, watching_writable flips off.
        //
        // Read until WouldBlock for the same reason the fill loops: one
        // read of a fixed size is not guaranteed to empty the queue, and
        // leftover bytes would make reconcile's drain block again and
        // leave `outbound` non-empty.
        let mut sink = vec![0u8; 64 * 1024];
        peer.set_nonblocking(true).unwrap();
        loop {
            match peer.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected error draining the peer: {err}"),
            }
        }
        let disc = reconcile_client_writable_interest(poll.registry(), &mut state);
        assert!(disc.is_empty());
        assert!(state.clients[&7].outbound.is_empty());
        assert!(!state.clients[&7].watching_writable);

        drop(peer);
    }

    /// Reusing an fd number for a new open description during before_block
    /// must refresh the core registration even when `(fd, kind)` is unchanged.
    #[test]
    fn backend_poll_generation_refreshes_reused_fd_description() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{io::Write, os::fd::FromRawFd};

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (old_reader, old_writer) = UnixStream::pair().unwrap();
        let old_fd = old_reader.as_raw_fd();
        drop(old_writer);
        let (new_reader, mut new_writer) = UnixStream::pair().unwrap();
        let mut old_reader = Some(old_reader);
        let mut new_reader = Some(new_reader);
        let (swapped_tx, swapped_rx) = crossbeam_channel::bounded(1);
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let mut backend = RecordingBackend::new()
            .with_poll_sources(vec![(old_fd, BackendFdKind::Drm)], ready_tx)
            .with_before_block_action(move |backend| {
                let Some(old_reader) = old_reader.take() else {
                    return;
                };
                let new_reader = new_reader.take().expect("new reader exists on first pass");
                let new_fd = new_reader.as_raw_fd();
                // Replacing the numeric descriptor closes its old open
                // description, which also removes the old epoll registration.
                let result = unsafe { libc::dup2(new_fd, old_fd) };
                assert_eq!(result, old_fd, "dup2 must reuse the old fd number");
                drop(new_reader);
                std::mem::forget(old_reader);
                backend.replace_poll_sources_for_tests(vec![(old_fd, BackendFdKind::Drm)]);
                let _ = swapped_tx.send(());
            });

        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        swapped_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("before_block replaced the open description");
        new_writer.write_all(&[1]).unwrap();
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            old_fd,
            "readiness from the reused descriptor must be dispatched"
        );
        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().0.unwrap();
        // dup2 installed the replacement description at this number; own
        // and close it now that the core has dropped its registration.
        let _replacement_reader = unsafe { OwnedFd::from_raw_fd(old_fd) };
    }

    /// Multi-device regression: two DRM fds of the same kind must get
    /// distinct poll tokens, and readiness on the second fd must carry
    /// that exact fd through `Backend::on_page_flip_ready`.
    #[test]
    fn drm_readiness_routes_to_the_exact_backend_fd() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{io::Write, os::fd::AsRawFd};

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (drm_reader_a, _drm_writer_a) = UnixStream::pair().unwrap();
        let (drm_reader_b, mut drm_writer_b) = UnixStream::pair().unwrap();
        let drm_fd_a = drm_reader_a.as_raw_fd();
        let drm_fd_b = drm_reader_b.as_raw_fd();
        let (ready_tx, ready_rx) = crossbeam_channel::unbounded();
        let mut backend = RecordingBackend::new().with_poll_sources(
            vec![
                (drm_fd_a, BackendFdKind::Drm),
                (drm_fd_b, BackendFdKind::Drm),
            ],
            ready_tx,
        );
        let handle = std::thread::spawn(move || {
            // `RecordingBackend` intentionally stores only raw fds; keep
            // their owners alive for the duration of the core loop.
            let _drm_readers = (drm_reader_a, drm_reader_b);
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        drm_writer_b.write_all(&[1]).unwrap();
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            drm_fd_b,
            "readiness from the second DRM source must retain its fd identity"
        );
        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        let (result, backend) = handle.join().unwrap();
        result.unwrap();
        let dispatched_fds = backend.page_flip_fds.lock().unwrap();
        assert!(
            !dispatched_fds.is_empty(),
            "the readable DRM source must be dispatched"
        );
        assert!(
            dispatched_fds.iter().all(|fd| *fd == drm_fd_b),
            "idle DRM fd {drm_fd_a} was dispatched: {dispatched_fds:?}",
        );
    }

    /// A fake XDMCP manager on loopback, for the two loop-level tests
    /// below. The service-level behaviour is covered in `core_loop::xdmcp`;
    /// what these prove is the *plumbing* — the UDP socket really is in
    /// this poll set, its readiness really is dispatched, and the reset
    /// hook really runs after the new generation is installed.
    #[cfg(feature = "xdmcp")]
    struct XdmcpManagerFixture {
        socket: std::net::UdpSocket,
    }

    #[cfg(feature = "xdmcp")]
    impl XdmcpManagerFixture {
        fn new() -> Self {
            let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            Self { socket }
        }

        fn service(&self, once: bool) -> XdmcpService {
            use crate::core_loop::xdmcp::{XdmcpMode, XdmcpSetup};
            XdmcpService::bind(&XdmcpSetup {
                mode: XdmcpMode::Query("127.0.0.1".into()),
                port: self.socket.local_addr().unwrap().port(),
                from: Some("127.0.0.1".into()),
                class: None,
                display_id: None,
                once,
                display_number: 7,
            })
            .unwrap()
        }

        fn expect(
            &self,
            what: &str,
        ) -> (yserver_protocol::xdmcp::XdmcpMessage, std::net::SocketAddr) {
            let mut buf = [0_u8; 8192];
            let (len, from) = self
                .socket
                .recv_from(&mut buf)
                .unwrap_or_else(|e| panic!("no {what} from the display: {e}"));
            (
                yserver_protocol::xdmcp::decode_message(&buf[..len]).unwrap(),
                from,
            )
        }

        fn send(&self, to: std::net::SocketAddr, message: &yserver_protocol::xdmcp::XdmcpMessage) {
            let packet = yserver_protocol::xdmcp::encode_message(message).unwrap();
            self.socket.send_to(&packet, to).unwrap();
        }
    }

    /// The socket is registered with the core poller, its readiness is
    /// dispatched, and a reset re-queries — from the loop, not from a
    /// hand-driven service.
    #[cfg(feature = "xdmcp")]
    #[test]
    fn the_xdmcp_socket_is_polled_and_a_reset_re_queries() {
        use crate::backend::recording::RecordingBackend;
        use yserver_protocol::xdmcp::XdmcpMessage;

        let manager = XdmcpManagerFixture::new();
        let service = manager.service(false);
        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let mut backend = RecordingBackend::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new_with_xdmcp(None, true),
                ResetPolicy::Reset,
                Some(service),
            )
        });

        let (query, display) = manager.expect("the startup Query");
        assert!(matches!(query, XdmcpMessage::Query { .. }), "{query:?}");

        manager.send(
            display,
            &XdmcpMessage::Willing {
                authentication_name: Vec::new(),
                hostname: b"fake-dm".to_vec(),
                status: b"willing".to_vec(),
            },
        );
        let (request, _) = manager.expect("a Request");
        assert!(
            matches!(request, XdmcpMessage::Request { .. }),
            "the loop did not dispatch the socket's readiness: {request:?}"
        );

        // A forced reset (the SIGHUP path) crosses the boundary; the XDMCP
        // hook then re-queries on the NEW generation.
        sender.send(Message::ResetRequested).unwrap();
        let (requery, _) = manager.expect("a re-query after the reset");
        assert!(matches!(requery, XdmcpMessage::Query { .. }), "{requery:?}");

        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().unwrap();
    }

    /// A `Terminate` from the machine ends the loop cleanly — the other
    /// half of the outcome wiring. `Failed` gets there in three packets
    /// instead of the 126 seconds a retransmission timeout would take.
    #[cfg(feature = "xdmcp")]
    #[test]
    fn an_xdmcp_terminate_ends_the_core_loop() {
        use crate::backend::recording::RecordingBackend;
        use yserver_protocol::xdmcp::{MIT_MAGIC_COOKIE_1, XdmcpMessage};

        let manager = XdmcpManagerFixture::new();
        let service = manager.service(false);
        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let mut backend = RecordingBackend::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new_with_xdmcp(None, true),
                ResetPolicy::Reset,
                Some(service),
            )
        });

        let (_, display) = manager.expect("the startup Query");
        manager.send(
            display,
            &XdmcpMessage::Willing {
                authentication_name: Vec::new(),
                hostname: b"fake-dm".to_vec(),
                status: b"willing".to_vec(),
            },
        );
        let _ = manager.expect("a Request");
        manager.send(
            display,
            &XdmcpMessage::Accept {
                session_id: 0x1234,
                authentication_name: Vec::new(),
                authentication_data: Vec::new(),
                authorization_name: MIT_MAGIC_COOKIE_1.to_vec(),
                authorization_data: b"cookie".to_vec(),
            },
        );
        let _ = manager.expect("a Manage");
        manager.send(
            display,
            &XdmcpMessage::Failed {
                session_id: 0x1234,
                status: b"no session for you".to_vec(),
            },
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "a fatal XDMCP packet did not stop the loop"
        );
        handle.join().unwrap().unwrap();
        drop(sender);
    }
    /// A setup that lost the `Refuse` race is dropped as orphaned — and
    /// that drop must not start a generation.
    ///
    /// `AuthState` is atomic per call, but a setup thread that already
    /// passed `check` is past that point: a `Refuse` can clear the
    /// session cookie while the thread is one instruction from sending
    /// its `ClientSetupComplete`. `XdmcpService::note_client_established`
    /// reports that loser and the loop disconnects it. Under XDMCP the
    /// policy is an implied `-reset`, so if the completion had armed the
    /// reset trigger, the orphan's *own* disconnect would drain an armed
    /// client set and cross the generation boundary — tearing down the
    /// negotiation that is at that very moment retrying its `Request`.
    ///
    /// Arming therefore belongs to the caller, after XDMCP admission has
    /// decided. What this pins is that decision order: the orphan goes
    /// away, the generation does not move, and the manager's outstanding
    /// offer is undisturbed. The injected message stands in for the
    /// racing setup thread exactly as it reaches the loop — a **remote**
    /// client, because the orphan rule deliberately spares local ones
    /// (Xorg's `XdmcpOpenDisplay` ignores a unix client, which the XDMCP
    /// cookie never authorized).
    /// `is_local` is an ADDRESS property. A TCP peer on this machine is a
    /// local client — Xorg's `xtransLocalClient` says so — and therefore
    /// keeps MIT-SHM, whose legacy `Attach` passes a SysV shmid rather
    /// than a descriptor and so works fine without fd passing.
    ///
    /// Deriving it from the transport instead, as this did until
    /// 2026-09-10, refused shared memory to a same-machine XDMCP session
    /// (`DISPLAY=127.0.0.1:1`) and pushed every image over the wire.
    #[test]
    fn a_tcp_peer_on_this_machine_is_a_local_client() {
        use std::net::{IpAddr, Ipv4Addr};

        assert!(
            address_is_ours(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "127.0.0.1 is ours"
        );
        assert!(
            address_is_ours(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))),
            "the whole loopback range is ours, not just 127.0.0.1"
        );
        // TEST-NET-3 (RFC 5737): reserved for documentation, so it cannot
        // be a real interface address on the machine running this test.
        assert!(
            !address_is_ours(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))),
            "a documentation-range address is not ours"
        );
    }

    #[cfg(feature = "xdmcp")]
    #[test]
    fn an_orphaned_xdmcp_client_does_not_reset_the_generation() {
        use crate::backend::recording::RecordingBackend;
        use std::io::Read;
        use yserver_protocol::{
            x11::ClientByteOrder,
            xdmcp::{MIT_MAGIC_COOKIE_1, XdmcpMessage, decode_message},
        };

        /// How long the "no reset happened" assertions watch for. The
        /// XDMCP retransmit floor is `XDM_MIN_RTX` = 2 s, so nothing the
        /// healthy machine does can land inside this window; a reset's
        /// re-query would land immediately.
        const QUIET: Duration = Duration::from_millis(400);
        const SESSION: u32 = 0x1234;

        let manager = XdmcpManagerFixture::new();
        let service = manager.service(false);
        let (poll, sender, rx) = channel().unwrap();
        // The generation is read from the counter, not inferred from
        // timing: the boundary bumps it and nothing else in the loop
        // does.
        let generations = rx.generation_counter();
        let sender_for_core = sender.clone_handle();
        let client_ids = std::sync::Arc::new(ClientIdAllocator::new());
        let client_ids_for_core = client_ids.clone();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let mut backend = RecordingBackend::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &client_ids_for_core,
                AuthState::new_with_xdmcp(None, true),
                ResetPolicy::Reset,
                Some(service),
            )
        });

        let start_generation = generations.current();

        // Query -> Willing -> Request -> Accept installs the session
        // cookie, and the machine answers with Manage.
        let (query, display) = manager.expect("the startup Query");
        assert!(matches!(query, XdmcpMessage::Query { .. }), "{query:?}");
        manager.send(
            display,
            &XdmcpMessage::Willing {
                authentication_name: Vec::new(),
                hostname: b"fake-dm".to_vec(),
                status: b"willing".to_vec(),
            },
        );
        let (request, _) = manager.expect("a Request");
        assert!(
            matches!(request, XdmcpMessage::Request { .. }),
            "{request:?}"
        );
        manager.send(
            display,
            &XdmcpMessage::Accept {
                session_id: SESSION,
                authentication_name: Vec::new(),
                authentication_data: Vec::new(),
                authorization_name: MIT_MAGIC_COOKIE_1.to_vec(),
                authorization_data: b"cookie".to_vec(),
            },
        );
        let (manage, _) = manager.expect("a Manage");
        assert!(matches!(manage, XdmcpMessage::Manage { .. }), "{manage:?}");

        // The Refuse clears the cookie and sends the machine back round
        // to Request. Reading that retry is the synchronisation point:
        // it cannot be on the wire until the Refuse has been fully
        // applied, so the injection below is unambiguously *after* the
        // clear. No generation change — the offer is being retried, not
        // abandoned.
        manager.send(
            display,
            &XdmcpMessage::Refuse {
                session_id: SESSION,
            },
        );
        let (retry, _) = manager.expect("the Request retry after the Refuse");
        assert!(
            matches!(retry, XdmcpMessage::Request { .. }),
            "a Refuse must resend the Request, got {retry:?}"
        );
        assert_eq!(
            generations.current(),
            start_generation,
            "a Refuse retries the offer; it does not cross a boundary"
        );

        // The racing setup thread's completion, arriving now.
        let orphan = client_ids.allocate();
        let (core_side, mut peer) = UnixStream::pair().expect("socketpair");
        peer.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let stale = sender.bind();
        stale
            .send(Message::ClientSetupComplete {
                id: orphan,
                generation: stale.generation(),
                stream: Transport::Unix(core_side),
                resource_id_base: 0x0020_0000,
                resource_id_mask: 0x000F_FFFF,
                byte_order: ClientByteOrder::LittleEndian,
                // Remote: only a TCP client can have been authorized by
                // the session credential the Refuse just revoked.
                is_local: false,
                fd_passing: false,
                setup_reply: Vec::new(),
            })
            .expect("send the racing completion");

        // 1. The orphan is dropped. `process_disconnect` shuts the
        //    socket down on both sides, so this is EOF, not a timeout.
        let mut sink = [0_u8; 1];
        match peer.read(&mut sink) {
            Ok(0) => {}
            other => panic!("a client with no session must be disconnected; read {other:?}"),
        }

        // Watch the manager socket for a while before judging anything.
        // The boundary is crossed at the *end* of the iteration the
        // disconnect ran in, so EOF above races the bump by microseconds
        // — this window is what makes the two assertions below decisive
        // rather than a coin flip. Collect only; asserting inside the
        // loop would let the re-query fire first and hide which of the
        // two actually broke.
        manager
            .socket
            .set_read_timeout(Some(QUIET))
            .expect("quiet-window timeout");
        let mut buf = [0_u8; 8192];
        let mut seen = Vec::new();
        let deadline = Instant::now() + QUIET;
        while Instant::now() < deadline {
            let len = match manager.socket.recv_from(&mut buf) {
                Ok((len, _)) => len,
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(err) => panic!("unexpected error reading the manager socket: {err}"),
            };
            seen.push(decode_message(&buf[..len]).expect("decode"));
        }

        // 2. The regression itself.
        assert_eq!(
            generations.current(),
            start_generation,
            "an orphaned client never became established; its disconnect must not \
             drain an armed session and start a new generation"
        );

        // 3. And the negotiation carried on untouched: the retry read
        //    above is the manager's Request, and no Query followed it —
        //    a Query is what a reset's re-query looks like.
        assert!(
            !seen
                .iter()
                .any(|message| matches!(message, XdmcpMessage::Query { .. })),
            "the display re-queried mid-negotiation: {seen:?}"
        );

        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn copied_scanout_completion_fd_dispatches_dedicated_hook() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{io::Write, os::fd::AsRawFd, sync::atomic::Ordering};

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (completion_reader, mut completion_writer) = UnixStream::pair().unwrap();
        let completion_fd = completion_reader.as_raw_fd();
        let (unused_page_tx, _unused_page_rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::unbounded();
        let mut backend = RecordingBackend::new()
            .with_poll_sources(
                vec![(completion_fd, BackendFdKind::ScanoutRenderCompletion)],
                unused_page_tx,
            )
            .with_scanout_render_completion_notification(ready_tx);
        let handle = std::thread::spawn(move || {
            // `RecordingBackend` stores only the raw fd, so retain its owner
            // until `run_core` has unregistered every backend source.
            let _completion_reader = completion_reader;
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        completion_writer.write_all(&[1]).unwrap();
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        let (result, backend) = handle.join().unwrap();
        result.unwrap();
        assert!(
            backend
                .scanout_render_completion_count
                .load(Ordering::Relaxed)
                >= 1,
            "readiness must dispatch the copied-scanout completion hook"
        );
        assert_eq!(
            backend.page_flip_count.load(Ordering::Relaxed),
            0,
            "copied-scanout readiness must not be misrouted as a DRM page flip"
        );
    }

    /// Regression (project_reclamation_starvation_leak): the core loop
    /// must drive backend GPU-resource reclamation (`before_block`) every
    /// iteration, INDEPENDENT of page-flips. The KMS v2 backend reaped
    /// per-op command buffers only from `on_page_flip_ready`; while the
    /// display was dark (DPMS-off / standby / VT-away) no flips occurred,
    /// so a client that kept drawing grew the engine `submitted` queue
    /// without bound until the GPU lost its device. Here we run the loop
    /// with ZERO DRM readiness events and assert `before_block` still
    /// fired — i.e. reclamation rides the dispatch loop, not scanout.
    #[test]
    fn before_block_runs_without_any_page_flip() {
        use crate::backend::recording::RecordingBackend;
        use std::sync::atomic::Ordering;

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let mut backend = RecordingBackend::new();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });
        // No DRM readiness — only a Shutdown. The loop must still run at
        // least one iteration, calling before_block before it blocks.
        sender.send(Message::Shutdown).unwrap();
        // Generous deadline so a slow/loaded CI box can't spuriously fail:
        // the loop breaks the instant the thread finishes, so this only
        // bounds the pathological-hang case.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        let (result, backend) = handle.join().unwrap();
        result.unwrap();
        assert_eq!(
            backend.page_flip_count.load(Ordering::Relaxed),
            0,
            "test must exercise the no-page-flip path",
        );
        assert!(
            backend.before_block_count.load(Ordering::Relaxed) >= 1,
            "before_block must run each iteration even with no page-flips \
             (reclamation must not be gated on scanout)",
        );
    }

    #[test]
    fn executor_control_readiness_dispatches_the_executor_hook() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{
            io::Write,
            os::{fd::AsRawFd, unix::net::UnixStream},
        };

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (control_reader, mut control_writer) = UnixStream::pair().unwrap();
        let control_fd = control_reader.as_raw_fd();
        let (unused_page_tx, _unused_page_rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::unbounded();
        let mut backend = RecordingBackend::new()
            .with_poll_sources(
                vec![(control_fd, BackendFdKind::ExecutorControl)],
                unused_page_tx,
            )
            .with_executor_readable_notification(ready_tx);
        let handle = std::thread::spawn(move || {
            let _control_reader = control_reader;
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            let result = run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            );
            (result, backend)
        });

        control_writer.write_all(&[1]).unwrap();
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("executor control readiness must reach on_executor_readable");
        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().0.unwrap();
    }

    /// The watchdog cannot fire from a loop that is blocked with no deadline.
    /// This proves the backend's deadline actually bounds the core's poll: with
    /// no fd ever becoming readable and no message sent, `before_block` must
    /// still be reached again shortly after the declared deadline.
    #[test]
    fn a_backend_deadline_wakes_the_core_with_no_fd_activity() {
        use crate::backend::recording::RecordingBackend;

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (block_tx, block_rx) = crossbeam_channel::unbounded();
        let mut backend = RecordingBackend::new()
            .with_wakeup_deadline(Instant::now() + Duration::from_millis(50))
            .with_before_block_notification(block_tx);
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            )
        });

        // At least two block-handler passes with no fd and no message: one before
        // the deadline and one after it. A core that ignored next_wakeup would
        // deliver the first and then block forever.
        block_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("first block handler");
        block_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("deadline did not wake the core");
        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().unwrap();
    }

    /// Regression: `before_block` must run BEFORE computing `poll_timeout` so that
    /// deadline updates made in `before_block` (e.g. replacing a 2s hardware deadline
    /// with a 50ms event deadline) and draining the last ready fd take effect on the
    /// immediate poll rather than blocking on the obsolete 2s timeout.
    #[test]
    fn before_block_replaces_deadline_and_poll_uses_new_deadline() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{
            io::{Read, Write},
            os::{fd::AsRawFd, unix::net::UnixStream},
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
        };

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (mut ready_reader, mut ready_writer) = UnixStream::pair().unwrap();
        ready_reader.set_nonblocking(true).unwrap();
        let ready_fd = ready_reader.as_raw_fd();
        let (unused_page_tx, _unused_page_rx) = crossbeam_channel::unbounded();

        // Write 1 byte so the fd is initially readable in poll.
        ready_writer.write_all(&[42]).unwrap();

        let initial_2s = Instant::now() + Duration::from_secs(2);
        let drained = Arc::new(AtomicBool::new(false));
        let drained_clone = Arc::clone(&drained);

        let (block_tx, block_rx) = crossbeam_channel::unbounded();

        let mut backend = RecordingBackend::new()
            .with_poll_sources(
                vec![(ready_fd, BackendFdKind::OwnerCompletion)],
                unused_page_tx,
            )
            .with_wakeup_deadline(initial_2s)
            .with_before_block_notification(block_tx)
            .with_before_block_action(move |b| {
                if !drained_clone.load(Ordering::Relaxed) {
                    // Drain the ready fd so it is no longer readable
                    let mut buf = [0u8; 16];
                    let _ = ready_reader.read(&mut buf);
                    drained_clone.store(true, Ordering::Relaxed);
                    // Replace 2s deadline with a 50ms deadline
                    b.set_wakeup_deadline(Some(Instant::now() + Duration::from_millis(50)));
                }
            });

        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            )
        });

        let start = Instant::now();
        // First before_block runs before poll; drains the fd and replaces the deadline.
        block_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("first before_block");

        // The core now blocks in poll. Because before_block ran before poll_timeout,
        // it must wake on the 50ms deadline, not the 2s deadline.
        block_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("second before_block after deadline wake");
        let elapsed = start.elapsed();

        // Must wake up well before the original 2s deadline (< 1.0s).
        assert!(
            elapsed < Duration::from_millis(1000),
            "core must wake up with the replaced 50ms deadline, but took {elapsed:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(30),
            "core must have waited for the 50ms deadline, but woke in {elapsed:?}"
        );

        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn owner_completion_fd_triggers_backend_callback() {
        use crate::backend::{BackendFdKind, recording::RecordingBackend};
        use std::{
            io::Write,
            os::{fd::AsRawFd, unix::net::UnixStream},
        };

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let (ready_reader, mut ready_writer) = UnixStream::pair().unwrap();
        ready_reader.set_nonblocking(true).unwrap();
        let ready_fd = ready_reader.as_raw_fd();
        let (unused_page_tx, _unused_page_rx) = crossbeam_channel::unbounded();

        ready_writer.write_all(&[42]).unwrap();

        let (completion_tx, completion_rx) = crossbeam_channel::unbounded();

        let mut backend = RecordingBackend::new()
            .with_poll_sources(
                vec![(ready_fd, BackendFdKind::OwnerCompletion)],
                unused_page_tx,
            )
            .with_owner_completion_ready_notification(completion_tx);

        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            )
        });

        completion_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("on_owner_completion_ready must be called when OwnerCompletion fd is readable");

        sender.send(Message::Shutdown).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(handle.is_finished(), "run_core did not return");
        handle.join().unwrap().unwrap();
    }

    /// `handle_host_input` arms the auto-repeat timer on a real
    /// KeyPress, replaces it on a different KeyPress, and clears it
    /// on the matching KeyRelease. Regression coverage for backend-owned input
    /// dispatch paths that must not call `backend.on_host_input` directly,
    /// bypassing this wrapper.
    #[test]
    fn handle_host_input_arms_repeat_state() {
        use crate::{backend::recording::RecordingBackend, host_x11::HostKeyEvent};

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        let key = |keycode: u8, pressed: bool| {
            HostInputEvent::Key(HostKeyEvent {
                origin: crate::core_loop::InputOrigin::NestedHost,
                pressed,
                keycode,
                time: 0,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state: 0,
            })
        };

        // Press A → armed on A.
        handle_host_input(&mut state, &mut backend, key(38, true));
        let armed = state
            .key_repeats
            .get(&crate::core_loop::InputOrigin::NestedHost)
            .expect("press should arm repeat state for NestedHost");
        assert_eq!(armed.event.keycode, 38);
        assert!(armed.event.pressed);

        // Press B (different keycode) → replaces armed key.
        handle_host_input(&mut state, &mut backend, key(39, true));
        let armed = state
            .key_repeats
            .get(&crate::core_loop::InputOrigin::NestedHost)
            .expect("second press should replace the same origin's repeat");
        assert_eq!(armed.event.keycode, 39);

        // Release A while B is armed → ignored (only the armed key's
        // release clears).
        handle_host_input(&mut state, &mut backend, key(38, false));
        assert!(
            state
                .key_repeats
                .contains_key(&crate::core_loop::InputOrigin::NestedHost),
            "release of non-armed key must not clear",
        );

        // Release B → clears.
        handle_host_input(&mut state, &mut backend, key(39, false));
        assert!(state.key_repeats.is_empty());
    }

    /// Regression guard for the idle free-run fix (cut 2a): the caller
    /// pokes the compositor (`mark_dirty`) only when a repeat actually
    /// fires. `fire_pending_repeats` must return `false` on the
    /// every-iteration "armed but not yet due" path (else a held/stuck
    /// key busy-spins the compositor at the loop rate) and `true` only
    /// when it fans out.
    #[test]
    fn fire_pending_repeats_reports_whether_it_fired() {
        use std::time::{Duration, Instant};

        use crate::{backend::recording::RecordingBackend, host_x11::HostKeyEvent};

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        // Nothing armed → no fire.
        assert!(!fire_pending_repeats(&mut state, &mut backend));

        // Arm a repeatable key (keycode 38 auto-repeats by default —
        // the sibling test relies on this too).
        handle_host_input(
            &mut state,
            &mut backend,
            HostInputEvent::Key(HostKeyEvent {
                origin: crate::core_loop::InputOrigin::NestedHost,
                pressed: true,
                keycode: 38,
                time: 0,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state: 0,
            }),
        );
        assert!(
            state
                .key_repeats
                .contains_key(&crate::core_loop::InputOrigin::NestedHost)
        );
        let transition = crate::core_loop::key_fanout::key_transition_status(
            &state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
        )
        .expect("NestedHost has a master keyboard");
        crate::core_loop::key_fanout::commit_key_transition(
            &mut state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
            transition,
        );

        // Freshly armed → `next_fire` is INITIAL_DELAY in the future →
        // NOT due → must return false (the idle busy-spin case).
        assert!(
            !fire_pending_repeats(&mut state, &mut backend),
            "armed-but-not-due must not report a fire",
        );

        // Force the deadline into the past → must fire.
        if let Some(s) = state
            .key_repeats
            .get_mut(&crate::core_loop::InputOrigin::NestedHost)
        {
            s.next_fire = Instant::now() - Duration::from_millis(1);
        }
        assert!(
            fire_pending_repeats(&mut state, &mut backend),
            "a due repeat must report a fire",
        );
    }

    /// A fired repeat reaches the backend as `KeyRepeat`, not device `Key`:
    /// it models Xorg's XKB soft repeat, which bypasses GetKeyboardEvents
    /// and so generates no XI2 raw key event (issue #173).
    #[test]
    fn fire_pending_repeats_sends_key_repeat_not_device_key() {
        use std::time::{Duration, Instant};

        use crate::{
            backend::recording::{RecordedCall, RecordingBackend},
            host_x11::HostKeyEvent,
        };

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        handle_host_input(
            &mut state,
            &mut backend,
            HostInputEvent::Key(HostKeyEvent {
                origin: crate::core_loop::InputOrigin::NestedHost,
                pressed: true,
                keycode: 38,
                time: 0,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state: 0,
            }),
        );
        let transition = crate::core_loop::key_fanout::key_transition_status(
            &state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
        )
        .expect("NestedHost has a master keyboard");
        crate::core_loop::key_fanout::commit_key_transition(
            &mut state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
            transition,
        );
        if let Some(s) = state
            .key_repeats
            .get_mut(&crate::core_loop::InputOrigin::NestedHost)
        {
            s.next_fire = Instant::now() - Duration::from_millis(1);
        }
        assert!(fire_pending_repeats(&mut state, &mut backend));

        let keys: Vec<RecordedCall> = backend
            .calls()
            .into_iter()
            .filter(|c| matches!(c, RecordedCall::HostKey { .. }))
            .collect();
        assert_eq!(
            keys,
            vec![
                RecordedCall::HostKey {
                    keycode: 38,
                    pressed: true,
                    repeat: false,
                },
                RecordedCall::HostKey {
                    keycode: 38,
                    pressed: false,
                    repeat: true,
                },
                RecordedCall::HostKey {
                    keycode: 38,
                    pressed: true,
                    repeat: true,
                },
            ]
        );
        let transition = crate::core_loop::key_fanout::key_transition_status(
            &state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
        )
        .expect("NestedHost has a master keyboard");
        crate::core_loop::key_fanout::commit_key_transition(
            &mut state,
            crate::core_loop::InputOrigin::NestedHost,
            38,
            true,
            transition,
        );
    }

    /// Helper: a touchpad `DeviceInfo` mirroring libinput's enumeration
    /// of a Synaptics pad (matches xinput.rs's `touchpad_info`).
    #[cfg(test)]
    fn probe_touchpad_info() -> crate::core_loop::DeviceInfo {
        use crate::core_loop::message::{BoolSetting, LibinputConfigSnapshot};
        crate::core_loop::DeviceInfo {
            source_id: crate::xinput::InputSourceId(u64::from(line!())),
            enabled: true,
            resume_key: None,
            capabilities: crate::xinput::InputCapabilities {
                keyboard: false,
                pointer: true,
                touch: false,
            },
            name: "SynPS/2 Synaptics TouchPad".into(),
            device_node: "/dev/input/event4".into(),
            sysname: "event4".into(),
            vendor_id: 0x046d,
            product_id: 0xc52f,
            is_touchpad: true,
            config: LibinputConfigSnapshot {
                tap: BoolSetting {
                    available: true,
                    current: true,
                    default: false,
                },
                natural_scroll: BoolSetting {
                    available: true,
                    current: false,
                    default: true,
                },
                dwt: BoolSetting {
                    available: true,
                    current: true,
                    default: true,
                },
                ..Default::default()
            },
        }
    }

    #[cfg(test)]
    fn xtest_pointer_name(state: &ServerState) -> String {
        state
            .xi_devices
            .iter()
            .find(|d| d.id == crate::xinput::DEVICEID_XTEST_POINTER)
            .expect("XTEST pointer (id 4) always present")
            .name
            .clone()
    }

    /// A backend with no on-core libinput (the trait default, and what
    /// Direct-mode / host-X11 / ynest present) is a clean no-op probe:
    /// returns 0 and leaves the static device model untouched.
    #[test]
    fn probe_input_devices_default_is_noop() {
        use crate::backend::recording::RecordingBackend;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new(); // no probe_rounds configured
        let before = xtest_pointer_name(&state);

        let seeded = backend.probe_input_devices(&mut state);

        assert_eq!(seeded, 0, "no-op probe seeds nothing");
        assert_eq!(
            xtest_pointer_name(&state),
            before,
            "device 4 unchanged when nothing to probe",
        );
    }

    /// A backend whose startup probe enumerates a touchpad seeds its
    /// physical facet before the serve loop while keeping XTEST 4 intact.
    #[test]
    fn probe_input_devices_seeds_touchpad_before_loop() {
        use crate::backend::recording::RecordingBackend;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        // One non-empty round (the touchpad), then libinput goes quiet.
        backend.probe_rounds.push_back(vec![probe_touchpad_info()]);

        assert_ne!(
            xtest_pointer_name(&state),
            "SynPS/2 Synaptics TouchPad",
            "precondition: device 4 starts as the virtual XTEST pointer",
        );

        let seeded = backend.probe_input_devices(&mut state);

        assert_eq!(seeded, 1, "exactly one device seeded");
        assert_eq!(
            xtest_pointer_name(&state),
            crate::xinput::registry::NAME_XTEST_POINTER,
            "device 4 remains the virtual XTEST pointer after startup probe",
        );
        let physical_pointer = state
            .xi_devices
            .iter()
            .find(|device| {
                device.name == "SynPS/2 Synaptics TouchPad"
                    && device.facet == Some(crate::xinput::XiFacetKind::PointerTouch)
            })
            .expect("startup probe registers a physical touchpad pointer facet");
        assert!(physical_pointer.id >= 6, "physical facet IDs start at 6");
        let tap_atom = state
            .atoms
            .id_for(crate::xinput::PROP_TAPPING_ENABLED)
            .unwrap();
        assert!(physical_pointer.properties.contains_key(&tap_atom));
        assert!(
            !state
                .xi_devices
                .device(crate::xinput::DEVICEID_XTEST_POINTER)
                .unwrap()
                .properties
                .contains_key(&tap_atom)
        );
    }

    /// The bounded drain TERMINATES: with libinput perpetually empty it
    /// stops after two consecutive empty rounds (not the MAX_ROUNDS
    /// ceiling), and even an adversarial always-non-empty source is
    /// capped at the ceiling rather than spinning forever.
    #[test]
    fn probe_input_devices_bounded_drain_terminates() {
        use crate::backend::recording::RecordingBackend;

        // Empty source → stops after the 2 empty rounds, well under the
        // 8-round ceiling.
        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        let seeded = backend.probe_input_devices(&mut state);
        assert_eq!(seeded, 0);
        assert_eq!(
            backend.probe_rounds_run.get(),
            2,
            "two consecutive empty rounds end the drain",
        );

        // Adversarial source that never goes empty → capped at the
        // MAX_ROUNDS ceiling, never unbounded.
        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        for _ in 0..100 {
            backend.probe_rounds.push_back(vec![probe_touchpad_info()]);
        }
        let seeded = backend.probe_input_devices(&mut state);
        assert_eq!(
            backend.probe_rounds_run.get(),
            8,
            "drain is capped at the MAX_ROUNDS ceiling",
        );
        assert_eq!(seeded, 8, "one device seeded per capped round");
    }

    #[test]
    fn shutdown_returns() {
        use crate::{
            backend::recording::RecordingBackend, core_loop::poll_tokens::ClientIdAllocator,
        };

        let (poll, sender, rx) = channel().unwrap();
        let sender_for_core = sender.clone_handle();
        let handle = std::thread::spawn(move || {
            let mut state = ServerState::new();
            let mut backend = RecordingBackend::new();
            let alloc = ClientIdAllocator::new();
            run_core(
                poll,
                rx,
                sender_for_core,
                &mut state,
                &mut backend,
                None,
                &alloc,
                AuthState::new(None),
                ResetPolicy::NoReset,
                None,
            )
        });
        sender.send(Message::Shutdown).unwrap();
        // Bound the wait so a regression that fails to return does not
        // hang the test runner.
        for _ in 0..50 {
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "run_core did not return on Shutdown");
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn evaluator_fires_idle_activation_when_deadline_elapsed() {
        let mut state = ServerState::new();
        state.screensaver.timeout_ms = 60_000;
        state.dpms.last_activity = Instant::now() - Duration::from_secs(61);
        // No client installed — emit_screen_saver_notify short-circuits
        // on empty selected_by; we're asserting state transition only.
        let mut backend = RecordingBackend::default();

        super::evaluate_screen_saver_post_poll(&mut state, &mut backend);

        assert_eq!(
            state.screensaver.active,
            ScreenSaverActive::On,
            "elapsed idle deadline must drive SS On"
        );
    }

    #[test]
    fn evaluator_fires_cycle_and_advances_next_cycle() {
        let mut state = ServerState::new();
        state.screensaver.active = ScreenSaverActive::On;
        state.screensaver.interval_ms = 60_000;
        let past = Instant::now() - Duration::from_millis(10);
        state.screensaver.next_cycle = Some(past);
        let mut backend = RecordingBackend::default();

        super::evaluate_screen_saver_post_poll(&mut state, &mut backend);

        let next = state.screensaver.next_cycle.expect("re-armed by evaluator");
        assert!(
            next > past,
            "next_cycle must advance past the prior deadline"
        );
    }

    #[test]
    fn evaluator_idle_path_skipped_while_dpms_blanked() {
        // Xorg WaitFor.c:457 — when DPMS is non-On the SS idle timer
        // is suppressed; the DPMS→SS coupling already handled it.
        let mut state = ServerState::new();
        state.screensaver.timeout_ms = 60_000;
        state.dpms.last_activity = Instant::now() - Duration::from_secs(120);
        state.dpms.power_level = 3; // Off
        let mut backend = RecordingBackend::default();

        super::evaluate_screen_saver_post_poll(&mut state, &mut backend);

        assert_eq!(
            state.screensaver.active,
            ScreenSaverActive::Off,
            "evaluator must not fire SS when DPMS is blanked"
        );
    }

    #[test]
    fn idletime_evaluator_fires_pos_transition_when_deadline_elapsed() {
        use std::time::Duration;
        use yserver_protocol::x11::{ClientId, sync as x11sync};
        let mut state = ServerState::new();
        // Pre-arm: a PositiveTransition alarm at 60_000ms, last_activity 61s ago.
        state.dpms.last_activity = std::time::Instant::now() - Duration::from_secs(61);
        let alarm_id = 0x2000;
        state.sync_alarms.insert(
            alarm_id,
            crate::server::SyncAlarm {
                owner: ClientId(1),
                counter: x11sync::IDLETIME_COUNTER,
                wait_value: 60_000,
                delta: 0,
                test_type: x11sync::TEST_POSITIVE_TRANSITION,
                events: false, // skip wire delivery; assert state mutation only
                state: x11sync::ALARM_STATE_ACTIVE,
                event_clients: Vec::new(),
                value_type: 0,
                raw_wait: 60_000,
                check_type: x11sync::TEST_POSITIVE_TRANSITION,
            },
        );
        let mut backend = RecordingBackend::default();

        super::evaluate_idletime_alarms_post_poll(&mut state, &mut backend);

        // PositiveTransition + delta=0 stays Active (Task 2 fix).
        let after = &state.sync_alarms[&alarm_id];
        assert_eq!(after.state, x11sync::ALARM_STATE_ACTIVE);
        // last_evaluated cache updated for global IDLETIME.
        assert!(
            state
                .idletime_last_evaluated
                .get(&x11sync::IDLETIME_COUNTER)
                .copied()
                .unwrap_or(0)
                >= 60_000,
            "last_evaluated cache should advance past the trigger value"
        );
    }

    #[test]
    fn idletime_evaluator_skips_when_no_idletime_alarms() {
        let mut state = ServerState::new();
        let mut backend = RecordingBackend::default();
        // No alarms at all — must not panic, must not insert spurious cache entries.
        super::evaluate_idletime_alarms_post_poll(&mut state, &mut backend);
        assert!(state.idletime_last_evaluated.is_empty());
    }

    /// Task 4/7 completion pacing: a completion whose gate targets a future
    /// vblank parks on drain (its wake is NOT signalled yet), and only fires —
    /// signalling `signal_present_wake` — once `fire_due_present_completions`
    /// runs at an MSC that has reached the target. Sibling to the NotifyMSC
    /// `parks_then_fires_on_vblank_advance` test.
    #[test]
    fn gated_present_completion_parks_then_fires_on_vblank_advance() {
        use crate::{
            backend::{CompletedPresentEvent, PresentWake, recording::RecordingBackend},
            server::PresentCompleteGate,
        };
        use yserver_protocol::x11::ClientId;

        const PRESENT_ID: u64 = 0x42;
        const TARGET_MSC: u64 = 200;
        const WINDOW_XID: u32 = 0x0000_0101;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        // A standalone sequence has advanced the general clock beyond the
        // target, but the completion-eligible clock is still zero. The gate
        // must park rather than taking the old already-due immediate path.
        state.present_crtc_clocks.insert(
            (0, 0),
            crate::server::PresentCrtcClock {
                epoch: 0,
                msc: 250,
                ust: 0,
                completion: crate::backend::PresentClockSample {
                    msc: 0,
                    ust: 0,
                    source: crate::backend::PresentClockSource::Immediate,
                },
            },
        );
        state.present_complete_gate.insert(
            PRESENT_ID,
            PresentCompleteGate {
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                effective_target_msc: TARGET_MSC,
                owner: ClientId(1),
                dst_window_xid: WINDOW_XID,
            },
        );
        // Backend reports the copy's GPU completion this iteration.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 7,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: Some(crate::backend::PresentClockSample {
                    msc: 0,
                    ust: 0,
                    source: crate::backend::PresentClockSource::Immediate,
                }),
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        // Drain: the future-target gate parks the completion — no wake yet.
        // (RecordingBackend's completion clock is (0,0), so the drain's own
        // `fire_due_present_completions` is skipped and the park holds.)
        drain_present_completions(&mut state, &mut backend);
        assert_eq!(
            state.present_pending_complete.len(),
            1,
            "future-target completion parks on drain"
        );
        assert!(
            state.present_complete_gate.is_empty(),
            "gate consumed when the copy completes"
        );
        assert!(
            backend.signalled_present_wakes.is_empty(),
            "parked completion's wake is NOT signalled before the vblank"
        );

        // Vblank advances to the target: the parked completion fires + signals.
        // The zero sample above is a fixture-only stand-in for the initial
        // copy-completion clock. Real copy completions carry `None`, allowing
        // the later selected-domain vblank sample to stamp the paced event.
        state.present_pending_complete[0].event.completion_clock = None;
        crate::core_loop::process_request::fire_due_present_completions(
            &mut state,
            &mut backend,
            crate::backend::PresentClockSample {
                msc: TARGET_MSC,
                ust: 0x1234,
                source: crate::backend::PresentClockSource::PageFlip,
            },
        );
        assert!(
            state.present_pending_complete.is_empty(),
            "parked completion released once its target MSC is reached"
        );
        assert_eq!(
            backend.signalled_present_wakes,
            vec![PRESENT_ID],
            "signal_present_wake fires exactly once at the target vblank"
        );
    }

    /// The gate-absent / already-reached path must NOT park: the completion
    /// fires immediately on drain and signals its wake once.
    #[test]
    fn ungated_present_completion_fires_immediately_without_parking() {
        use crate::backend::{CompletedPresentEvent, PresentWake, recording::RecordingBackend};
        use yserver_protocol::x11::ClientId;

        const PRESENT_ID: u64 = 0x43;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        // No gate recorded for this present_id → complete-now arm.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 8,
                host_xid: 0x0000_0202,
                dst_host_xid: 0x0000_0202,
                options: 0,
                present_id: PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        drain_present_completions(&mut state, &mut backend);
        assert!(
            state.present_pending_complete.is_empty(),
            "gate-absent completion does not park"
        );
        assert_eq!(
            backend.signalled_present_wakes,
            vec![PRESENT_ID],
            "gate-absent completion signals its wake immediately"
        );
    }

    /// Spec §"Ordered completion delivery" item 2: the due arm of the
    /// drain (a completion whose gate is already satisfied when its GPU
    /// fence retires) must route through `present_pending_complete`
    /// instead of firing inline via `complete_present_with_clock` — so
    /// that the per-window sweep in `fire_due_present_completions`, not
    /// raw arrival order, decides delivery order against anything else
    /// already parked for the same window. Pre-fix this fired here
    /// directly and never touched the queue at all.
    #[test]
    fn due_gate_arm_pushes_into_queue_instead_of_firing_inline() {
        use crate::{
            backend::{CompletedPresentEvent, PresentWake, recording::RecordingBackend},
            server::PresentCompleteGate,
        };
        use yserver_protocol::x11::ClientId;

        const PRESENT_ID: u64 = 0x44;
        const WINDOW_XID: u32 = 0x0000_0303;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        // effective_target_msc 0 is already satisfied against
        // RecordingBackend's default (0, 0) completion clock — the "due"
        // arm, not the "still future" park arm.
        state.present_complete_gate.insert(
            PRESENT_ID,
            PresentCompleteGate {
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                effective_target_msc: 0,
                owner: ClientId(1),
                dst_window_xid: WINDOW_XID,
            },
        );
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 9,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        drain_present_completions(&mut state, &mut backend);
        assert!(
            state.present_complete_gate.is_empty(),
            "gate consumed when the copy completes"
        );
        assert_eq!(
            state.present_pending_complete.len(),
            1,
            "the due arm pushes into the ordered queue rather than firing \
             inline (RecordingBackend's zero completion clock means the \
             same-pass sweep can't drain it yet, which is fine — this test \
             only pins that it did NOT fire inline)"
        );
        assert!(
            backend.signalled_present_wakes.is_empty(),
            "must not signal the wake inline — delivery is the sweep's job"
        );
    }

    /// Spec round-4 F6: presents without a usable clock
    /// (`effective_target_msc == None`, no gate entry — the drain's
    /// gate-absent arm) sit outside the
    /// per-window hold-back entirely and complete immediately, even ahead
    /// of an earlier-arrived, still-unresolved synced present parked for
    /// the same window. This is Xorg-parity and pre-existing; documented
    /// so it isn't mistaken for a hold-back bug.
    #[test]
    fn no_clock_present_completion_bypasses_per_window_hold_back() {
        use crate::{
            backend::{CompletedPresentEvent, PresentWake, recording::RecordingBackend},
            server::PendingPresentComplete,
        };
        use yserver_protocol::x11::{ClientId, present as x11present};

        const WINDOW_XID: u32 = 0x0000_0606;
        const PARKED_SMALLER_ID: u64 = 5;
        const ASYNC_ID: u64 = 6;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        // An earlier, smaller-id synced present is still parked/unresolved
        // for this window.
        state.present_pending_complete.push(PendingPresentComplete {
            event: CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 1,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: PARKED_SMALLER_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            },
            effective_target_msc: 0,
            mode: x11present::COMPLETE_MODE_COPY,
            emit_idle: true,
        });

        // A later async completion for the same window: no gate entry at all.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 2,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: ASYNC_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        drain_present_completions(&mut state, &mut backend);
        assert_eq!(
            backend.signalled_present_wakes,
            vec![ASYNC_ID],
            "the async completion fires immediately, bypassing hold-back"
        );
        assert_eq!(
            state.present_pending_complete.len(),
            1,
            "the earlier parked synced present is untouched by the async path"
        );
    }

    /// Review fix (post-Task-6): the async exemption above covers async
    /// firing ahead of a still-HELD entry — it must NOT cover an async
    /// completion overtaking a gated Copy that is already due and simply
    /// hasn't been swept yet. In one `drain_present_completions` pass,
    /// `completed = [X(gated, due, id=5), Y(async, id=7)]` for the SAME
    /// window: X's due-arm pushes into the queue (per Task 6 Step 3), then
    /// Y's gate-absent arm used to fire straight through, landing before
    /// X's post-loop sweep — id=7 then id=5, a backward serial that
    /// didn't exist pre-Task-6 (eager firing kept them in arrival order).
    /// Fixed by flushing due-and-unblocked entries from the queue before
    /// the async arm fires inline, so id=5 goes out first.
    #[test]
    fn gated_due_copy_delivers_before_same_drain_async_completion() {
        use crate::{
            backend::{CompletedPresentEvent, PresentWake, recording::RecordingBackend},
            server::PresentCompleteGate,
        };
        use yserver_protocol::x11::ClientId;

        const WINDOW_XID: u32 = 0x0000_0808;
        const GATED_ID: u64 = 5;
        const ASYNC_ID: u64 = 7;
        const TARGET_MSC: u64 = 300;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        // A real, nonzero completion clock this time (RecordingBackend
        // defaults to (0,0), which would make fire_due_present_completions
        // bail before ever reaching the ordering bug this test pins).
        backend.present_ust_msc = (TARGET_MSC, 0xABCD);

        state.present_complete_gate.insert(
            GATED_ID,
            PresentCompleteGate {
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                effective_target_msc: TARGET_MSC,
                owner: ClientId(1),
                dst_window_xid: WINDOW_XID,
            },
        );
        // Arrival order within one drain: the gated-due entry first, the
        // async one second — matching the reviewer's vector.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 1,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: GATED_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: Some(crate::backend::PresentClockSample {
                    msc: TARGET_MSC,
                    ust: 0xABCD,
                    source: crate::backend::PresentClockSource::PageFlip,
                }),
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 2,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: ASYNC_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        drain_present_completions(&mut state, &mut backend);
        assert_eq!(
            backend.signalled_present_wakes,
            vec![GATED_ID, ASYNC_ID],
            "Copy(5) must deliver before async(7) in the same drain pass — \
             pre-fix this reads [7, 5]"
        );
    }

    /// Task 4 (spec "Loop-order and clock contract" item 1): the tail's
    /// drain must run BEFORE `maybe_composite`, so a present executed in
    /// this iteration's drain is visible to this iteration's compose
    /// instead of slipping a full period behind unrelated damage. Drives
    /// both halves of the drain — a source-ready `PresentPixmap` copy
    /// (whose execution marks dirty, `process_request.rs:8714`) and a
    /// canned GPU-completion event (`drain_completed_present_events`) —
    /// and asserts both are recorded before `maybe_composite` in
    /// `RecordingBackend`'s call log. Fails against the pre-Task-4 order
    /// (`maybe_composite` before the drain).
    #[test]
    fn run_iteration_tail_drains_present_work_before_compositing() {
        use crate::{
            backend::{
                CompletedPresentEvent, PresentWake,
                recording::{RecordedCall, RecordingBackend},
            },
            server::{PendingPresentEntry, PendingPresentPixmap, PendingPresentRequest},
        };
        use yserver_protocol::x11::{ClientId, present::PixmapRequest};

        const WAIT_ID: u64 = 7;
        const DEFERRED_PRESENT_ID: u64 = 0x77;
        const PRESENT_ID: u64 = 0x99;
        const WINDOW_XID: u32 = 0x0000_0303;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        // A source-ready PresentPixmap copy: draining it runs
        // `execute_present_pixmap_copy` then `mark_dirty` — the real
        // production link between "the drain executed something" and
        // "compose must see it this iteration".
        state
            .present_wait_to_id
            .insert(WAIT_ID, DEFERRED_PRESENT_ID);
        state.present_pending_exec.insert(
            DEFERRED_PRESENT_ID,
            PendingPresentEntry {
                pending: PendingPresentPixmap {
                    origin: None,
                    client_id: ClientId(1),
                    request: PendingPresentRequest::Pixmap(PixmapRequest {
                        window: WINDOW_XID,
                        pixmap: 0x304,
                        serial: 9,
                        valid: 0,
                        update: 0,
                        x_off: 0,
                        y_off: 0,
                        target_crtc: 0,
                        wait_fence: 0,
                        idle_fence: 0,
                        options: 0,
                        target_msc: 0,
                        divisor: 0,
                        remainder: 0,
                        notifies: Vec::new(),
                    }),
                    wake: crate::backend::PresentWake::Pixmap { idle_fence_xid: 0 },
                    masked_options: 0,
                    src_host_xid: 0x0040_0304,
                    paint_dst_host_xid: 0x0040_0303,
                    completion_dst_host_xid: 0x0040_0303,
                    src_width: 10,
                    src_height: 10,
                    update_rects: None,
                    present_id: DEFERRED_PRESENT_ID,
                    window_generation: 0,
                    crtc_id: 0,
                    crtc_epoch: 0,
                    msc_offset: 0,
                    effective_target_msc: None,
                },
                source_ready: false,
                wait_id: Some(WAIT_ID),
                pin: None,
            },
        );
        backend.ready_present_source_waits.push(WAIT_ID);

        // A canned GPU-completion event: exercises the second drain half.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 9,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        run_iteration_tail(&mut state, &mut backend);

        let calls = backend.calls();
        let mark_dirty_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::MarkDirty))
            .expect("source-ready copy executed and marked dirty");
        let drain_completed_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::DrainCompletedPresentEvents))
            .expect("completed present events drained");
        let composite_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::MaybeComposite))
            .expect("maybe_composite invoked");

        assert!(
            mark_dirty_idx < composite_idx,
            "drain's mark_dirty ({mark_dirty_idx}) must precede maybe_composite ({composite_idx})"
        );
        assert!(
            drain_completed_idx < composite_idx,
            "drain_completed_present_events ({drain_completed_idx}) must precede maybe_composite ({composite_idx})"
        );
    }

    /// Fix-forward: idle-vblank arming for a parked Present completion must
    /// run AFTER `maybe_composite`, not inside the pre-compose drain.
    /// `mark_dirty()` alone (no output damage) makes a real KMS compose
    /// return `Skipped(EmptyDamage)`, which still clears
    /// `scene_wants_compose()` — so `present_completion_is_idle()` only
    /// reports idle post-compose. Arming pre-compose would see a dirty
    /// scene and arm nothing, stranding the parked `CompleteNotify` with no
    /// fd left to wake `poll`. Fails against the arm folded into
    /// `drain_present_completions` (landing before `MaybeComposite`).
    #[test]
    fn run_iteration_tail_arms_present_completion_idle_vblanks_after_compositing() {
        use crate::{
            backend::{
                CompletedPresentEvent, PresentWake,
                recording::{RecordedCall, RecordingBackend},
            },
            server::PendingPresentComplete,
        };
        use yserver_protocol::x11::ClientId;

        const PARKED_PRESENT_ID: u64 = 0x55;
        const DRAINED_PRESENT_ID: u64 = 0x56;
        const WINDOW_XID: u32 = 0x0000_0505;

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();

        // Something for the arm to arm: a completion already parked on a
        // future target MSC.
        state.present_pending_complete.push(PendingPresentComplete {
            event: CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 10,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: PARKED_PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            },
            effective_target_msc: 500,
            mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
            emit_idle: true,
        });

        // A canned GPU-completion event so the drain half also runs.
        backend
            .completed_present_events_to_drain
            .push(CompletedPresentEvent {
                client_id: ClientId(1),
                serial: 11,
                host_xid: WINDOW_XID,
                dst_host_xid: WINDOW_XID,
                options: 0,
                present_id: DRAINED_PRESENT_ID,
                window_generation: 0,
                crtc_id: 0,
                crtc_epoch: 0,
                msc_offset: 0,
                completion_clock: None,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: yserver_protocol::x11::present::COMPLETE_MODE_COPY,
                emit_idle: true,
            });

        run_iteration_tail(&mut state, &mut backend);

        let calls = backend.calls();
        let drain_completed_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::DrainCompletedPresentEvents))
            .expect("completed present events drained");
        let composite_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::MaybeComposite))
            .expect("maybe_composite invoked");
        let arm_idx = calls
            .iter()
            .position(|c| matches!(c, RecordedCall::ArmPresentCompletionIdleVblanks))
            .expect("parked completion armed an idle vblank");

        assert!(
            drain_completed_idx < composite_idx,
            "drain ({drain_completed_idx}) must still precede compose ({composite_idx})"
        );
        assert!(
            composite_idx < arm_idx,
            "arm ({arm_idx}) must run after compose ({composite_idx}), not inside the pre-compose drain"
        );
    }

    #[test]
    fn run_iteration_tail_flushes_damage_before_compositing() {
        use crate::backend::recording::{RecordedCall, RecordingBackend};

        let mut state = ServerState::new();
        let mut backend = RecordingBackend::new();
        state.damage_notify_flush_pending = true;

        run_iteration_tail(&mut state, &mut backend);

        let calls = backend.calls();
        let flush = calls
            .iter()
            .position(|call| matches!(call, RecordedCall::FlushBeforeDamageNotify))
            .expect("damage boundary flushed");
        let compose = calls
            .iter()
            .position(|call| matches!(call, RecordedCall::MaybeComposite))
            .expect("compose attempted");
        assert!(flush < compose);
        assert!(!state.damage_notify_flush_pending);
    }
}

/// The armed reset trigger, driven through a live `run_core` rather than
/// against `ResetTrigger` directly (that state machine is unit-tested in
/// `core_loop::reset`). What these pin is the *wiring*: which loop events
/// arm it, which fire it, and what the boundary does afterwards.
///
/// A reset is observed through the generation counter — the boundary
/// bumps it, and nothing else in the loop does — read from the clone the
/// test keeps before handing `CoreReceiver` to the loop.
#[cfg(test)]
mod server_reset {
    use std::{
        io::{Read, Write},
        os::unix::net::{UnixListener, UnixStream},
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    use super::{Listener, ResetPolicy, ServerState, run_core};
    use crate::{
        backend::recording::RecordingBackend,
        core_loop::{
            Generation, GenerationCounter, Message,
            auth::AuthState,
            poll_tokens::ClientIdAllocator,
            sender::{CoreSender, channel},
        },
        transport::Transport,
    };
    use yserver_protocol::x11::{ClientByteOrder, ClientId, RequestHeader, SequenceNumber};

    /// Generous: these wait on real threads (setup, reader, core) under a
    /// loaded test binary, and every use is a wait-for-success.
    const TIMEOUT: Duration = Duration::from_secs(10);
    /// How long a "must NOT happen" assertion watches for. The positive
    /// cases below complete in single-digit milliseconds.
    const QUIET: Duration = Duration::from_millis(400);

    /// The value a `GenerationCounter` holds after `n` resets.
    fn generation_after(n: u64) -> Generation {
        let counter = GenerationCounter::new();
        for _ in 0..n {
            counter.bump();
        }
        counter.current()
    }

    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if cond() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for {what}");
    }

    /// A `run_core` running on its own thread over a private unix socket.
    struct Server {
        path: PathBuf,
        sender: CoreSender,
        generations: GenerationCounter,
        /// The loop's own id allocator, shared so a test can learn the
        /// `ClientId` the next connection will be given. Monotonic, so a
        /// peek before `establish()` names that client exactly.
        client_ids: std::sync::Arc<ClientIdAllocator>,
        handle: Option<JoinHandle<std::io::Result<()>>>,
    }

    impl Server {
        fn start(policy: ResetPolicy, auth: std::sync::Arc<AuthState>) -> Self {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "yserver-reset-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("bind");
            let (poll, sender, rx) = channel().expect("channel");
            let generations = rx.generation_counter();
            let sender_for_core = sender.clone_handle();
            let client_ids = std::sync::Arc::new(ClientIdAllocator::new());
            let client_ids_for_core = client_ids.clone();
            let handle = std::thread::spawn(move || {
                let mut state = ServerState::new();
                let mut backend = RecordingBackend::new();
                run_core(
                    poll,
                    rx,
                    sender_for_core,
                    &mut state,
                    &mut backend,
                    [Listener::Unix(listener)],
                    &client_ids_for_core,
                    auth,
                    policy,
                    None,
                )
            });
            Self {
                path,
                sender,
                generations,
                client_ids,
                handle: Some(handle),
            }
        }

        /// The id the next accepted connection will get.
        fn next_client_id(&self) -> ClientId {
            self.client_ids.peek()
        }

        fn with_policy(policy: ResetPolicy) -> Self {
            Self::start(policy, AuthState::new(None))
        }

        fn generation(&self) -> Generation {
            self.generations.current()
        }

        fn connect(&self) -> UnixStream {
            let peer = UnixStream::connect(&self.path).expect("connect");
            peer.set_read_timeout(Some(TIMEOUT)).expect("read timeout");
            peer
        }

        /// Connect and take the connection all the way to *established*:
        /// setup handshake, then a `GetInputFocus` round-trip. The reply
        /// is what proves the core reached `handle_client_setup_complete`
        /// — the setup reply alone is written by the setup thread and
        /// says nothing about `state.clients`.
        fn establish(&self) -> UnixStream {
            let mut peer = self.connect();
            peer.write_all(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                .expect("setup request");
            let mut header = [0_u8; 8];
            peer.read_exact(&mut header).expect("setup reply header");
            assert_eq!(header[0], 1, "setup must succeed");
            let len = usize::from(u16::from_le_bytes([header[6], header[7]])) * 4;
            peer.read_exact(&mut vec![0; len])
                .expect("setup reply body");
            round_trip(&mut peer);
            peer
        }

        fn shutdown(mut self) -> std::io::Result<()> {
            let _ = self.sender.send(Message::Shutdown);
            let result = self.handle.take().expect("handle").join().expect("join");
            let _ = std::fs::remove_file(&self.path);
            result
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = self.sender.send(Message::Shutdown);
                let _ = handle.join();
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// `GetInputFocus` — the classic X11 round-trip probe. Always replies,
    /// changes nothing.
    fn round_trip(peer: &mut UnixStream) {
        peer.write_all(&[43, 0, 1, 0]).expect("GetInputFocus");
        let mut reply = [0_u8; 32];
        peer.read_exact(&mut reply).expect("GetInputFocus reply");
        assert_eq!(reply[0], 1, "reply, not an error");
    }

    /// `SetCloseDownMode(RetainPermanent)` — opcode 112, mode in the data
    /// byte. Followed by a round-trip so the request is known to have been
    /// processed before the caller drops the socket.
    fn set_retain_permanent(peer: &mut UnixStream) {
        peer.write_all(&[112, 1, 1, 0]).expect("SetCloseDownMode");
        round_trip(peer);
    }

    // -- the trigger table: last client leaves x each policy ----------

    #[test]
    fn the_last_client_leaving_resets_under_reset() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let peer = server.establish();
        assert_eq!(server.generation(), generation_after(0));
        drop(peer);
        wait_until("the drained session to reset", || {
            server.generation() == generation_after(1)
        });
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn the_last_client_leaving_does_nothing_under_noreset() {
        let server = Server::with_policy(ResetPolicy::NoReset);
        let peer = server.establish();
        drop(peer);
        std::thread::sleep(QUIET);
        // Non-vacuous: the server is still serving, so the loop did run
        // through the disconnect — it simply did not reset.
        let survivor = server.establish();
        assert_eq!(
            server.generation(),
            generation_after(0),
            "-noreset must never cross the boundary"
        );
        drop(survivor);
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn the_last_client_leaving_terminates_under_terminate() {
        let mut server = Server::with_policy(ResetPolicy::Terminate);
        let peer = server.establish();
        drop(peer);
        let handle = server.handle.take().expect("handle");
        wait_until("run_core to return", || handle.is_finished());
        handle
            .join()
            .expect("join")
            .expect("-terminate must shut down cleanly, not error");
        assert_eq!(
            server.generation(),
            generation_after(0),
            "-terminate exits instead of resetting"
        );
    }

    #[test]
    fn a_retain_permanent_client_does_not_inhibit_the_reset() {
        // `process_disconnect` keeps a retained client's resources as a
        // zombie but removes it from `state.clients` regardless, so the
        // session still counts as drained. Xorg's `really_close_down`
        // gate does the opposite; the forced teardown at the boundary is
        // what makes ours safe.
        let server = Server::with_policy(ResetPolicy::Reset);
        let mut peer = server.establish();
        set_retain_permanent(&mut peer);
        drop(peer);
        wait_until("a retained client's departure to reset", || {
            server.generation() == generation_after(1)
        });
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn a_client_that_leaves_while_another_stays_does_not_reset() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let first = server.establish();
        let mut second = server.establish();
        drop(first);
        std::thread::sleep(QUIET);
        round_trip(&mut second);
        assert_eq!(
            server.generation(),
            generation_after(0),
            "the session is not drained while a client remains"
        );
        drop(second);
        wait_until("the second departure to reset", || {
            server.generation() == generation_after(1)
        });
        server.shutdown().expect("clean shutdown");
    }

    // -- SIGHUP, per policy -------------------------------------------

    #[test]
    fn sighup_resets_a_running_session_under_reset() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let _peer = server.establish();
        server
            .sender
            .send(Message::ResetRequested)
            .expect("send SIGHUP request");
        wait_until("SIGHUP to reset", || {
            server.generation() == generation_after(1)
        });
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn sighup_resets_rather_than_terminating_under_terminate() {
        let server = Server::with_policy(ResetPolicy::Terminate);
        let _peer = server.establish();
        server
            .sender
            .send(Message::ResetRequested)
            .expect("send SIGHUP request");
        wait_until("SIGHUP to reset", || {
            server.generation() == generation_after(1)
        });
        assert!(
            !server.handle.as_ref().expect("handle").is_finished(),
            "SIGHUP raises DE_RESET, not DE_TERMINATE"
        );
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn sighup_does_not_reset_under_noreset() {
        // Under the default policy the signal thread never produces this
        // message (it keeps sending `Shutdown`); the loop refuses it
        // anyway, so `-noreset` behaviour cannot drift.
        let server = Server::with_policy(ResetPolicy::NoReset);
        let mut peer = server.establish();
        server
            .sender
            .send(Message::ResetRequested)
            .expect("send SIGHUP request");
        std::thread::sleep(QUIET);
        round_trip(&mut peer);
        assert_eq!(server.generation(), generation_after(0));
        drop(peer);
        server.shutdown().expect("clean shutdown");
    }

    // -- the arming cases a happy-path suite misses --------------------

    #[test]
    fn an_idle_reset_server_never_resets() {
        // No client has ever connected, so the client set is empty from
        // the first iteration. A state check would reset here, over and
        // over.
        let server = Server::with_policy(ResetPolicy::Reset);
        std::thread::sleep(QUIET);
        assert_eq!(server.generation(), generation_after(0));
        // Still serving: the idle loop was running, not wedged.
        let peer = server.establish();
        assert_eq!(server.generation(), generation_after(0));
        drop(peer);
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn a_connection_dropped_before_setup_completes_arms_nothing() {
        // Accepted, a setup thread spawned, then gone before any
        // `ClientSetupComplete`. Nothing was ever established, so
        // nothing may fire.
        let server = Server::with_policy(ResetPolicy::Reset);
        for _ in 0..5 {
            let peer = server.connect();
            drop(peer);
        }
        std::thread::sleep(QUIET);
        assert_eq!(
            server.generation(),
            generation_after(0),
            "accept is not arming"
        );
        let peer = server.establish();
        assert_eq!(server.generation(), generation_after(0));
        drop(peer);
        server.shutdown().expect("clean shutdown");
    }

    #[test]
    fn a_client_refused_for_a_bad_cookie_arms_nothing() {
        // The case a stranger can reach: stage 1's TCP listener binds
        // 0.0.0.0, so a wrong cookie must not be able to erase a
        // session. Refused inside the setup thread — the core never
        // hears about it.
        let cookie = [0x5A_u8; 16];
        let auth_path = write_xauth(&cookie);
        let server = Server::start(ResetPolicy::Reset, AuthState::new(Some(auth_path.clone())));

        let mut peer = server.connect();
        write_setup_with_cookie(&mut peer, &[0x22_u8; 16]);
        let mut header = [0_u8; 8];
        peer.read_exact(&mut header).expect("setup reply header");
        assert_eq!(header[0], 0, "a bad cookie must be refused");
        drop(peer);

        std::thread::sleep(QUIET);
        assert_eq!(
            server.generation(),
            generation_after(0),
            "a refused connection is not arming"
        );

        // And the good cookie still works, so the refusal was the
        // server's decision, not a broken fixture.
        let mut good = server.connect();
        write_setup_with_cookie(&mut good, &cookie);
        good.read_exact(&mut header).expect("setup reply header");
        assert_eq!(header[0], 1, "the matching cookie must be accepted");
        let len = usize::from(u16::from_le_bytes([header[6], header[7]])) * 4;
        good.read_exact(&mut vec![0; len])
            .expect("setup reply body");
        round_trip(&mut good);
        assert_eq!(server.generation(), generation_after(0));

        drop(good);
        wait_until("the authorized client's departure to reset", || {
            server.generation() == generation_after(1)
        });
        server.shutdown().expect("clean shutdown");
        let _ = std::fs::remove_file(&auth_path);
    }

    #[test]
    fn a_reset_disarms_the_new_generation() {
        // The boundary leaves an empty client set behind, and the dead
        // generation's reader thread posts its `ClientDisconnected`
        // afterwards. Neither may produce a second reset.
        let server = Server::with_policy(ResetPolicy::Reset);
        let peer = server.establish();
        drop(peer);
        wait_until("the first reset", || {
            server.generation() == generation_after(1)
        });
        std::thread::sleep(QUIET);
        assert_eq!(
            server.generation(),
            generation_after(1),
            "exactly one reset; the fresh generation starts disarmed"
        );
        // The new generation serves, and arms again on its own client.
        let peer = server.establish();
        drop(peer);
        wait_until("the second generation to reset in turn", || {
            server.generation() == generation_after(2)
        });
        server.shutdown().expect("clean shutdown");
    }

    // -- the generation quarantine: producers bound at creation time ---

    /// Watch a socket for `QUIET` and report whether the server wrote
    /// **no bytes** to it. Restores the long timeout, so the caller can
    /// keep using the socket afterwards.
    ///
    /// "The far end went away without writing" counts as quiet, and has
    /// three shapes here: a timeout (nobody holds the other half open),
    /// `Ok(0)`, and — when the other half is dropped while bytes we sent
    /// are still unread in its queue, which is exactly what discarding a
    /// message carrying a `Transport` does — `ECONNRESET`. What must not
    /// happen is bytes arriving; a caller that also cares whether the
    /// peer is still *alive* follows this with a `round_trip`.
    fn stays_quiet(peer: &mut UnixStream) -> bool {
        peer.set_read_timeout(Some(QUIET)).expect("read timeout");
        let mut byte = [0_u8; 1];
        let quiet = match peer.read(&mut byte) {
            Ok(0) => true,
            Ok(_) => false,
            Err(err) => matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
            ),
        };
        peer.set_read_timeout(Some(TIMEOUT)).expect("read timeout");
        quiet
    }

    /// The hole the quarantine exists to close: a producer belonging to
    /// the destroyed session must not be able to hand the new one a
    /// client.
    ///
    /// `reset_generation` shuts the setup sockets down, which narrows the
    /// window but does not close it — a setup thread can already hold a
    /// fully decoded `ClientSetupComplete` and be one instruction away
    /// from sending it. The producer here stands in for that thread: it
    /// takes its handle while generation 0 runs, exactly where
    /// `accept_pending` hands one to `setup_thread::spawn`, and sends
    /// after the boundary. Tagging at *send* time would stamp it with the
    /// new generation and let it through.
    #[test]
    fn an_old_setup_completion_cannot_create_a_client_in_the_new_generation() {
        let server = Server::with_policy(ResetPolicy::Reset);
        // The id the pre-reset client holds — what a real old setup
        // thread's completion would carry.
        let doomed = server.next_client_id();
        let stale = server.sender.bind();
        let peer = server.establish();
        assert_eq!(server.generation(), generation_after(0));
        drop(peer);
        wait_until("the drained session to reset", || {
            server.generation() == generation_after(1)
        });

        let (core_side, mut phantom) = UnixStream::pair().expect("socketpair");
        stale
            .send(Message::ClientSetupComplete {
                id: doomed,
                generation: stale.generation(),
                stream: Transport::Unix(core_side),
                resource_id_base: 0x0020_0000,
                resource_id_mask: 0x000F_FFFF,
                byte_order: ClientByteOrder::LittleEndian,
                is_local: true,
                fd_passing: true,
                setup_reply: Vec::new(),
            })
            .expect("send the stale completion");

        // Accepted, this would insert a `ClientState` and spawn a reader,
        // and the request below would come back answered.
        phantom.write_all(&[43, 0, 1, 0]).expect("GetInputFocus");
        assert!(
            stays_quiet(&mut phantom),
            "a client authorized in the destroyed session must not be served by the new one"
        );
        // Nor may it arm the fresh generation: an accepted completion
        // calls `note_client_established`, and the phantom's own
        // departure would then reset a session it was never part of.
        drop(phantom);
        std::thread::sleep(QUIET);
        assert_eq!(
            server.generation(),
            generation_after(1),
            "the phantom must not arm — and then drain — the new generation"
        );
        server.shutdown().expect("clean shutdown");
    }

    /// The reader-thread half: a `Request` produced by a retired reader
    /// must not execute against the new session.
    ///
    /// It names a client of the *new* session deliberately. The loop
    /// already drops a request whose client is unknown
    /// (`process_request_inline`'s post-disconnect guard), so an old id
    /// would pass whether or not the generation filter works, and the
    /// test would prove nothing. The reply landing on a live client's
    /// socket is the sharpest observable there is.
    #[test]
    fn an_old_generation_request_is_not_executed_in_the_new_session() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let stale = server.sender.bind();
        let peer = server.establish();
        drop(peer);
        wait_until("the drained session to reset", || {
            server.generation() == generation_after(1)
        });

        let live_id = server.next_client_id();
        let mut live = server.establish();

        stale
            .send(Message::Request {
                id: live_id,
                sequence: SequenceNumber(0x4242),
                accepted_at: None,
                header: RequestHeader {
                    opcode: 43, // GetInputFocus — always replies
                    data: 0,
                    length_units: 1,
                },
                body: Vec::new(),
                attached_fd: None,
            })
            .expect("send the stale request");

        assert!(
            stays_quiet(&mut live),
            "a request tagged by a retired producer must not be executed"
        );
        // Quiet because the request was discarded, not because the
        // client is broken.
        round_trip(&mut live);
        assert_eq!(server.generation(), generation_after(1));
        drop(live);
        server.shutdown().expect("clean shutdown");
    }

    /// The other message a retired reader can still emit. Accepted, it
    /// runs `disconnect_with_pending_cleanup` against the new session —
    /// which under `-reset` drains it and resets a generation that was
    /// serving a live client.
    #[test]
    fn an_old_generation_disconnect_cannot_tear_down_a_new_session_client() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let stale = server.sender.bind();
        let peer = server.establish();
        drop(peer);
        wait_until("the drained session to reset", || {
            server.generation() == generation_after(1)
        });

        let live_id = server.next_client_id();
        let mut live = server.establish();

        stale
            .send(Message::ClientDisconnected {
                id: live_id,
                reason: std::io::Error::other("retired reader"),
            })
            .expect("send the stale disconnect");

        std::thread::sleep(QUIET);
        assert_eq!(
            server.generation(),
            generation_after(1),
            "a retired producer must not be able to drain the new session"
        );
        round_trip(&mut live);

        // The real departure still works, so the filter did not wedge
        // the trigger.
        drop(live);
        wait_until("the live client's own departure to reset", || {
            server.generation() == generation_after(2)
        });
        server.shutdown().expect("clean shutdown");
    }

    /// The other half of binding at accept: a connection accepted *after*
    /// the boundary gets a producer bound to the new generation, so
    /// nothing it sends is stale. Both its setup thread and its reader
    /// thread have to pass the filter for `establish` (which ends in a
    /// `GetInputFocus` round-trip) to return at all.
    #[test]
    fn a_connection_accepted_after_a_reset_is_served_normally() {
        let server = Server::with_policy(ResetPolicy::Reset);
        let peer = server.establish();
        drop(peer);
        wait_until("the drained session to reset", || {
            server.generation() == generation_after(1)
        });

        let mut fresh = server.establish();
        for _ in 0..3 {
            round_trip(&mut fresh);
        }
        assert_eq!(server.generation(), generation_after(1));
        drop(fresh);
        wait_until("the second generation to reset in turn", || {
            server.generation() == generation_after(2)
        });
        server.shutdown().expect("clean shutdown");
    }

    // -- fixtures for the auth case ------------------------------------

    const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";

    fn write_xauth(cookie: &[u8]) -> PathBuf {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&256_u16.to_be_bytes()); // FamilyLocal
        for field in [
            b"host".as_slice(),
            b"0",
            MIT_MAGIC_COOKIE.as_bytes(),
            cookie,
        ] {
            bytes.extend_from_slice(
                &u16::try_from(field.len())
                    .expect("field fits")
                    .to_be_bytes(),
            );
            bytes.extend_from_slice(field);
        }
        let path = std::env::temp_dir().join(format!("yserver-reset-auth-{}", std::process::id()));
        std::fs::write(&path, bytes).expect("write xauthority");
        path
    }

    fn write_setup_with_cookie(peer: &mut UnixStream, cookie: &[u8]) {
        let name = MIT_MAGIC_COOKIE.as_bytes();
        let mut buf = Vec::new();
        buf.push(b'l');
        buf.push(0);
        buf.extend_from_slice(&11_u16.to_le_bytes());
        buf.extend_from_slice(&0_u16.to_le_bytes());
        buf.extend_from_slice(&u16::try_from(name.len()).expect("name fits").to_le_bytes());
        buf.extend_from_slice(
            &u16::try_from(cookie.len())
                .expect("cookie fits")
                .to_le_bytes(),
        );
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(name);
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        buf.extend_from_slice(cookie);
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        peer.write_all(&buf).expect("setup request with cookie");
    }
}
