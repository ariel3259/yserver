//! Full-capacity cursor sprite framebuffers staged before A5 switches the
//! atomic binding path over from the legacy cursor buffer.

use std::{io, rc::Rc};

use super::ops::{CursorBuffer, CursorBufferIo};
use crate::drm::Device;
use yserver_render::kms::render::scene::CursorUploadOutcome;

/// A staged slot/version pair for the atomic binding phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SpriteBufferLease {
    slot: usize,
    version: u64,
}

impl SpriteBufferLease {
    pub(crate) fn version(self) -> u64 {
        self.version
    }
}

struct SpriteBuffer {
    buffer: CursorBuffer,
    version: Option<u64>,
    users: usize,
    prepared_users: usize,
}

/// A per-DRM-device pool. All framebuffer allocations use driver cursor-cap
/// dimensions; image bytes are top-left, with the remaining mapped rows and
/// columns cleared to transparent before every stage.
pub(crate) struct SpritePool {
    width: u32,
    height: u32,
    buffers: Vec<SpriteBuffer>,
    max_buffers: usize,
    wanted: Option<WantedSprite>,
    candidate: Option<SpriteBufferLease>,
}

struct WantedSprite {
    version: u64,
    width: u32,
    height: u32,
    bgra_bytes: Vec<u8>,
}

impl SpritePool {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffers: Vec::new(),
            max_buffers: 2,
            wanted: None,
            candidate: None,
        }
    }

    pub(crate) fn set_active_crtc_count(&mut self, count: usize) {
        self.max_buffers = count.saturating_add(1).max(2);
    }

    pub(crate) fn disarm(&mut self) {
        for buffer in std::mem::take(&mut self.buffers) {
            std::mem::forget(buffer);
        }
        self.candidate = None;
        self.wanted = None;
    }

    pub(crate) fn prepare(
        &mut self,
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        version: u64,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<CursorUploadOutcome> {
        validate_image(
            self.width,
            self.height,
            image_width,
            image_height,
            bgra_bytes,
        )?;
        self.wanted = Some(WantedSprite {
            version,
            width: image_width,
            height: image_height,
            bgra_bytes: bgra_bytes.to_vec(),
        });
        if self.buffers.is_empty() {
            let first = match CursorBuffer::allocate(
                Rc::clone(&device),
                Rc::clone(&io),
                self.width,
                self.height,
            ) {
                Ok(buffer) => buffer,
                Err(error) => {
                    return allocation_outcome(error);
                }
            };
            let second = match CursorBuffer::allocate(
                Rc::clone(&device),
                Rc::clone(&io),
                self.width,
                self.height,
            ) {
                Ok(buffer) => buffer,
                Err(error) => {
                    if let Err(cleanup_error) = first.release() {
                        return Err(io::Error::other(format!(
                            "{error}; releasing first cursor sprite allocation failed: {cleanup_error}"
                        )));
                    }
                    return allocation_outcome(error);
                }
            };
            self.buffers = vec![
                SpriteBuffer {
                    buffer: first,
                    version: None,
                    users: 0,
                    prepared_users: 0,
                },
                SpriteBuffer {
                    buffer: second,
                    version: None,
                    users: 0,
                    prepared_users: 0,
                },
            ];
        }

        let wanted_version = self
            .wanted
            .as_ref()
            .expect("wanted sprite is retained")
            .version;
        if self
            .candidate
            .is_some_and(|lease| lease.version == wanted_version)
        {
            return Ok(CursorUploadOutcome::Ready);
        }
        let slot = self
            .buffers
            .iter()
            .enumerate()
            .find(|(_, buffer)| buffer.users == 0 && buffer.prepared_users == 0)
            .map(|(slot, _)| slot);
        let slot = if let Some(slot) = slot {
            slot
        } else if self.buffers.len() < self.max_buffers {
            let buffer = match CursorBuffer::allocate(device, io, self.width, self.height) {
                Ok(buffer) => buffer,
                Err(error) => return allocation_outcome(error),
            };
            self.buffers.push(SpriteBuffer {
                buffer,
                version: None,
                users: 0,
                prepared_users: 0,
            });
            self.buffers.len() - 1
        } else {
            return Ok(CursorUploadOutcome::Deferred);
        };
        let wanted = self.wanted.as_ref().expect("wanted sprite is retained");
        match self.buffers[slot]
            .buffer
            .write_image(wanted.width, wanted.height, &wanted.bgra_bytes)
        {
            Ok(()) => {
                self.buffers[slot].version = Some(version);
                self.candidate = Some(SpriteBufferLease { slot, version });
                Ok(CursorUploadOutcome::Ready)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Ok(CursorUploadOutcome::Deferred)
            }
            Err(error) => Err(error),
        }
    }

    #[allow(
        dead_code,
        reason = "A5 consumes the staged framebuffer lease for atomic binding"
    )]
    pub(crate) fn candidate_lease(&self) -> Option<SpriteBufferLease> {
        self.candidate
    }

    #[allow(
        dead_code,
        reason = "A5 uses the lease to add FB_ID to an atomic cursor commit"
    )]
    pub(crate) fn framebuffer_for_lease(&self, lease: SpriteBufferLease) -> Option<u32> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| buffer.version == Some(lease.version))
            .map(|buffer| u32::from(buffer.buffer.fb_id()))
    }

    pub(crate) fn reserve(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.version != Some(lease.version) {
            return false;
        }
        buffer.prepared_users = buffer.prepared_users.saturating_add(1);
        true
    }

    pub(crate) fn accept_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.version != Some(lease.version) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        buffer.users = buffer.users.saturating_add(1);
        true
    }

    pub(crate) fn reject_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.version != Some(lease.version) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        true
    }

    pub(crate) fn release_user(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.version != Some(lease.version) || buffer.users == 0 {
            return false;
        }
        buffer.users -= 1;
        true
    }

    #[allow(
        dead_code,
        reason = "A5 cancels candidates rejected before atomic submission"
    )]
    pub(crate) fn cancel_candidate(&mut self, lease: SpriteBufferLease) -> bool {
        if self.candidate != Some(lease) {
            return false;
        }
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.users != 0 || buffer.prepared_users != 0 {
            return false;
        }
        buffer.version = None;
        self.candidate = None;
        true
    }

    #[cfg(test)]
    pub(crate) fn user_count(&self, lease: SpriteBufferLease) -> Option<usize> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| buffer.version == Some(lease.version))
            .map(|buffer| buffer.users)
    }

    #[cfg(test)]
    pub(crate) fn all_users_zero(&self) -> bool {
        self.buffers
            .iter()
            .all(|buffer| buffer.users == 0 && buffer.prepared_users == 0)
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "A4 Backend tests assert wanted-record retry after allocation failure"
    )]
    pub(crate) fn wanted_version(&self) -> Option<u64> {
        self.wanted.as_ref().map(|wanted| wanted.version)
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "A4 Backend tests distinguish completed from deferred staging"
    )]
    pub(crate) fn candidate_version(&self) -> Option<u64> {
        self.candidate.map(|lease| lease.version)
    }
}

fn allocation_outcome(error: io::Error) -> io::Result<CursorUploadOutcome> {
    if error.kind() == io::ErrorKind::WouldBlock {
        log::debug!("cursor sprite pool allocation deferred by pool pressure: {error}");
        Ok(CursorUploadOutcome::Deferred)
    } else {
        Err(error)
    }
}

fn validate_image(
    capacity_width: u32,
    capacity_height: u32,
    image_width: u32,
    image_height: u32,
    bgra_bytes: &[u8],
) -> io::Result<()> {
    if image_width == 0
        || image_height == 0
        || image_width > capacity_width
        || image_height > capacity_height
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor exceeds hardware plane size",
        ));
    }
    let expected = usize::try_from(image_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .and_then(|row| {
            usize::try_from(image_height)
                .ok()
                .and_then(|height| row.checked_mul(height))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor byte size overflow"))?;
    if bgra_bytes.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor bytes must equal width*height*4",
        ));
    }
    Ok(())
}
