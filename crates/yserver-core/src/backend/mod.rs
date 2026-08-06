//! Backend abstraction. Currently `HostX11Backend` is the sole impl;
//! Phase 6.3+ will add a KMS backend.

pub mod gamma;
pub mod handles;
pub mod params;
mod trait_def;

#[cfg(test)]
pub mod recording;

pub use gamma::{identity_ramp, resample_channel};
pub use handles::{
    AnyHandle, ColormapHandle, CursorHandle, FontHandle, GlyphSetHandle, HandleKind, PictureHandle,
    PixmapHandle, VisualHandle, WindowHandle,
};
pub use params::{
    ArcMode, BgState, CapStyle, ClipState, DrawState, FillRule, FillState, FillStyle, GcFunction,
    JoinStyle, LineStyle, SubwindowMode,
};
pub use trait_def::{
    ActiveCursorImage, Backend, BackendFdKind, CompletedPresentEvent, Dri3Caps, Dri3PixmapExport,
    HostSocketStatus, KeymapLoad, ModeSpec, PresentCaps, PresentClockSample, PresentClockSource,
    PresentSourceWait, PresentWake, SyncobjHandle, XkbNewKeyboardInfo, XshmfenceHandle,
};

use yserver_protocol::x11::ClientId;

pub(crate) use crate::server::BackendCapabilities;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OriginContext {
    pub client_id: ClientId,
    pub nested_seq: u16,
    pub opcode: u8,
}

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
