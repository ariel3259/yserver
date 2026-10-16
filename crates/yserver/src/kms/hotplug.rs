//! DRM display hotplug detection for the KMS backend.
//!
//! Linux-only: a udev monitor on the `drm` subsystem exposes a pollable
//! fd. The backend drains it and debounces the expensive reprobe work.

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::{cell::RefCell, os::fd::RawFd};

pub(crate) trait DisplayHotplugSource {
    fn raw_fd(&self) -> RawFd;
    fn drain_changes(&self) -> bool;
}

#[derive(Default)]
#[allow(
    dead_code,
    reason = "fallback source used when DRM hotplug monitoring is unavailable"
)]
pub(crate) struct EmptyHotplugSource;

impl DisplayHotplugSource for EmptyHotplugSource {
    fn raw_fd(&self) -> RawFd {
        -1
    }

    fn drain_changes(&self) -> bool {
        false
    }
}

#[allow(
    dead_code,
    reason = "the selected recorder is exercised by A1 fixtures"
)]
pub(crate) struct RecordingDisplayHotplugSource {
    reader: RefCell<std::os::unix::net::UnixStream>,
    writer: RefCell<std::os::unix::net::UnixStream>,
}

impl RecordingDisplayHotplugSource {
    #[allow(
        dead_code,
        reason = "queued hotplug events are exercised by A1 fixtures"
    )]
    pub(crate) fn new() -> std::io::Result<Self> {
        let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self {
            reader: RefCell::new(reader),
            writer: RefCell::new(writer),
        })
    }

    #[allow(
        dead_code,
        reason = "queued hotplug events are exercised by A1 fixtures"
    )]
    pub(crate) fn queue_change(&self) -> std::io::Result<()> {
        use std::io::Write as _;
        self.writer.borrow_mut().write_all(&[1])
    }
}

impl DisplayHotplugSource for RecordingDisplayHotplugSource {
    fn raw_fd(&self) -> RawFd {
        use std::os::fd::AsRawFd as _;
        self.reader.borrow().as_raw_fd()
    }

    fn drain_changes(&self) -> bool {
        use std::io::Read as _;
        let mut reader = self.reader.borrow_mut();
        let mut saw_change = false;
        let mut buffer = [0_u8; 64];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => saw_change = true,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    log::warn!("recording DRM hotplug source read failed: {error}");
                    break;
                }
            }
        }
        saw_change
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct DrmHotplugMonitor {
    socket: RefCell<udev::MonitorSocket>,
}

#[cfg(target_os = "linux")]
impl DrmHotplugMonitor {
    pub(crate) fn new() -> std::io::Result<Option<Self>> {
        let builder = match udev::MonitorBuilder::new() {
            Ok(builder) => builder,
            Err(e) => {
                log::warn!("drm hotplug: udev monitor unavailable: {e}; hotplug disabled");
                return Ok(None);
            }
        };
        let socket = builder.match_subsystem("drm")?.listen()?;
        log::info!("drm hotplug: udev monitor listening on drm subsystem");
        Ok(Some(Self {
            socket: RefCell::new(socket),
        }))
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.socket.borrow().as_raw_fd()
    }

    pub(crate) fn drain(&mut self) -> bool {
        let mut saw_change = false;
        for event in self.socket.get_mut().iter() {
            let action = event.action().and_then(|a| a.to_str()).unwrap_or("");
            log::debug!("drm hotplug: uevent action={action:?}");
            if matches!(action, "add" | "remove" | "change") {
                saw_change = true;
            }
        }
        saw_change
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct DrmDisplayHotplugSource {
    monitor: RefCell<Option<DrmHotplugMonitor>>,
}

#[cfg(target_os = "linux")]
impl DrmDisplayHotplugSource {
    pub(crate) fn new() -> Self {
        let monitor = match DrmHotplugMonitor::new() {
            Ok(monitor) => monitor,
            Err(error) => {
                log::warn!("render PlatformBackend: DRM hotplug monitor unavailable ({error})");
                None
            }
        };
        Self {
            monitor: RefCell::new(monitor),
        }
    }
}

#[cfg(target_os = "linux")]
impl DisplayHotplugSource for DrmDisplayHotplugSource {
    fn raw_fd(&self) -> RawFd {
        self.monitor
            .borrow()
            .as_ref()
            .map_or(-1, DrmHotplugMonitor::raw_fd)
    }

    fn drain_changes(&self) -> bool {
        self.monitor
            .borrow_mut()
            .as_mut()
            .is_some_and(|monitor| monitor.drain())
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub(crate) struct DrmDisplayHotplugSource;

#[cfg(not(target_os = "linux"))]
impl DrmDisplayHotplugSource {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[cfg(not(target_os = "linux"))]
impl DisplayHotplugSource for DrmDisplayHotplugSource {
    fn raw_fd(&self) -> RawFd {
        -1
    }

    fn drain_changes(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn monitor_opens_and_drains_without_blocking() {
        match super::DrmHotplugMonitor::new() {
            Ok(Some(mut monitor)) => {
                assert!(monitor.raw_fd() >= 0);
                let _ = monitor.drain();
            }
            Ok(None) => {}
            Err(e) => panic!("monitor construction errored unexpectedly: {e}"),
        }
    }
}
