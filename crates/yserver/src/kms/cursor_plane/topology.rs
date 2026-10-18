//! DRM cursor-plane discovery, qualification, and exclusive CRTC matching.

use std::collections::{HashMap, HashSet};

use drm::control::{Device as ControlDevice, PlaneType, crtc, plane, property};

use crate::drm::Device;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DiscoveredCursorPlane {
    pub(super) handle: plane::Handle,
    pub(super) crtcs: HashSet<crtc::Handle>,
    pub(super) properties: HashMap<String, property::Handle>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CursorPlaneDiscovery {
    pub(super) crtcs: Vec<crtc::Handle>,
    pub(super) planes: Vec<DiscoveredCursorPlane>,
    pub(super) caps: Option<crate::drm::CursorDeviceCaps>,
}

pub(super) trait CursorPlaneDiscoveryIo {
    fn discover(&self, device: &Device) -> std::io::Result<CursorPlaneDiscovery>;
}

pub(super) struct DrmCursorPlaneDiscovery;

impl CursorPlaneDiscoveryIo for DrmCursorPlaneDiscovery {
    fn discover(&self, device: &Device) -> std::io::Result<CursorPlaneDiscovery> {
        discover_cursor_planes(device)
    }
}

fn discover_cursor_planes(device: &Device) -> std::io::Result<CursorPlaneDiscovery> {
    let resources = device.resource_handles()?;
    let mut cursor_planes = Vec::new();
    for handle in device.plane_handles()? {
        let info = device.get_plane(handle)?;
        let props = device.get_properties(handle)?;
        let map = props.as_hashmap(device)?;
        let Some(type_info) = map.get("type") else {
            continue;
        };
        let raw = props
            .iter()
            .find(|(property, _)| **property == type_info.handle())
            .map(|(_, value)| *value)
            .unwrap_or(0);
        if raw != PlaneType::Cursor as u64 {
            continue;
        }
        cursor_planes.push(DiscoveredCursorPlane {
            handle,
            crtcs: resources
                .filter_crtcs(info.possible_crtcs())
                .into_iter()
                .collect(),
            properties: map
                .into_iter()
                .map(|(name, property)| (name, property.handle()))
                .collect(),
        });
    }
    Ok(CursorPlaneDiscovery {
        crtcs: resources.crtcs().to_vec(),
        planes: cursor_planes,
        caps: device.cursor_caps().cloned(),
    })
}

/// Atomic state needed to bind, position, and size one assigned cursor plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorPlaneProperties {
    pub(crate) plane: plane::Handle,
    pub(crate) fb_id: property::Handle,
    pub(crate) crtc_id: property::Handle,
    pub(crate) crtc_x: property::Handle,
    pub(crate) crtc_y: property::Handle,
    pub(crate) crtc_w: property::Handle,
    pub(crate) crtc_h: property::Handle,
    pub(crate) src_x: property::Handle,
    pub(crate) src_y: property::Handle,
    pub(crate) src_w: property::Handle,
    pub(crate) src_h: property::Handle,
    pub(crate) hotspot_x: Option<property::Handle>,
    pub(crate) hotspot_y: Option<property::Handle>,
}

pub(super) type CursorPlaneTopology = HashMap<crtc::Handle, CursorPlaneProperties>;

impl DiscoveredCursorPlane {
    fn cursor_properties(
        &self,
        caps: Option<&crate::drm::CursorDeviceCaps>,
    ) -> Option<CursorPlaneProperties> {
        let caps = caps?;
        let get = |name: &str| self.properties.get(name).copied();
        let hotspot_x = get("HOTSPOT_X");
        let hotspot_y = get("HOTSPOT_Y");
        if caps.is_virtualized_cursor_driver()
            && (!caps.cursor_plane_hotspot_cap_accepted()
                || hotspot_x.is_none()
                || hotspot_y.is_none())
        {
            return None;
        }
        Some(CursorPlaneProperties {
            plane: self.handle,
            fb_id: get("FB_ID")?,
            crtc_id: get("CRTC_ID")?,
            crtc_x: get("CRTC_X")?,
            crtc_y: get("CRTC_Y")?,
            crtc_w: get("CRTC_W")?,
            crtc_h: get("CRTC_H")?,
            src_x: get("SRC_X")?,
            src_y: get("SRC_Y")?,
            src_w: get("SRC_W")?,
            src_h: get("SRC_H")?,
            hotspot_x,
            hotspot_y,
        })
    }
}

/// Driver names whose cursor-plane visibility depends on client hotspot support.
pub(crate) fn is_virtualized_cursor_driver(driver_name: &str) -> bool {
    matches!(driver_name, "virtio_gpu" | "vmwgfx" | "qxl" | "vboxvideo")
}

/// Assign each CRTC at most one cursor plane and each plane to at most one
/// CRTC, like wlroots: a plane whose `possible_crtcs` is exactly one CRTC goes
/// to that CRTC (the kernel's legacy cursor plane on per-CRTC-cursor drivers),
/// then the remaining planes are matched to the remaining CRTCs with augmenting
/// paths (a maximum matching, so greedy order cannot strand a coverable CRTC).
fn assign_cursor_planes(
    crtcs: &[crtc::Handle],
    planes: &[DiscoveredCursorPlane],
) -> HashMap<crtc::Handle, usize> {
    let mut assignment = HashMap::new();
    let mut exclusive = vec![false; planes.len()];
    for (index, plane) in planes.iter().enumerate() {
        if plane.crtcs.len() == 1
            && let Some(crtc) = plane.crtcs.iter().next()
            && crtcs.contains(crtc)
            && !assignment.contains_key(crtc)
        {
            assignment.insert(*crtc, index);
            exclusive[index] = true;
        }
    }
    let mut owner: Vec<Option<usize>> = vec![None; planes.len()];
    for (crtc_index, crtc) in crtcs.iter().enumerate() {
        if !assignment.contains_key(crtc) {
            let mut visited = vec![false; planes.len()];
            augment_cursor_matching(
                crtc_index,
                crtcs,
                planes,
                &exclusive,
                &mut owner,
                &mut visited,
            );
        }
    }
    for (index, crtc_index) in owner.iter().enumerate() {
        if let Some(crtc_index) = crtc_index {
            assignment.insert(crtcs[*crtc_index], index);
        }
    }
    assignment
}

/// Kuhn's augmenting-path step: give `crtcs[crtc_index]` a plane, moving
/// earlier non-exclusive owners to other planes when that frees one.
fn augment_cursor_matching(
    crtc_index: usize,
    crtcs: &[crtc::Handle],
    planes: &[DiscoveredCursorPlane],
    exclusive: &[bool],
    owner: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    for index in 0..planes.len() {
        if exclusive[index] || visited[index] || !planes[index].crtcs.contains(&crtcs[crtc_index]) {
            continue;
        }
        visited[index] = true;
        let free = match owner[index] {
            None => true,
            Some(other) => augment_cursor_matching(other, crtcs, planes, exclusive, owner, visited),
        };
        if free {
            owner[index] = Some(crtc_index);
            return true;
        }
    }
    false
}

/// Maximum matching of eligible planes to CRTCs. Unqualified planes are
/// removed before matching so they cannot displace a usable candidate.
pub(super) fn cursor_plane_topology(
    crtcs: &[crtc::Handle],
    planes: &[DiscoveredCursorPlane],
    caps: Option<&crate::drm::CursorDeviceCaps>,
) -> CursorPlaneTopology {
    let eligible: Vec<_> = planes
        .iter()
        .filter(|plane| plane.cursor_properties(caps).is_some())
        .cloned()
        .collect();
    assign_cursor_planes(crtcs, &eligible)
        .into_iter()
        .filter_map(|(crtc, index)| Some((crtc, eligible[index].cursor_properties(caps)?)))
        .collect()
}

#[cfg(test)]
pub(crate) struct CursorPlaneTestPlane {
    pub(crate) handle: u32,
    pub(crate) possible_crtcs: Vec<u32>,
    pub(crate) property_names: Vec<String>,
}

#[cfg(test)]
pub(crate) struct CursorPlaneTestSnapshot {
    pub(crate) crtcs: Vec<u32>,
    pub(crate) planes: Vec<CursorPlaneTestPlane>,
    pub(crate) driver_name: String,
    pub(crate) hotspot_cap_accepted: bool,
}

#[cfg(test)]
impl From<CursorPlaneTestSnapshot> for CursorPlaneDiscovery {
    fn from(snapshot: CursorPlaneTestSnapshot) -> Self {
        Self {
            crtcs: snapshot
                .crtcs
                .into_iter()
                .map(|raw| ::drm::control::from_u32(raw).expect("test CRTC handle"))
                .collect(),
            planes: snapshot
                .planes
                .into_iter()
                .map(|plane| DiscoveredCursorPlane {
                    handle: ::drm::control::from_u32(plane.handle).expect("test plane handle"),
                    crtcs: plane
                        .possible_crtcs
                        .into_iter()
                        .map(|raw| ::drm::control::from_u32(raw).expect("test CRTC handle"))
                        .collect(),
                    properties: plane
                        .property_names
                        .into_iter()
                        .enumerate()
                        .map(|(index, name)| {
                            (
                                name,
                                ::drm::control::from_u32(0x9000 + index as u32)
                                    .expect("test property handle"),
                            )
                        })
                        .collect(),
                })
                .collect(),
            caps: Some(crate::drm::CursorDeviceCaps::new(
                snapshot.hotspot_cap_accepted,
                snapshot.driver_name,
            )),
        }
    }
}
