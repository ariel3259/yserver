//! Injectable mode-blob operations used by the shared modeset path.

use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, AsRawFd},
};

use ::drm::control::{connector, crtc, plane, property};

use super::Device;

pub(crate) trait ModeBlobIo {
    fn properties_for_object(
        &self,
        device: &Device,
        object: ModesetObject,
    ) -> io::Result<HashMap<String, property::Handle>>;
    fn create_property_blob(&self, device: &Device, mode: &::drm::control::Mode)
    -> io::Result<u64>;
    fn destroy_property_blob(&self, device: &Device, blob: u64) -> io::Result<()>;

    #[cfg(test)]
    fn for_tests_record_count(&self) -> Option<usize> {
        None
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ModesetObject {
    Connector(connector::Handle),
    Crtc(crtc::Handle),
    Plane(plane::Handle),
}

#[derive(Default)]
pub(crate) struct DrmModeBlobIo;

impl ModeBlobIo for DrmModeBlobIo {
    fn properties_for_object(
        &self,
        device: &Device,
        object: ModesetObject,
    ) -> io::Result<HashMap<String, property::Handle>> {
        use ::drm::control::Device as _;

        let properties = match object {
            ModesetObject::Connector(handle) => device.get_properties(handle)?,
            ModesetObject::Crtc(handle) => device.get_properties(handle)?,
            ModesetObject::Plane(handle) => device.get_properties(handle)?,
        };
        Ok(properties
            .as_hashmap(device)?
            .into_iter()
            .map(|(name, info)| (name, info.handle()))
            .collect())
    }

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
    fn properties_for_object(
        &self,
        _device: &Device,
        object: ModesetObject,
    ) -> io::Result<HashMap<String, property::Handle>> {
        let (base, names): (u32, &[&str]) = match object {
            ModesetObject::Connector(_) => (0xD000, &["CRTC_ID"]),
            ModesetObject::Crtc(_) => (0xD100, &["ACTIVE", "MODE_ID"]),
            ModesetObject::Plane(_) => (
                0xD200,
                &[
                    "FB_ID", "CRTC_ID", "SRC_X", "SRC_Y", "SRC_W", "SRC_H", "CRTC_X", "CRTC_Y",
                    "CRTC_W", "CRTC_H",
                ],
            ),
        };
        Ok(names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    (*name).to_owned(),
                    ::drm::control::from_u32(base + index as u32 + 1)
                        .expect("recording modeset property handle"),
                )
            })
            .collect())
    }

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
