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
}

impl CursorMapping {
    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
    pub(crate) fn recording(len: usize) -> io::Result<Self> {
        let mut owned = vec![0_u8; len].into_boxed_slice();
        let ptr = NonNull::new(owned.as_mut_ptr())
            .ok_or_else(|| io::Error::other("empty cursor mapping"))?;
        Ok(Self {
            ptr,
            len,
            owned: Some(owned),
        })
    }
}

pub(crate) trait CursorBufferIo {
    fn create_dumb(&self, device: &Device, width: u32, height: u32)
    -> io::Result<CursorDumbBuffer>;
    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping>;
    fn add_argb_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        width: u32,
        height: u32,
    ) -> io::Result<framebuffer::Handle>;
    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()>;
    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, mapping: CursorMapping);
    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()>;
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
        width: u32,
        height: u32,
    ) -> io::Result<CursorDumbBuffer> {
        let dumb = device.create_dumb_buffer((width, height), DrmFourcc::Argb8888, 32)?;
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
        })
    }

    fn add_argb_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        width: u32,
        height: u32,
    ) -> io::Result<framebuffer::Handle> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .as_ref()
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM AddFB2"))?;
        device.add_planar_framebuffer(
            &CursorPlanarBuffer {
                dumb: *raw_dumb,
                width,
                height,
            },
            FbCmd2Flags::empty(),
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
    width: u32,
    height: u32,
}

impl PlanarBuffer for CursorPlanarBuffer {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn format(&self) -> DrmFourcc {
        DrmFourcc::Argb8888
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
}

#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CursorBufferRecord {
    Create {
        device_fd: i32,
        handle: u32,
    },
    Map {
        device_fd: i32,
        handle: u32,
    },
    AddFb {
        device_fd: i32,
        handle: u32,
        fb: u32,
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
        width: u32,
        height: u32,
    ) -> io::Result<CursorDumbBuffer> {
        let fd = device.as_fd().as_raw_fd();
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
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Create {
                device_fd: fd,
                handle,
            });
        Ok(CursorDumbBuffer::recording(device, handle, pitch, len))
    }

    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping> {
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
        CursorMapping::recording(dumb.len)
    }

    fn add_argb_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        _width: u32,
        _height: u32,
    ) -> io::Result<framebuffer::Handle> {
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
            });
        Ok(fb)
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
        let mut dumb = io.create_dumb(&device, width, height)?;
        let stride = dumb.pitch();
        let mapping = match io.map_dumb(&device, &mut dumb) {
            Ok(mapping) => mapping,
            Err(error) => {
                let _ = io.destroy_dumb(&device, dumb);
                return Err(error);
            }
        };
        let fb = match io.add_argb_framebuffer(&device, &dumb, width, height) {
            Ok(fb) => fb,
            Err(error) => {
                io.unmap_dumb(&device, &dumb, mapping);
                let _ = io.destroy_dumb(&device, dumb);
                return Err(error);
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
        };
        // The newly allocated ARGB image starts transparent.
        buffer.clear();
        Ok(buffer)
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

    pub(crate) fn clear(&mut self) {
        // SAFETY: mapping remains live and covers `len` writable bytes.
        unsafe { std::ptr::write_bytes(self.ptr().as_ptr(), 0, self.len) };
    }
}

impl Drop for CursorBuffer {
    fn drop(&mut self) {
        if let Some(fb) = self.fb.take()
            && let Err(error) = self.io.remove_framebuffer(&self.device, fb)
        {
            log::warn!("cursor RMFB failed: {error}");
        }
        if let (Some(dumb), Some(mapping)) = (self.dumb.as_ref(), self.mapping.take()) {
            self.io.unmap_dumb(&self.device, dumb, mapping);
        }
        if let Some(dumb) = self.dumb.take()
            && let Err(error) = self.io.destroy_dumb(&self.device, dumb)
        {
            log::warn!("cursor dumb-buffer destroy failed: {error}");
        }
    }
}
