use std::{io, num::NonZeroU32, rc::Rc};

use drm::{
    buffer::Handle as DrmBufferHandle,
    control::{Device as ControlDevice, framebuffer},
};

use super::lease::AllocationLease;
use crate::{drm::Device, kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GemOwner {
    Right,
    Gbm,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightState {
    Registered,
    FramebufferRemoved,
    Discharged,
    Frozen,
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DrmCleanupRight {
    pub(crate) device_key: DrmDeviceKey,
    pub(crate) incarnation: IncarnationId,
    pub(crate) fb: u32,
    pub(crate) gem: u32,
    pub(crate) gem_owner: GemOwner,
    pub(crate) state: RightState,
}

#[allow(dead_code)]
impl DrmCleanupRight {
    pub(crate) fn new(
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        fb: u32,
        gem: u32,
        gem_owner: GemOwner,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            fb,
            gem,
            gem_owner,
            state: RightState::Registered,
        }
    }

    pub(crate) fn fb(&self) -> u32 {
        self.fb
    }

    pub(crate) fn gem(&self) -> u32 {
        self.gem
    }

    pub(crate) fn gem_owner(&self) -> GemOwner {
        self.gem_owner
    }

    pub(crate) fn state(&self) -> RightState {
        self.state
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct FileFamilyClosed {
    pub(crate) device_key: DrmDeviceKey,
    pub(crate) incarnation: IncarnationId,
    _private: (),
}

#[allow(dead_code)]
pub(crate) trait CleanupIo {
    fn remove_fb(&mut self, fb: u32) -> io::Result<()>;
    fn close_gem(&mut self, gem: u32) -> io::Result<()>;
}

#[allow(dead_code)]
pub(crate) struct DeviceCleanupIo {
    device: Rc<Device>,
}

#[allow(dead_code)]
impl DeviceCleanupIo {
    pub(crate) fn new(device: Rc<Device>) -> Self {
        Self { device }
    }
}

impl CleanupIo for DeviceCleanupIo {
    fn remove_fb(&mut self, fb: u32) -> io::Result<()> {
        let nz = NonZeroU32::new(fb).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid zero framebuffer handle",
            )
        })?;
        let handle = framebuffer::Handle::from(nz);
        self.device.destroy_framebuffer(handle)?;
        Ok(())
    }

    fn close_gem(&mut self, gem: u32) -> io::Result<()> {
        let nz = NonZeroU32::new(gem).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid zero gem buffer handle",
            )
        })?;
        let handle = DrmBufferHandle::from(nz);
        self.device.close_buffer(handle)?;
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct FakeFamilyInventory {
    pub(crate) control_closed: bool,
    pub(crate) helper_reaped: bool,
    pub(crate) non_payload_aliases: usize,
}

#[allow(dead_code)]
pub(crate) struct DrmCleanupRegistry {
    device_key: DrmDeviceKey,
    incarnation: IncarnationId,
    io: Box<dyn CleanupIo>,
    device: Option<Rc<Device>>,
    frozen: bool,
    family_closed: bool,
    payload_aliases: usize,
    fake_family: Option<FakeFamilyInventory>,
}

impl std::fmt::Debug for DrmCleanupRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrmCleanupRegistry")
            .field("device_key", &self.device_key)
            .field("incarnation", &self.incarnation)
            .field("has_device", &self.device.is_some())
            .field("frozen", &self.frozen)
            .field("family_closed", &self.family_closed)
            .field("payload_aliases", &self.payload_aliases)
            .finish()
    }
}

#[allow(dead_code)]
impl DrmCleanupRegistry {
    pub(crate) fn new(
        device: Rc<Device>,
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
    ) -> Self {
        let io = Box::new(DeviceCleanupIo::new(Rc::clone(&device)));
        Self {
            device_key,
            incarnation,
            io,
            device: Some(device),
            frozen: false,
            family_closed: false,
            payload_aliases: 0,
            fake_family: None,
        }
    }

    pub(crate) fn new_with_io(
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        io: Box<dyn CleanupIo>,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            io,
            device: None,
            frozen: false,
            family_closed: false,
            payload_aliases: 0,
            fake_family: None,
        }
    }

    pub(crate) fn new_with_device_and_io(
        device: Rc<Device>,
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        io: Box<dyn CleanupIo>,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            io,
            device: Some(device),
            frozen: false,
            family_closed: false,
            payload_aliases: 0,
            fake_family: None,
        }
    }

    pub(crate) fn device_key(&self) -> DrmDeviceKey {
        self.device_key
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub(crate) fn is_family_closed(&self) -> bool {
        self.family_closed
    }

    pub(crate) fn register_right(
        &mut self,
        fb: u32,
        gem: u32,
        gem_owner: GemOwner,
    ) -> DrmCleanupRight {
        DrmCleanupRight::new(self.device_key, self.incarnation, fb, gem, gem_owner)
    }

    pub(crate) fn register_payload_alias(&mut self) {
        self.payload_aliases += 1;
    }

    pub(crate) fn unregister_payload_alias(&mut self) {
        self.payload_aliases = self.payload_aliases.saturating_sub(1);
    }

    pub(crate) fn payload_aliases(&self) -> usize {
        self.payload_aliases
    }

    pub(crate) fn freeze_incarnation(&mut self) {
        self.frozen = true;
    }

    pub(crate) fn consume(
        &mut self,
        mut right: DrmCleanupRight,
    ) -> Result<(), (io::Error, DrmCleanupRight)> {
        if self.frozen || right.state == RightState::Frozen {
            right.state = RightState::Frozen;
            return Err((io::Error::other("incarnation is frozen"), right));
        }

        if self.family_closed {
            return Err((
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "descriptor family is closed; ioctl prohibited",
                ),
                right,
            ));
        }

        if right.state == RightState::Registered {
            if let Err(err) = self.io.remove_fb(right.fb) {
                return Err((err, right));
            }
            right.state = RightState::FramebufferRemoved;
        }

        if right.state == RightState::FramebufferRemoved {
            let res = if right.gem_owner == GemOwner::Right {
                self.io.close_gem(right.gem)
            } else {
                Ok(())
            };
            if let Err(err) = res {
                return Err((err, right));
            }
            // GemOwner::Gbm never closes the GEM handle in DrmCleanupRight
            right.state = RightState::Discharged;
            let _ = right;
        }

        Ok(())
    }

    pub(crate) fn init_fake_family(&mut self) {
        self.fake_family = Some(FakeFamilyInventory::default());
    }

    pub(crate) fn close_fake_control(&mut self) {
        if let Some(fake) = &mut self.fake_family {
            fake.control_closed = true;
        }
    }

    pub(crate) fn reap_fake_helper(&mut self) {
        if let Some(fake) = &mut self.fake_family {
            fake.helper_reaped = true;
        }
    }

    pub(crate) fn add_fake_alias(&mut self) {
        if let Some(fake) = &mut self.fake_family {
            fake.non_payload_aliases += 1;
        }
    }

    pub(crate) fn remove_fake_alias(&mut self) {
        if let Some(fake) = &mut self.fake_family {
            fake.non_payload_aliases = fake.non_payload_aliases.saturating_sub(1);
        }
    }

    pub(crate) fn try_mint_file_family_closed(&mut self) -> Result<FileFamilyClosed, io::Error> {
        if self.family_closed {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "file family already closed",
            ));
        }

        if let Some(fake) = &self.fake_family {
            if !fake.control_closed {
                return Err(io::Error::other("control fd is not closed"));
            }
            if !fake.helper_reaped {
                return Err(io::Error::other("helper process is not reaped"));
            }
            if fake.non_payload_aliases > 0 {
                return Err(io::Error::other("non-payload aliases still active"));
            }
            if self.payload_aliases > 0 {
                return Err(io::Error::other("payload aliases still active"));
            }
        }

        if let Some(device) = &self.device {
            if self.payload_aliases > 0 {
                return Err(io::Error::other("payload aliases still active"));
            }
            // Only the registry's own strong reference must remain
            let strong_count = Rc::strong_count(device);
            if strong_count > 1 {
                return Err(io::Error::other(format!(
                    "external device aliases still active (count={strong_count})"
                )));
            }
        }

        // Registry performs the description's last close
        self.device = None;
        self.family_closed = true;

        Ok(FileFamilyClosed {
            device_key: self.device_key,
            incarnation: self.incarnation,
            _private: (),
        })
    }

    pub(crate) fn retire_closed_family(&mut self, proof: FileFamilyClosed) {
        assert_eq!(
            proof.device_key, self.device_key,
            "mismatched device key for family retirement"
        );
        assert_eq!(
            proof.incarnation, self.incarnation,
            "mismatched incarnation for family retirement"
        );
        self.device = None;
        self.family_closed = true;
    }
}

#[allow(dead_code)]
pub(crate) struct DirectFramebufferAllocation {
    pub(crate) right: Option<DrmCleanupRight>,
    pub(crate) source_lease: Option<AllocationLease>,
    pub(crate) fb: framebuffer::Handle,
    pub(crate) gem: DrmBufferHandle,
    pub(crate) gem_owner: GemOwner,
    pub(crate) device: Option<Rc<Device>>,
}

impl std::fmt::Debug for DirectFramebufferAllocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectFramebufferAllocation")
            .field("right", &self.right)
            .field("source_lease", &self.source_lease)
            .field("fb", &self.fb)
            .field("gem", &self.gem)
            .field("gem_owner", &self.gem_owner)
            .field("has_device", &self.device.is_some())
            .finish()
    }
}

#[allow(dead_code)]
impl DirectFramebufferAllocation {
    pub(crate) fn new(
        right: DrmCleanupRight,
        source_lease: Option<AllocationLease>,
        fb: framebuffer::Handle,
        gem: DrmBufferHandle,
        gem_owner: GemOwner,
        device: Option<Rc<Device>>,
    ) -> Self {
        Self {
            right: Some(right),
            source_lease,
            fb,
            gem,
            gem_owner,
            device,
        }
    }

    pub(crate) fn take_right(&mut self) -> Option<DrmCleanupRight> {
        self.right.take()
    }

    pub(crate) fn right(&self) -> Option<&DrmCleanupRight> {
        self.right.as_ref()
    }

    pub(crate) fn fb_handle(&self) -> framebuffer::Handle {
        self.fb
    }

    pub(crate) fn gem_handle(&self) -> DrmBufferHandle {
        self.gem
    }

    pub(crate) fn source_lease(&self) -> Option<&AllocationLease> {
        self.source_lease.as_ref()
    }
}
