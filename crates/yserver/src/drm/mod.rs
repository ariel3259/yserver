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
