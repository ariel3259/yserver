//! Full-capacity cursor sprite framebuffers retained for accepted atomic
//! cursor bindings.

use std::{io, rc::Rc};

use super::ops::{CursorBuffer, CursorBufferIo};
use crate::drm::Device;
use yserver_render::kms::render::scene::CursorUploadOutcome;

/// A staged image/hotspot identity for the atomic binding phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SpriteBufferLease {
    slot: usize,
    version: u64,
    width: u32,
    height: u32,
    hot_x: u16,
    hot_y: u16,
}

impl SpriteBufferLease {
    pub(crate) fn version(self) -> u64 {
        self.version
    }

    pub(crate) fn matches(self, version: u64, hot_x: u16, hot_y: u16) -> bool {
        self.version == version && self.hot_x == hot_x && self.hot_y == hot_y
    }

    pub(crate) fn sprite_key(self) -> Option<(u16, u16, u16, u16)> {
        Some((
            u16::try_from(self.width).ok()?,
            u16::try_from(self.height).ok()?,
            self.hot_x,
            self.hot_y,
        ))
    }
}

struct SpriteBuffer {
    buffer: CursorBuffer,
    version: Option<u64>,
    hotspot: Option<(u16, u16)>,
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
    quarantined: bool,
}

struct WantedSprite {
    version: u64,
    hot_x: u16,
    hot_y: u16,
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
            quarantined: false,
        }
    }

    pub(crate) fn set_active_crtc_count(&mut self, count: usize) {
        self.max_buffers = count.saturating_add(1).max(2);
    }

    pub(crate) fn full_with_live_users(&self) -> bool {
        self.buffers.len() >= self.max_buffers
            && self
                .buffers
                .iter()
                .all(|buffer| buffer.users != 0 || buffer.prepared_users != 0)
    }

    pub(crate) fn has_available_buffer(&self) -> bool {
        self.buffers
            .iter()
            .any(|buffer| buffer.users == 0 && buffer.prepared_users == 0)
    }

    pub(crate) fn disarm(&mut self) {
        self.quarantined = true;
    }

    /// Release the destruction quarantine after a blocking all-off and its
    /// old-event drain have proved that no cursor framebuffer is in use.
    pub(crate) fn recover_after_all_off(&mut self) {
        self.quarantined = false;
    }

    pub(crate) fn prepare(
        &mut self,
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        version: u64,
        hot_x: u16,
        hot_y: u16,
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
            hot_x,
            hot_y,
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
                    hotspot: None,
                    users: 0,
                    prepared_users: 0,
                },
                SpriteBuffer {
                    buffer: second,
                    version: None,
                    hotspot: None,
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
            .is_some_and(|lease| lease.matches(wanted_version, hot_x, hot_y))
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
                hotspot: None,
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
                self.buffers[slot].hotspot = Some((wanted.hot_x, wanted.hot_y));
                self.candidate = Some(SpriteBufferLease {
                    slot,
                    version,
                    width: wanted.width,
                    height: wanted.height,
                    hot_x: wanted.hot_x,
                    hot_y: wanted.hot_y,
                });
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
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| u32::from(buffer.buffer.fb_id()))
    }

    pub(crate) fn dump_sprite_to_ppm(
        &self,
        lease: SpriteBufferLease,
        path: &str,
    ) -> io::Result<()> {
        let buffer = self
            .buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .ok_or_else(|| io::Error::other("applied cursor sprite lease is no longer valid"))?;
        buffer.buffer.dump_to_ppm(path)
    }

    pub(crate) fn reserve(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) {
            return false;
        }
        buffer.prepared_users = buffer.prepared_users.saturating_add(1);
        true
    }

    pub(crate) fn accept_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        buffer.users = buffer.users.saturating_add(1);
        true
    }

    pub(crate) fn can_accept_reserved(&self, lease: SpriteBufferLease) -> bool {
        self.buffers
            .get(lease.slot)
            .is_some_and(|buffer| lease_matches(buffer, lease) && buffer.prepared_users > 0)
    }

    pub(crate) fn reject_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        true
    }

    pub(crate) fn release_user(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.users == 0 {
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
        buffer.hotspot = None;
        self.candidate = None;
        true
    }

    #[cfg(test)]
    pub(crate) fn user_count(&self, lease: SpriteBufferLease) -> Option<usize> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| buffer.users)
    }

    #[cfg(test)]
    pub(crate) fn all_users_zero(&self) -> bool {
        self.buffers
            .iter()
            .all(|buffer| buffer.users == 0 && buffer.prepared_users == 0)
    }

    #[cfg(test)]
    pub(crate) fn prepared_user_count(&self) -> usize {
        self.buffers
            .iter()
            .map(|buffer| buffer.prepared_users)
            .sum()
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

    #[cfg(test)]
    pub(crate) fn for_tests_sprite_bytes(&self, lease: SpriteBufferLease) -> Option<Vec<u8>> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| buffer.buffer.for_tests_bytes())
    }
}

impl Drop for SpritePool {
    fn drop(&mut self) {
        if self.quarantined {
            for buffer in std::mem::take(&mut self.buffers) {
                std::mem::forget(buffer);
            }
        }
    }
}

fn lease_matches(buffer: &SpriteBuffer, lease: SpriteBufferLease) -> bool {
    buffer.version == Some(lease.version) && buffer.hotspot == Some((lease.hot_x, lease.hot_y))
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
