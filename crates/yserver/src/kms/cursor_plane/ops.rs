//! Raw cursor-buffer operations and the shared dumb-buffer allocation path.

use std::{
    collections::{HashMap, HashSet},
    io,
    os::fd::{AsFd, AsRawFd},
    ptr::NonNull,
    rc::Rc,
};

use std::cell::RefCell;

use drm::{
    buffer::{Buffer, DrmFourcc, DrmModifier, Handle as DrmBufferHandle, PlanarBuffer},
    control::{Device as ControlDevice, FbCmd2Flags, crtc, dumbbuffer::DumbBuffer, framebuffer},
};

use crate::drm::Device;

pub(crate) struct CursorDumbBuffer {
    device_fd: i32,
    #[allow(dead_code)] // recording fixtures use the raw identity
    handle: u32,
    pitch: u32,
    #[allow(dead_code)] // recording fixtures use the raw allocation length
    len: usize,
    dumb: Option<DumbBuffer>,
}

impl CursorDumbBuffer {
    #[allow(dead_code)]
    pub(crate) fn handle(&self) -> u32 {
        self.handle
    }

    pub(crate) fn pitch(&self) -> u32 {
        self.pitch
    }

    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn as_dumb(&self) -> Option<DumbBuffer> {
        self.dumb
    }

    #[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
    pub(crate) fn recording(device: &Device, handle: u32, pitch: u32, len: usize) -> Self {
        Self {
            device_fd: device.as_fd().as_raw_fd(),
            handle,
            pitch,
            len,
            dumb: None,
        }
    }
}

pub(crate) struct CursorMapping {
    ptr: NonNull<u8>,
    len: usize,
    owned: Option<Box<[u8]>>,
    _recording: Option<Rc<RefCell<Box<[u8]>>>>,
}

impl CursorMapping {
    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
    pub(crate) fn recording(backing: Rc<RefCell<Box<[u8]>>>) -> io::Result<Self> {
        let ptr = NonNull::new(backing.borrow_mut().as_mut_ptr())
            .ok_or_else(|| io::Error::other("empty cursor mapping"))?;
        let len = backing.borrow().len();
        Ok(Self {
            ptr,
            len,
            owned: None,
            _recording: Some(backing),
        })
    }
}

pub(crate) trait CursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer>;
    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping>;
    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle>;
    fn write_image(
        &self,
        mapping: &CursorMapping,
        stride: u32,
        capacity_width: u32,
        capacity_height: u32,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()>;
    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()>;
    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, mapping: CursorMapping);
    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()>;
}

#[derive(Clone, Copy)]
pub(crate) struct CursorFramebufferDescriptor {
    width: u32,
    height: u32,
    format: DrmFourcc,
}

impl CursorFramebufferDescriptor {
    fn argb8888(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: DrmFourcc::Argb8888,
        }
    }
}

/// Legacy cursor operations retained until the atomic cursor submission path
/// replaces them. This seam records the actual bind/move/hide requests.
pub(crate) trait LegacyCursorIo {
    fn bind(
        &self,
        device: &Device,
        crtc: drm::control::crtc::Handle,
        buffer: Option<&CursorBuffer>,
        hotspot: (i32, i32),
    ) -> io::Result<()>;
    fn position(
        &self,
        device: &Device,
        crtc: drm::control::crtc::Handle,
        x: i32,
        y: i32,
    ) -> io::Result<()>;
}

#[derive(Default)]
pub(crate) struct DrmLegacyCursorIo;

impl LegacyCursorIo for DrmLegacyCursorIo {
    #[allow(deprecated)]
    fn bind(
        &self,
        device: &Device,
        crtc: drm::control::crtc::Handle,
        buffer: Option<&CursorBuffer>,
        hotspot: (i32, i32),
    ) -> io::Result<()> {
        match buffer {
            Some(buffer) => {
                let dumb = buffer
                    .dumb()
                    .ok_or_else(|| io::Error::other("cursor buffer has no DRM dumb handle"))?;
                device.set_cursor2(crtc, Some(&dumb), hotspot)
            }
            None => device.set_cursor2::<DumbBuffer>(crtc, None, (0, 0)),
        }
    }

    #[allow(deprecated)]
    fn position(
        &self,
        device: &Device,
        crtc: drm::control::crtc::Handle,
        x: i32,
        y: i32,
    ) -> io::Result<()> {
        device.move_cursor(crtc, (x, y))
    }
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyCursorOperation {
    Bind,
    Move,
    Hide,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LegacyCursorRecord {
    pub(crate) device_fd: i32,
    pub(crate) crtc: crtc::Handle,
    pub(crate) operation: LegacyCursorOperation,
    pub(crate) framebuffer: Option<u32>,
    pub(crate) hotspot: (i32, i32),
    pub(crate) position: Option<(i32, i32)>,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Default)]
pub(crate) struct RecordingLegacyCursorIo {
    records: RefCell<Vec<LegacyCursorRecord>>,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
impl RecordingLegacyCursorIo {
    pub(crate) fn records(&self) -> Vec<LegacyCursorRecord> {
        self.records.borrow().clone()
    }
}

impl LegacyCursorIo for RecordingLegacyCursorIo {
    fn bind(
        &self,
        device: &Device,
        crtc: crtc::Handle,
        buffer: Option<&CursorBuffer>,
        hotspot: (i32, i32),
    ) -> io::Result<()> {
        self.records.borrow_mut().push(LegacyCursorRecord {
            device_fd: device.as_fd().as_raw_fd(),
            crtc,
            operation: if buffer.is_some() {
                LegacyCursorOperation::Bind
            } else {
                LegacyCursorOperation::Hide
            },
            framebuffer: buffer.map(|buffer| u32::from(buffer.fb_id())),
            hotspot,
            position: None,
        });
        Ok(())
    }

    fn position(&self, device: &Device, crtc: crtc::Handle, x: i32, y: i32) -> io::Result<()> {
        self.records.borrow_mut().push(LegacyCursorRecord {
            device_fd: device.as_fd().as_raw_fd(),
            crtc,
            operation: LegacyCursorOperation::Move,
            framebuffer: None,
            hotspot: (0, 0),
            position: Some((x, y)),
        });
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct DrmCursorBufferIo;

impl CursorBufferIo for DrmCursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer> {
        let dumb = device.create_dumb_buffer(
            (descriptor.width, descriptor.height),
            descriptor.format,
            32,
        )?;
        let handle = dumb.handle().into();
        let pitch = dumb.pitch();
        let len = usize::try_from(dumb.size().0)
            .ok()
            .and_then(|row| row.checked_mul(usize::try_from(dumb.size().1).ok()?))
            .and_then(|_| {
                usize::try_from(dumb.pitch())
                    .ok()?
                    .checked_mul(usize::try_from(dumb.size().1).ok()?)
            })
            .ok_or_else(|| io::Error::other("cursor dumb buffer size overflow"))?;
        Ok(CursorDumbBuffer {
            device_fd: device.as_fd().as_raw_fd(),
            handle,
            pitch,
            len,
            dumb: Some(dumb),
        })
    }

    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .as_mut()
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM mapper"))?;
        let mapping = device.map_dumb_buffer(raw_dumb)?;
        let len = mapping.len();
        let ptr = NonNull::new(mapping.as_ptr() as *mut u8)
            .ok_or_else(|| io::Error::other("non-null dumb mapping expected"))?;
        std::mem::forget(mapping);
        Ok(CursorMapping {
            ptr,
            len,
            owned: None,
            _recording: None,
        })
    }

    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .as_ref()
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM AddFB2"))?;
        device.add_planar_framebuffer(
            &CursorPlanarBuffer {
                dumb: *raw_dumb,
                descriptor,
            },
            FbCmd2Flags::empty(),
        )
    }

    fn write_image(
        &self,
        mapping: &CursorMapping,
        stride: u32,
        capacity_width: u32,
        capacity_height: u32,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()> {
        write_mapped_image(
            mapping,
            stride,
            capacity_width,
            capacity_height,
            image_width,
            image_height,
            bgra_bytes,
        )
    }

    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()> {
        device.destroy_framebuffer(fb)
    }

    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, mapping: CursorMapping) {
        if ensure_device(device, dumb.device_fd).is_err() {
            log::error!("cursor buffer mapping belonged to a different DRM device");
            return;
        }
        if mapping.owned.is_none()
            // SAFETY: this mapping was created by drm-rs for `dumb` and its
            // ownership was transferred out of DumbMapping in `map_dumb`.
            && unsafe { libc::munmap(mapping.ptr.as_ptr().cast(), mapping.len) } != 0
        {
            log::warn!(
                "cursor dumb-buffer munmap failed: {}",
                io::Error::last_os_error()
            );
        }
    }

    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM destroy"))?;
        device.destroy_dumb_buffer(raw_dumb)
    }
}

fn ensure_device(device: &Device, expected_fd: i32) -> io::Result<()> {
    if device.as_fd().as_raw_fd() == expected_fd {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor resource used with a different DRM device",
        ))
    }
}

struct CursorPlanarBuffer {
    dumb: DumbBuffer,
    descriptor: CursorFramebufferDescriptor,
}

impl PlanarBuffer for CursorPlanarBuffer {
    fn size(&self) -> (u32, u32) {
        (self.descriptor.width, self.descriptor.height)
    }

    fn format(&self) -> DrmFourcc {
        self.descriptor.format
    }

    fn modifier(&self) -> Option<DrmModifier> {
        None
    }

    fn pitches(&self) -> [u32; 4] {
        [self.dumb.pitch(), 0, 0, 0]
    }

    fn handles(&self) -> [Option<DrmBufferHandle>; 4] {
        [Some(self.dumb.handle()), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [0; 4]
    }
}

pub(crate) struct CursorBuffer {
    device: Rc<Device>,
    io: Rc<dyn CursorBufferIo>,
    dumb: Option<CursorDumbBuffer>,
    mapping: Option<CursorMapping>,
    fb: Option<framebuffer::Handle>,
    stride: u32,
    len: usize,
    width: u32,
    height: u32,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CursorBufferRecord {
    Create {
        device_fd: i32,
        handle: u32,
        width: u32,
        height: u32,
    },
    Map {
        device_fd: i32,
        handle: u32,
    },
    AddFb {
        device_fd: i32,
        handle: u32,
        fb: u32,
        width: u32,
        height: u32,
        format: u32,
    },
    RemoveFb {
        device_fd: i32,
        fb: u32,
    },
    Unmap {
        device_fd: i32,
        handle: u32,
    },
    Destroy {
        device_fd: i32,
        handle: u32,
    },
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Default)]
pub(crate) struct CursorBufferLedger {
    records: RefCell<Vec<CursorBufferRecord>>,
    live_dumb: RefCell<HashSet<(i32, u32)>>,
    live_mappings: RefCell<HashSet<(i32, u32)>>,
    live_framebuffers: RefCell<HashSet<(i32, u32)>>,
    storage: RefCell<CursorBufferStorageMap>,
    failures: RefCell<Vec<CursorBufferFailure>>,
}

type CursorBufferStorageMap = HashMap<(i32, u32), Rc<RefCell<Box<[u8]>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorBufferOperation {
    Create,
    Map,
    AddFb,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CursorBufferFailure {
    operation: CursorBufferOperation,
    successful_calls_before_failure: usize,
    raw_os_error: i32,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
impl CursorBufferLedger {
    pub(crate) fn records(&self) -> Vec<CursorBufferRecord> {
        self.records.borrow().clone()
    }

    pub(crate) fn end_state(&self) -> (usize, usize, usize) {
        (
            self.live_dumb.borrow().len(),
            self.live_mappings.borrow().len(),
            self.live_framebuffers.borrow().len(),
        )
    }

    #[allow(
        dead_code,
        reason = "A4 Backend allocation tests inject raw acquisition failures"
    )]
    pub(crate) fn fail_after(
        &self,
        operation: CursorBufferOperation,
        successful_calls: usize,
        raw_os_error: i32,
    ) {
        self.failures.borrow_mut().push(CursorBufferFailure {
            operation,
            successful_calls_before_failure: successful_calls,
            raw_os_error,
        });
    }

    #[allow(
        dead_code,
        reason = "A4 Backend upload tests inspect the recorded mmap bytes"
    )]
    pub(crate) fn mapped_bytes(&self, device_fd: i32, handle: u32) -> Option<Vec<u8>> {
        self.storage
            .borrow()
            .get(&(device_fd, handle))
            .map(|storage| storage.borrow().to_vec())
    }

    fn take_failure(&self, operation: CursorBufferOperation) -> Option<i32> {
        let mut failures = self.failures.borrow_mut();
        let index = failures
            .iter()
            .position(|failure| failure.operation == operation)?;
        let failure = &mut failures[index];
        if failure.successful_calls_before_failure != 0 {
            failure.successful_calls_before_failure -= 1;
            return None;
        }
        Some(failures.remove(index).raw_os_error)
    }
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
pub(crate) struct RecordingCursorBufferIo {
    ledger: Rc<CursorBufferLedger>,
    next_handle: RefCell<u32>,
    next_fb: RefCell<u32>,
    framebuffer_devices: RefCell<HashMap<u32, i32>>,
    pitch_alignment: u32,
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
impl RecordingCursorBufferIo {
    pub(crate) fn new(pitch_alignment: u32) -> (Self, Rc<CursorBufferLedger>) {
        let ledger = Rc::new(CursorBufferLedger::default());
        (
            Self {
                ledger: Rc::clone(&ledger),
                next_handle: RefCell::new(1),
                next_fb: RefCell::new(100),
                framebuffer_devices: RefCell::new(HashMap::new()),
                pitch_alignment: pitch_alignment.max(1),
            },
            ledger,
        )
    }
}

impl CursorBufferIo for RecordingCursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Create) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        let width = descriptor.width;
        let height = descriptor.height;
        let mut next = self.next_handle.borrow_mut();
        let handle = *next;
        *next = next.wrapping_add(1).max(1);
        let tight_pitch = width
            .checked_mul(4)
            .ok_or_else(|| io::Error::other("cursor pitch overflow"))?;
        let alignment = self.pitch_alignment;
        let pitch = tight_pitch
            .checked_add(alignment - 1)
            .map(|value| value / alignment * alignment)
            .ok_or_else(|| io::Error::other("cursor padded pitch overflow"))?;
        let len = usize::try_from(pitch)
            .ok()
            .and_then(|pitch| pitch.checked_mul(usize::try_from(height).ok()?))
            .ok_or_else(|| io::Error::other("cursor mapping size overflow"))?;
        self.ledger.live_dumb.borrow_mut().insert((fd, handle));
        self.ledger.storage.borrow_mut().insert(
            (fd, handle),
            Rc::new(RefCell::new(vec![0_u8; len].into_boxed_slice())),
        );
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Create {
                device_fd: fd,
                handle,
                width,
                height,
            });
        Ok(CursorDumbBuffer::recording(device, handle, pitch, len))
    }

    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Map) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self.ledger.live_dumb.borrow().contains(&(fd, dumb.handle)) {
            return Err(io::Error::other("recording cursor dumb buffer is not live"));
        }
        self.ledger
            .live_mappings
            .borrow_mut()
            .insert((fd, dumb.handle));
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Map {
                device_fd: fd,
                handle: dumb.handle,
            });
        let storage = self
            .ledger
            .storage
            .borrow()
            .get(&(fd, dumb.handle))
            .cloned()
            .ok_or_else(|| io::Error::other("recording cursor backing is not live"))?;
        CursorMapping::recording(storage)
    }

    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::AddFb) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self
            .ledger
            .live_mappings
            .borrow()
            .contains(&(fd, dumb.handle))
        {
            return Err(io::Error::other("recording cursor buffer is not mapped"));
        }
        let mut next = self.next_fb.borrow_mut();
        let raw_fb = *next;
        *next = next.wrapping_add(1).max(100);
        let fb = ::drm::control::from_u32(raw_fb)
            .ok_or_else(|| io::Error::other("invalid recording framebuffer handle"))?;
        self.ledger
            .live_framebuffers
            .borrow_mut()
            .insert((fd, raw_fb));
        self.framebuffer_devices.borrow_mut().insert(raw_fb, fd);
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::AddFb {
                device_fd: fd,
                handle: dumb.handle,
                fb: raw_fb,
                width: descriptor.width,
                height: descriptor.height,
                format: descriptor.format as u32,
            });
        Ok(fb)
    }

    fn write_image(
        &self,
        mapping: &CursorMapping,
        stride: u32,
        capacity_width: u32,
        capacity_height: u32,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Write) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        write_mapped_image(
            mapping,
            stride,
            capacity_width,
            capacity_height,
            image_width,
            image_height,
            bgra_bytes,
        )
    }

    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()> {
        let fd = device.as_fd().as_raw_fd();
        let raw_fb = u32::from(fb);
        if self.framebuffer_devices.borrow_mut().remove(&raw_fb) != Some(fd)
            || !self
                .ledger
                .live_framebuffers
                .borrow_mut()
                .remove(&(fd, raw_fb))
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown recording cursor FB",
            ));
        }
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::RemoveFb {
                device_fd: fd,
                fb: raw_fb,
            });
        Ok(())
    }

    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, _mapping: CursorMapping) {
        let fd = device.as_fd().as_raw_fd();
        if ensure_device(device, dumb.device_fd).is_err()
            || !self
                .ledger
                .live_mappings
                .borrow_mut()
                .remove(&(fd, dumb.handle))
        {
            log::error!("unknown recording cursor mapping during unmap");
            return;
        }
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Unmap {
                device_fd: fd,
                handle: dumb.handle,
            });
    }

    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()> {
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self
            .ledger
            .live_dumb
            .borrow_mut()
            .remove(&(fd, dumb.handle))
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown recording cursor dumb buffer",
            ));
        }
        self.ledger.storage.borrow_mut().remove(&(fd, dumb.handle));
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Destroy {
                device_fd: fd,
                handle: dumb.handle,
            });
        Ok(())
    }
}

impl CursorBuffer {
    /// Shared production allocation algorithm. Rollback releases only
    /// successfully acquired resources, in reverse acquisition order.
    pub(crate) fn allocate(
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        width: u32,
        height: u32,
    ) -> io::Result<Self> {
        let descriptor = CursorFramebufferDescriptor::argb8888(width, height);
        let mut dumb = io.create_dumb(&device, descriptor)?;
        let stride = dumb.pitch();
        let mapping = match io.map_dumb(&device, &mut dumb) {
            Ok(mapping) => mapping,
            Err(error) => {
                let handle = dumb.handle;
                let cleanup = io.destroy_dumb(&device, dumb);
                return Err(allocation_cleanup_error(
                    error,
                    format!("destroy cursor dumb buffer {handle:#x}"),
                    cleanup,
                ));
            }
        };
        let fb = match io.add_framebuffer(&device, &dumb, descriptor) {
            Ok(fb) => fb,
            Err(error) => {
                io.unmap_dumb(&device, &dumb, mapping);
                let handle = dumb.handle;
                let cleanup = io.destroy_dumb(&device, dumb);
                return Err(allocation_cleanup_error(
                    error,
                    format!("destroy cursor dumb buffer {handle:#x}"),
                    cleanup,
                ));
            }
        };
        let len = mapping.len();
        let mut buffer = Self {
            device,
            io,
            dumb: Some(dumb),
            mapping: Some(mapping),
            fb: Some(fb),
            stride,
            len,
            width,
            height,
        };
        // The newly allocated ARGB image starts transparent.
        buffer.clear();
        Ok(buffer)
    }

    pub(crate) fn release(mut self) -> io::Result<()> {
        self.release_resources()
    }

    fn release_resources(&mut self) -> io::Result<()> {
        let mut cleanup_error = None;
        if let Some(fb) = self.fb.take()
            && let Err(error) = self.io.remove_framebuffer(&self.device, fb)
        {
            cleanup_error = Some(error);
        }
        if let (Some(dumb), Some(mapping)) = (self.dumb.as_ref(), self.mapping.take()) {
            self.io.unmap_dumb(&self.device, dumb, mapping);
        }
        if let Some(dumb) = self.dumb.take()
            && let Err(error) = self.io.destroy_dumb(&self.device, dumb)
        {
            if let Some(previous) = cleanup_error {
                cleanup_error = Some(io::Error::other(format!(
                    "{previous}; destroy cursor dumb buffer failed: {error}"
                )));
            } else {
                cleanup_error = Some(error);
            }
        }
        cleanup_error.map_or(Ok(()), Err)
    }

    #[allow(dead_code)] // consumed by atomic cursor operations in the next phase
    pub(crate) fn fb_id(&self) -> framebuffer::Handle {
        self.fb.expect("cursor framebuffer is live")
    }

    pub(crate) fn dumb(&self) -> Option<DumbBuffer> {
        self.dumb
            .as_ref()
            .expect("cursor dumb buffer is live")
            .as_dumb()
    }

    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.mapping.as_ref().expect("cursor mapping is live").ptr()
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn stride(&self) -> u32 {
        self.stride
    }

    pub(crate) fn io(&self) -> Rc<dyn CursorBufferIo> {
        Rc::clone(&self.io)
    }

    pub(crate) fn write_image(
        &mut self,
        image_w: u32,
        image_h: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()> {
        if image_w == 0 || image_h == 0 || image_w > self.width || image_h > self.height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor exceeds sprite framebuffer dimensions",
            ));
        }
        let row_len = usize::try_from(image_w)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor width overflow"))?;
        let expected = row_len
            .checked_mul(usize::try_from(image_h).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow")
            })?)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor byte size overflow")
            })?;
        if bgra_bytes.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor bytes must equal width*height*4",
            ));
        }
        let _ = expected;
        self.io.write_image(
            self.mapping
                .as_ref()
                .expect("cursor mapping is live during upload"),
            self.stride,
            self.width,
            self.height,
            image_w,
            image_h,
            bgra_bytes,
        )
    }

    pub(crate) fn clear(&mut self) {
        // SAFETY: mapping remains live and covers `len` writable bytes.
        unsafe { std::ptr::write_bytes(self.ptr().as_ptr(), 0, self.len) };
    }
}

fn write_mapped_image(
    mapping: &CursorMapping,
    stride: u32,
    capacity_width: u32,
    capacity_height: u32,
    image_width: u32,
    image_height: u32,
    bgra_bytes: &[u8],
) -> io::Result<()> {
    let pitch = usize::try_from(stride)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cursor pitch overflow"))?;
    let row_len = usize::try_from(image_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor row overflow"))?;
    let capacity_row_len = usize::try_from(capacity_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor width overflow"))?;
    if row_len > pitch || capacity_row_len > pitch || image_height > capacity_height {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor image does not fit the mapped framebuffer geometry",
        ));
    }
    let len = pitch
        .checked_mul(
            usize::try_from(capacity_height).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow")
            })?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor mapping overflow"))?;
    if len > mapping.len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor mapping is smaller than its framebuffer geometry",
        ));
    }
    // SAFETY: the allocation mapping covers its padded pitch and height.
    unsafe { std::ptr::write_bytes(mapping.ptr.as_ptr(), 0, len) };
    for row in 0..usize::try_from(image_height)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow"))?
    {
        // SAFETY: image validation checked the source size and each
        // destination row fits within the padded framebuffer pitch.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bgra_bytes.as_ptr().add(row * row_len),
                mapping.ptr.as_ptr().add(row * pitch),
                row_len,
            );
        }
    }
    Ok(())
}

fn allocation_cleanup_error(
    operation_error: io::Error,
    cleanup_context: String,
    cleanup_result: io::Result<()>,
) -> io::Error {
    match cleanup_result {
        Ok(()) => operation_error,
        Err(cleanup_error) => io::Error::other(format!(
            "{operation_error}; rollback {cleanup_context} failed: {cleanup_error}"
        )),
    }
}

impl Drop for CursorBuffer {
    fn drop(&mut self) {
        if let Err(error) = self.release_resources() {
            log::warn!("cursor buffer cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MapAndDestroyFailure;

    impl CursorBufferIo for MapAndDestroyFailure {
        fn create_dumb(
            &self,
            device: &Device,
            _descriptor: CursorFramebufferDescriptor,
        ) -> io::Result<CursorDumbBuffer> {
            Ok(CursorDumbBuffer::recording(device, 0x51, 256, 4096))
        }

        fn map_dumb(
            &self,
            _device: &Device,
            _dumb: &mut CursorDumbBuffer,
        ) -> io::Result<CursorMapping> {
            Err(io::Error::other("map failed"))
        }

        fn add_framebuffer(
            &self,
            _device: &Device,
            _dumb: &CursorDumbBuffer,
            _descriptor: CursorFramebufferDescriptor,
        ) -> io::Result<framebuffer::Handle> {
            unreachable!("framebuffer creation follows a successful map")
        }

        fn write_image(
            &self,
            _mapping: &CursorMapping,
            _stride: u32,
            _capacity_width: u32,
            _capacity_height: u32,
            _image_width: u32,
            _image_height: u32,
            _bgra_bytes: &[u8],
        ) -> io::Result<()> {
            unreachable!("image write follows successful allocation")
        }

        fn remove_framebuffer(&self, _device: &Device, _fb: framebuffer::Handle) -> io::Result<()> {
            unreachable!("allocation did not reach framebuffer creation")
        }

        fn unmap_dumb(&self, _device: &Device, _dumb: &CursorDumbBuffer, _mapping: CursorMapping) {
            unreachable!("allocation did not create a mapping")
        }

        fn destroy_dumb(&self, _device: &Device, _dumb: CursorDumbBuffer) -> io::Result<()> {
            Err(io::Error::other("destroy failed"))
        }
    }

    #[test]
    // Mutation: discard destroy_dumb's rollback error; cleanup context must remain observable.
    fn cursor_allocation_preserves_map_and_destroy_errors() {
        let device = Rc::new(Device::for_tests().expect("test device"));
        let error = match CursorBuffer::allocate(device, Rc::new(MapAndDestroyFailure), 32, 32) {
            Ok(_) => panic!("map failure must reject cursor allocation"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::Other);
        let message = error.to_string();
        assert!(message.contains("map failed"), "{message}");
        assert!(
            message.contains("destroy cursor dumb buffer 0x51"),
            "{message}"
        );
        assert!(message.contains("destroy failed"), "{message}");
    }
}
