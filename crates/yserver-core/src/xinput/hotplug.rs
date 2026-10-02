//! XI2 hierarchy and device-change notifications for physical sources.

use std::collections::HashSet;

use yserver_protocol::x11::{self, ClientByteOrder, ClientId};

use crate::{core_loop::fanout::fanout_event_to_clients, server::ServerState};

use super::{XI2_DEVICE_CHANGED_MASK, XI2_HIERARCHY_CHANGED_MASK, XiDevice, XiDeviceRole};

const XI2_MAJOR_OPCODE: u8 = 137;
const XI_HIERARCHY_CHANGED_MASK_WIDE: u64 = XI2_HIERARCHY_CHANGED_MASK as u64;
const XI_DEVICE_CHANGED_MASK_WIDE: u64 = XI2_DEVICE_CHANGED_MASK as u64;
const XI_SLAVE_SWITCH: u8 = 1;
const XI_DEVICE_CHANGE: u8 = 2;

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

/// XI2.h:107-108 (`XISlaveSwitch = 1`, `XIDeviceChange = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum XiDeviceChangeReason {
    DeviceChange = XI_DEVICE_CHANGE,
    SlaveSwitch = XI_SLAVE_SWITCH,
}

#[derive(Clone, Copy)]
enum DeviceClassSnapshot {
    Pointer(crate::xinput::query::XiQueryClassData),
    Keyboard,
}

fn snapshot_device_classes(state: &mut ServerState, sourceid: u16) -> Option<DeviceClassSnapshot> {
    match state.xi_devices.role(sourceid)? {
        XiDeviceRole::MasterPointer | XiDeviceRole::SlavePointer => {
            Some(DeviceClassSnapshot::Pointer(
                crate::core_loop::fanout::pointer_class_data_for_device(state, sourceid),
            ))
        }
        XiDeviceRole::MasterKeyboard | XiDeviceRole::SlaveKeyboard => {
            Some(DeviceClassSnapshot::Keyboard)
        }
    }
}

fn encode_device_classes(
    classes: DeviceClassSnapshot,
    sourceid: u16,
    byte_order: ClientByteOrder,
) -> (Vec<u8>, u16) {
    match classes {
        DeviceClassSnapshot::Pointer(data) => {
            crate::xinput::query::build_pointer_classes(byte_order, sourceid, data)
        }
        DeviceClassSnapshot::Keyboard => {
            crate::xinput::query::build_key_classes(byte_order, sourceid)
        }
    }
}

/// Build one DeviceChanged class block using the same encoders as
/// XIQueryDevice. The caller chooses the event device and reason.
pub(crate) fn device_changed_class_block(
    state: &mut ServerState,
    sourceid: u16,
    byte_order: ClientByteOrder,
) -> Option<(Vec<u8>, u16)> {
    let snapshot = snapshot_device_classes(state, sourceid)?;
    Some(encode_device_classes(snapshot, sourceid, byte_order))
}

/// Emit an XI2 `XI_DeviceChanged` event on `id`, using the classes and
/// source identity of `sourceid`. Selection is on the root window for the
/// exact event device, XIAllDevices, or XIAllMasterDevices when `id` is a
/// master. Returns clients whose output buffers overflowed.
pub fn emit_xi2_device_changed(
    state: &mut ServerState,
    id: u16,
    reason: XiDeviceChangeReason,
    sourceid: u16,
) -> Vec<ClientId> {
    let Some(id_role) = state.xi_devices.role(id) else {
        return Vec::new();
    };
    let Some(source_classes) = snapshot_device_classes(state, sourceid) else {
        return Vec::new();
    };
    let is_master = matches!(
        id_role,
        XiDeviceRole::MasterPointer | XiDeviceRole::MasterKeyboard
    );
    let targets: Vec<ClientId> = state
        .clients
        .iter()
        .filter_map(|(client_id, client)| {
            let selected = client
                .xi2_masks
                .iter()
                .any(|(&(window, device_id), &mask)| {
                    window == crate::resources::ROOT_WINDOW
                        && (device_id == id || device_id == 0 || (is_master && device_id == 1))
                        && (mask & XI_DEVICE_CHANGED_MASK_WIDE) != 0
                });
            selected.then_some(ClientId(*client_id))
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }

    let time = state.timestamp_now();
    crate::core_loop::fanout::fanout_event_to_clients(state, &targets, |buf, sequence, order| {
        let (classes, num_classes) = encode_device_classes(source_classes, sourceid, order);
        x11::encode_xi2_device_changed_event(
            buf,
            order,
            sequence,
            XI2_MAJOR_OPCODE,
            id,
            time,
            num_classes,
            sourceid,
            reason as u8,
            &classes,
        );
    })
}

/// Record and announce the first attached-slave event after the master
/// changes source. The state guard rejects disabled, floating, mismatched,
/// or master-only device identities.
pub fn announce_xi2_slave_switch(
    state: &mut ServerState,
    master_id: u16,
    sourceid: u16,
) -> Vec<ClientId> {
    if state.xi_record_last_slave(master_id, sourceid) {
        emit_xi2_device_changed(
            state,
            master_id,
            XiDeviceChangeReason::SlaveSwitch,
            sourceid,
        )
    } else {
        Vec::new()
    }
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
