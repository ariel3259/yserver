//! XI2 hierarchy notifications for physical source lifecycle changes.

use std::collections::HashSet;

use yserver_protocol::x11::{self, ClientId};

use crate::{core_loop::fanout::fanout_event_to_clients, server::ServerState};

use super::{XI2_HIERARCHY_CHANGED_MASK, XiDevice, XiDeviceRole};

const XI2_MAJOR_OPCODE: u8 = 137;
const XI_HIERARCHY_CHANGED_MASK_WIDE: u64 = XI2_HIERARCHY_CHANGED_MASK as u64;

const XI_SLAVE_ADDED: u32 = 1 << 2;
const XI_SLAVE_REMOVED: u32 = 1 << 3;
const XI_DEVICE_ENABLED: u32 = 1 << 6;
const XI_DEVICE_DISABLED: u32 = 1 << 7;

/// One Xorg-compatible hierarchy transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XiHierarchyStep {
    SlaveAdded,
    DeviceEnabled,
    DeviceDisabled,
    SlaveRemoved,
}

impl XiHierarchyStep {
    const fn flag(self) -> u32 {
        match self {
            Self::SlaveAdded => XI_SLAVE_ADDED,
            Self::DeviceEnabled => XI_DEVICE_ENABLED,
            Self::DeviceDisabled => XI_DEVICE_DISABLED,
            Self::SlaveRemoved => XI_SLAVE_REMOVED,
        }
    }
}

/// Publish one XI2 hierarchy step to clients selecting it on XIAllDevices.
///
/// The event contains every currently live device, with flags set only on
/// `changed_ids`. A removal event additionally appends the removed facet
/// descriptors with Xorg's removed-device wire fields (`enabled = false`,
/// `use = 0`, and attachment zero). Callers must remove facets before the
/// `SlaveRemoved` step and retain their snapshots until this function queues
/// the notification.
pub fn emit_xi_hierarchy_changed(
    state: &mut ServerState,
    change: XiHierarchyStep,
    changed_ids: &[u16],
    removed: &[XiDevice],
) -> Vec<ClientId> {
    if changed_ids.is_empty() {
        return Vec::new();
    }

    let changed: HashSet<u16> = changed_ids.iter().copied().collect();
    let step_flag = change.flag();
    let mut infos = Vec::with_capacity(
        state.xi_devices.devices().len()
            + if change == XiHierarchyStep::SlaveRemoved {
                removed.len()
            } else {
                0
            },
    );

    for device in state.xi_devices.devices() {
        infos.push(x11::XiHierarchyInfo {
            device_id: device.id,
            attachment: state.xi_devices.attachment(device.id).unwrap_or(0),
            use_: hierarchy_use(state.xi_devices.role(device.id)),
            enabled: device.enabled,
            flags: if changed.contains(&device.id) {
                step_flag
            } else {
                0
            },
        });
    }

    if change == XiHierarchyStep::SlaveRemoved {
        let mut removed_in_id_order: Vec<&XiDevice> = removed
            .iter()
            .filter(|device| changed.contains(&device.id))
            .collect();
        removed_in_id_order.sort_unstable_by_key(|device| device.id);
        for device in removed_in_id_order {
            infos.push(x11::XiHierarchyInfo {
                device_id: device.id,
                attachment: 0,
                use_: 0,
                enabled: false,
                flags: step_flag,
            });
        }
    }

    let targets: Vec<ClientId> = state
        .clients
        .iter()
        .filter_map(|(id, client)| {
            client
                .xi2_masks
                .iter()
                .any(|((_, device_id), mask)| {
                    *device_id == 0 && (mask & XI_HIERARCHY_CHANGED_MASK_WIDE) != 0
                })
                .then_some(ClientId(*id))
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }

    let time = state.timestamp_now();
    fanout_event_to_clients(state, &targets, |buf, sequence, byte_order| {
        x11::encode_xi2_hierarchy_changed_event(
            buf,
            byte_order,
            sequence,
            XI2_MAJOR_OPCODE,
            time,
            &infos,
        );
    })
}

fn hierarchy_use(role: Option<XiDeviceRole>) -> u8 {
    match role {
        Some(XiDeviceRole::MasterPointer) => 1,  // XIMasterPointer
        Some(XiDeviceRole::MasterKeyboard) => 2, // XIMasterKeyboard
        Some(XiDeviceRole::SlavePointer) => 3,   // XISlavePointer
        Some(XiDeviceRole::SlaveKeyboard) => 4,  // XISlaveKeyboard
        None => 0,
    }
}
