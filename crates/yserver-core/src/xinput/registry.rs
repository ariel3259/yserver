//! Source-owned XI device facets.
//!
//! The registry keeps process-local input source identity separate from the
//! reusable eight-bit XI device IDs.  A source record remains present even
//! when there is not enough XI ID capacity to publish all of its facets.

use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

use crate::core_loop::DeviceInfo;

use super::{
    DEVICEID_MASTER_KEYBOARD, DEVICEID_MASTER_POINTER, DEVICEID_SLAVE_KEYBOARD,
    DEVICEID_SLAVE_POINTER, XiDevice,
};

const FIRST_PHYSICAL_DEVICE_ID: u16 = 6;
const LAST_PHYSICAL_DEVICE_ID: u16 = 127;

pub const NAME_XTEST_POINTER: &str = "Virtual core XTEST pointer";
pub const NAME_XTEST_KEYBOARD: &str = "Virtual core XTEST keyboard";

/// Runtime identity for one backend input source attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputSourceId(pub u64);

/// Capabilities reported for one backend input source.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct InputCapabilities {
    pub keyboard: bool,
    pub pointer: bool,
    pub touch: bool,
}

/// XI facet kinds supported by the keyboard/pointer registry stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XiFacetKind {
    Keyboard,
    PointerTouch,
}

#[derive(Debug)]
struct SourceRecord {
    info: DeviceInfo,
    facets: HashMap<XiFacetKind, u16>,
}

/// The single owner of live XI devices and input-source metadata.
#[derive(Debug)]
pub struct XiRegistry {
    devices: Vec<XiDevice>,
    sources: HashMap<InputSourceId, SourceRecord>,
}

impl XiRegistry {
    /// Create masters 2/3 and the virtual XTEST slaves 4/5.
    ///
    /// This constructor intentionally has no atom-table dependency; server
    /// initialization seeds the XTEST marker after interning its atom.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: vec![
                XiDevice::new(DEVICEID_MASTER_POINTER, super::NAME_MASTER_POINTER),
                XiDevice::new(DEVICEID_MASTER_KEYBOARD, super::NAME_MASTER_KEYBOARD),
                XiDevice::new(DEVICEID_SLAVE_POINTER, NAME_XTEST_POINTER),
                XiDevice::new(DEVICEID_SLAVE_KEYBOARD, NAME_XTEST_KEYBOARD),
            ],
            sources: HashMap::new(),
        }
    }

    /// Register all supported keyboard/pointer facets for a source.
    ///
    /// IDs are selected from the lowest free value in 6..=127. Required IDs
    /// are reserved before any device is inserted, so an insufficient range
    /// never publishes only part of a mixed source. Its metadata is still
    /// retained for core delivery and later lookup.
    pub fn register(&mut self, info: &DeviceInfo) -> Vec<u16> {
        if let Some(ids) = self
            .sources
            .get(&info.source_id)
            .map(|record| sorted_facet_ids(&record.facets))
        {
            if !ids.is_empty() {
                self.sources
                    .get_mut(&info.source_id)
                    .expect("source record observed above")
                    .info = info.clone();
                return ids;
            }
            // A prior registration may have retained metadata without facets
            // because capacity was exhausted. Retry when this source is
            // observed again after another source has been removed.
            self.sources.remove(&info.source_id);
        }

        let required = facets_for(info.capabilities);
        let free_ids: Vec<u16> = (FIRST_PHYSICAL_DEVICE_ID..=LAST_PHYSICAL_DEVICE_ID)
            .filter(|id| !self.devices.iter().any(|device| device.id == *id))
            .take(required.len())
            .collect();

        let mut facets = HashMap::new();
        if free_ids.len() == required.len() {
            for (facet, id) in required.iter().copied().zip(free_ids.iter().copied()) {
                self.devices.push(XiDevice::physical(id, info, facet));
                facets.insert(facet, id);
            }
            self.devices.sort_by_key(|device| device.id);
        } else if !required.is_empty() {
            log::warn!(
                "xinput: no XI IDs available for source {:?} (needs {}, available {})",
                info.source_id,
                required.len(),
                free_ids.len(),
            );
        }

        let result = sorted_facet_ids(&facets);
        self.sources.insert(
            info.source_id,
            SourceRecord {
                info: info.clone(),
                facets,
            },
        );
        result
    }

    /// Remove a source and all of its currently published facets.
    ///
    /// Returned IDs can be used by later lifecycle stages to publish removal
    /// before those IDs are allocated to a new source.
    pub fn remove(&mut self, source_id: InputSourceId) -> Vec<u16> {
        let Some(record) = self.sources.remove(&source_id) else {
            return Vec::new();
        };
        let mut removed = sorted_facet_ids(&record.facets);
        self.devices
            .retain(|device| device.source_id != Some(source_id));
        removed.sort_unstable();
        removed
    }

    #[must_use]
    pub fn device(&self, device_id: u16) -> Option<&XiDevice> {
        self.devices.iter().find(|device| device.id == device_id)
    }

    pub fn device_mut(&mut self, device_id: u16) -> Option<&mut XiDevice> {
        self.devices
            .iter_mut()
            .find(|device| device.id == device_id)
    }

    #[must_use]
    pub fn facet(&self, source_id: InputSourceId, kind: XiFacetKind) -> Option<u16> {
        self.sources
            .get(&source_id)
            .and_then(|record| record.facets.get(&kind).copied())
    }

    #[must_use]
    pub fn source(&self, source_id: InputSourceId) -> Option<&DeviceInfo> {
        self.sources.get(&source_id).map(|record| &record.info)
    }

    pub fn source_mut(&mut self, source_id: InputSourceId) -> Option<&mut DeviceInfo> {
        self.sources
            .get_mut(&source_id)
            .map(|record| &mut record.info)
    }

    #[must_use]
    pub fn devices(&self) -> &[XiDevice] {
        &self.devices
    }
}

impl Default for XiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Temporary compatibility for existing property/query helpers while their
/// callers migrate from a raw slice to explicit registry lookups.
impl Deref for XiRegistry {
    type Target = [XiDevice];

    fn deref(&self) -> &Self::Target {
        &self.devices
    }
}

impl DerefMut for XiRegistry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.devices
    }
}

fn facets_for(capabilities: InputCapabilities) -> Vec<XiFacetKind> {
    let mut facets = Vec::with_capacity(2);
    if capabilities.keyboard {
        facets.push(XiFacetKind::Keyboard);
    }
    // Touch-only facet publication is added by the touch implementation.
    if capabilities.pointer {
        facets.push(XiFacetKind::PointerTouch);
    }
    facets
}

fn sorted_facet_ids(facets: &HashMap<XiFacetKind, u16>) -> Vec<u16> {
    let mut ids: Vec<u16> = facets.values().copied().collect();
    ids.sort_unstable();
    ids
}
