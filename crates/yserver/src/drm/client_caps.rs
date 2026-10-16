use std::io;

use std::os::fd::{AsFd as _, AsRawFd as _};
#[cfg(test)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use crate::drm::Device;

/// Client capability requests needed to discover atomic cursor planes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeviceClientCapability {
    UniversalPlanes,
    Atomic,
    CursorPlaneHotspot,
}

/// Cached device facts that affect atomic cursor-plane eligibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorDeviceCaps {
    hotspot_cap_accepted: bool,
    driver_name: String,
}

impl CursorDeviceCaps {
    pub(crate) fn new(hotspot_cap_accepted: bool, driver_name: String) -> Self {
        Self {
            hotspot_cap_accepted,
            driver_name,
        }
    }

    /// Whether this fd accepted `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`.
    pub const fn cursor_plane_hotspot_cap_accepted(&self) -> bool {
        self.hotspot_cap_accepted
    }

    /// Kernel DRM driver name, as returned by the driver's version ioctl.
    pub fn driver_name(&self) -> &str {
        &self.driver_name
    }

    /// Whether this driver needs hotspot support to expose a virtual cursor plane.
    pub fn is_virtualized_cursor_driver(&self) -> bool {
        crate::kms::cursor_plane::topology::is_virtualized_cursor_driver(self.driver_name())
    }
}

/// Operations performed while a primary DRM fd is initialized.
pub(crate) trait DeviceControl: Send + Sync {
    fn set_client_capability(
        &self,
        device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()>;

    fn driver_name(&self, device: &Device) -> io::Result<String>;
}

/// The production implementation forwards client caps and the driver query to DRM.
pub(crate) struct DrmDeviceControl;

impl DeviceControl for DrmDeviceControl {
    fn set_client_capability(
        &self,
        device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()> {
        match capability {
            DeviceClientCapability::UniversalPlanes => {
                use ::drm::Device as _;
                device.set_client_capability(::drm::ClientCapability::UniversalPlanes, true)
            }
            DeviceClientCapability::Atomic => {
                use ::drm::Device as _;
                device.set_client_capability(::drm::ClientCapability::Atomic, true)
            }
            DeviceClientCapability::CursorPlaneHotspot => set_cursor_plane_hotspot_cap(device),
        }
    }

    fn driver_name(&self, device: &Device) -> io::Result<String> {
        use ::drm::Device as _;
        Ok(device.get_driver()?.name().to_string_lossy().into_owned())
    }
}

#[repr(C)]
struct DrmSetClientCap {
    capability: u64,
    value: u64,
}

// _IOW('d', 0x0d, struct drm_set_client_cap), whose two u64 fields make
// the ioctl payload 16 bytes on Linux glibc, Linux musl, and FreeBSD.
const DRM_IOCTL_SET_CLIENT_CAP: nix::sys::ioctl::ioctl_num_type =
    nix::request_code_write!(b'd', 0x0d, std::mem::size_of::<DrmSetClientCap>());
const DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT: u64 = 6;

fn set_cursor_plane_hotspot_cap(device: &Device) -> io::Result<()> {
    let mut request = DrmSetClientCap {
        capability: DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT,
        value: 1,
    };
    // The ioctl request's high bit is part of its unsigned bit pattern. `as _`
    // lets libc select the target's request parameter type without relying on
    // libc::Ioctl, which is not defined consistently by glibc, musl, and BSD.
    let result = unsafe {
        libc::ioctl(
            device.as_fd().as_raw_fd(),
            DRM_IOCTL_SET_CLIENT_CAP as _,
            &mut request,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Negotiate capabilities before callers enumerate any DRM planes.
pub(crate) fn negotiate_client_caps(
    device: &Device,
    control: &dyn DeviceControl,
) -> io::Result<CursorDeviceCaps> {
    if let Err(err) = control.set_client_capability(device, DeviceClientCapability::UniversalPlanes)
    {
        log::warn!(
            "DRM_CLIENT_CAP_UNIVERSAL_PLANES rejected ({err}); driver may expose universal planes by default"
        );
    }
    if let Err(err) = control.set_client_capability(device, DeviceClientCapability::Atomic) {
        log::warn!(
            "DRM_CLIENT_CAP_ATOMIC rejected ({err}); the driver may still honour atomic commits"
        );
    }
    let hotspot_cap_accepted =
        match control.set_client_capability(device, DeviceClientCapability::CursorPlaneHotspot) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT rejected ({err})");
                false
            }
        };
    let driver_name = control.driver_name(device)?;
    Ok(CursorDeviceCaps::new(hotspot_cap_accepted, driver_name))
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeviceControlCall {
    SetClientCapability(DeviceClientCapability),
    DriverName,
}

/// Scripted implementation for deterministic constructor tests.
#[cfg(test)]
pub(crate) struct RecordingDeviceControl {
    calls: Mutex<Vec<DeviceControlCall>>,
    capability_results: Mutex<VecDeque<Result<(), i32>>>,
    driver_name_result: Mutex<Option<Result<String, i32>>>,
}

#[cfg(test)]
impl RecordingDeviceControl {
    pub(crate) fn new(
        capability_results: impl IntoIterator<Item = Result<(), i32>>,
        driver_name_result: Result<String, i32>,
    ) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            capability_results: Mutex::new(capability_results.into_iter().collect()),
            driver_name_result: Mutex::new(Some(driver_name_result)),
        }
    }

    pub(crate) fn calls(&self) -> Vec<DeviceControlCall> {
        self.calls.lock().expect("control call mutex").clone()
    }
}

#[cfg(test)]
impl DeviceControl for RecordingDeviceControl {
    fn set_client_capability(
        &self,
        _device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()> {
        self.calls
            .lock()
            .expect("control call mutex")
            .push(DeviceControlCall::SetClientCapability(capability));
        match self
            .capability_results
            .lock()
            .expect("capability result mutex")
            .pop_front()
        {
            Some(Ok(())) => Ok(()),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(errno)),
            None => Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }

    fn driver_name(&self, _device: &Device) -> io::Result<String> {
        self.calls
            .lock()
            .expect("control call mutex")
            .push(DeviceControlCall::DriverName);
        match self
            .driver_name_result
            .lock()
            .expect("driver result mutex")
            .take()
        {
            Some(Ok(name)) => Ok(name),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(errno)),
            None => Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }
}

#[cfg(test)]
pub(crate) fn recording_control(driver_name: &str) -> Arc<RecordingDeviceControl> {
    Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Ok(())],
        Ok(driver_name.to_owned()),
    ))
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    #[test]
    // Mutation: changing the target's _IOW direction encoding must fail this ABI assertion.
    fn set_client_cap_ioctl_matches_target_abi() {
        #[cfg(all(
            target_os = "linux",
            not(any(
                target_arch = "mips",
                target_arch = "mips32r6",
                target_arch = "mips64",
                target_arch = "mips64r6",
                target_arch = "powerpc",
                target_arch = "powerpc64",
                target_arch = "sparc64"
            ))
        ))]
        let expected = 0x4010_640d_u64;

        #[cfg(any(
            target_os = "freebsd",
            all(
                target_os = "linux",
                any(
                    target_arch = "mips",
                    target_arch = "mips32r6",
                    target_arch = "mips64",
                    target_arch = "mips64r6",
                    target_arch = "powerpc",
                    target_arch = "powerpc64",
                    target_arch = "sparc64"
                )
            )
        ))]
        let expected = 0x8010_640d_u64;

        assert_eq!(
            format!("{:#x}", super::DRM_IOCTL_SET_CLIENT_CAP),
            format!("{expected:#x}"),
        );
    }
}
