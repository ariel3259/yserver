//! Injectable mode-blob operations used by the shared modeset path.

use std::{
    io,
    os::fd::{AsFd, AsRawFd},
};

use super::Device;

pub(crate) trait ModeBlobIo {
    fn create_property_blob(&self, device: &Device, mode: &::drm::control::Mode)
    -> io::Result<u64>;
    fn destroy_property_blob(&self, device: &Device, blob: u64) -> io::Result<()>;

    #[cfg(test)]
    fn for_tests_record_count(&self) -> Option<usize> {
        None
    }
}

#[derive(Default)]
pub(crate) struct DrmModeBlobIo;

impl ModeBlobIo for DrmModeBlobIo {
    fn create_property_blob(
        &self,
        device: &Device,
        mode: &::drm::control::Mode,
    ) -> io::Result<u64> {
        use ::drm::control::Device as _;
        device.create_property_blob(mode).map(Into::into)
    }

    fn destroy_property_blob(&self, device: &Device, blob: u64) -> io::Result<()> {
        use ::drm::control::Device as _;
        device.destroy_property_blob(blob)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModeBlobRecord {
    Create {
        device_fd: i32,
        blob: u64,
        width: u32,
        height: u32,
    },
    Destroy {
        device_fd: i32,
        blob: u64,
    },
}

/// Recording mode-blob operations for safe backend fixtures. Blob identities
/// are local to the owning test device and remain live until a successful
/// destroy operation.
#[derive(Default)]
#[allow(
    dead_code,
    reason = "the selected recorder is inspected by A1 fixtures"
)]
pub(crate) struct RecordingModeBlobIo {
    records: std::cell::RefCell<Vec<ModeBlobRecord>>,
    live: std::cell::RefCell<std::collections::HashSet<(i32, u64)>>,
    next_blob: std::cell::Cell<u64>,
}

impl RecordingModeBlobIo {
    #[allow(
        dead_code,
        reason = "the selected recorder is inspected by A1 fixtures"
    )]
    pub(crate) fn records(&self) -> Vec<ModeBlobRecord> {
        self.records.borrow().clone()
    }

    #[allow(
        dead_code,
        reason = "the selected recorder is inspected by A1 fixtures"
    )]
    pub(crate) fn live_count(&self) -> usize {
        self.live.borrow().len()
    }
}

impl ModeBlobIo for RecordingModeBlobIo {
    fn create_property_blob(
        &self,
        device: &Device,
        mode: &::drm::control::Mode,
    ) -> io::Result<u64> {
        let blob = self.next_blob.get().wrapping_add(1).max(1);
        self.next_blob.set(blob);
        let device_fd = device.as_fd().as_raw_fd();
        let (width, height) = mode.size();
        self.live.borrow_mut().insert((device_fd, blob));
        self.records.borrow_mut().push(ModeBlobRecord::Create {
            device_fd,
            blob,
            width: u32::from(width),
            height: u32::from(height),
        });
        Ok(blob)
    }

    fn destroy_property_blob(&self, device: &Device, blob: u64) -> io::Result<()> {
        let device_fd = device.as_fd().as_raw_fd();
        if !self.live.borrow_mut().remove(&(device_fd, blob)) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown recording mode blob",
            ));
        }
        self.records
            .borrow_mut()
            .push(ModeBlobRecord::Destroy { device_fd, blob });
        Ok(())
    }

    #[cfg(test)]
    fn for_tests_record_count(&self) -> Option<usize> {
        Some(self.records.borrow().len())
    }
}
