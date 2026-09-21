pub mod buffer;
pub mod device;
pub mod event_stream;
pub mod modeset;
pub mod page_flip;
pub mod swapchain;

pub use buffer::Buffer;
pub use device::Device;
pub use event_stream::{DrainStop, DrmEventRecord};
pub use swapchain::Swapchain;

/// B-10/R11: the typed refusal every legacy-write DRM sink returns when
/// `legacy_write_permitted` is `false` -- checked immediately before the
/// ioctl that would otherwise mutate the device, so a refusal here leaves
/// hardware state exactly as if the sink had not been called. Every
/// production caller computes `legacy_write_permitted` from
/// `PlatformBackend::allows_legacy`, which answers `true` when no
/// `resources::transport::TransportGate` is installed for the device (the
/// case for every production device today, R8): this is unreachable in
/// production and inert there. Tests install a gate and drive it to
/// `Quiescing`/`Owner`/`Closed` to reach it.
///
/// Deliberately `ErrorKind::Other`, not `PermissionDenied`: several real
/// DRM failure paths (`cursor_plane_hide_all`'s "no master" branch among
/// them) already branch on `PermissionDenied` for a genuine EACCES from the
/// kernel, and this refusal never reaches the kernel at all -- reusing that
/// kind would silently misfile it under "no master" logging instead of
/// surfacing as the distinct thing it is. `raw_os_error()` is `None` here,
/// which alone already distinguishes it from any real ioctl failure.
pub(crate) fn transport_gate_refusal(op: &str) -> std::io::Error {
    std::io::Error::other(format!("transport gate: legacy {op} write refused"))
}

// Test-only scaffolding for the C.0 structural-debt inventory: record entry
// into a converted legacy primary/unflip sink before its permit check. The
// recorder observes sink entry independently of whether the transport gate
// later refuses the write, and is deleted with the legacy branches.
#[cfg(test)]
thread_local! {
    static LEGACY_SINK_ENTRIES: std::cell::RefCell<Vec<(
        String,
        crate::kms::render::resources::WriterClass,
    )>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn record_legacy_sink_entry_for_tests(
    device: &Device,
    class: crate::kms::render::resources::WriterClass,
) {
    LEGACY_SINK_ENTRIES.with(|entries| {
        entries
            .borrow_mut()
            .push((device.path().to_string(), class));
    });
}

#[cfg(test)]
pub(crate) fn clear_legacy_sink_entries_for_tests() {
    LEGACY_SINK_ENTRIES.with(|entries| entries.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn legacy_sink_entries_for_tests()
-> Vec<(String, crate::kms::render::resources::WriterClass)> {
    LEGACY_SINK_ENTRIES.with(|entries| entries.borrow().clone())
}
