//! DRM display hotplug detection for the KMS backend.
//!
//! Linux-only: a udev monitor on the `drm` subsystem exposes a pollable fd.
//! The backend classifies its typed records before arming connector probes.

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, RawFd};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrmHotplugAction {
    Add,
    Remove,
    Change,
    Other,
}

impl DrmHotplugAction {
    fn from_str(action: Option<&str>) -> Self {
        match action {
            Some("add") => Self::Add,
            Some("remove") => Self::Remove,
            Some("change") => Self::Change,
            _ => Self::Other,
        }
    }

    pub(crate) const fn is_legacy_edge(self) -> bool {
        matches!(self, Self::Add | Self::Remove | Self::Change)
    }
}

/// One DRM uevent as delivered by udev. Connector records carry the owning
/// card's dev_t when udev exposes it through the DRM minor parent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DrmHotplugRecord {
    pub(crate) action: DrmHotplugAction,
    pub(crate) dev_t: Option<crate::platform::drm::DrmDeviceKey>,
    pub(crate) devnode: Option<std::path::PathBuf>,
    pub(crate) subsystem: Option<String>,
    pub(crate) is_card_node: bool,
    pub(crate) is_render_node: bool,
    pub(crate) is_connector_subdevice: bool,
    pub(crate) hotplug: bool,
}

#[cfg(target_os = "linux")]
impl DrmHotplugRecord {
    fn from_udev_event(event: &udev::Event) -> Self {
        let devnode = event.devnode().map(std::path::Path::to_path_buf);
        let devtype = event.devtype().and_then(|value| value.to_str());
        let node_name = devnode
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .or_else(|| event.sysname().to_str());
        let is_card_node = devtype == Some("drm_minor") && node_name.is_some_and(is_card_name);
        let is_render_node = devtype == Some("drm_minor") && node_name.is_some_and(is_render_name);
        let is_connector_subdevice = devtype == Some("drm_connector");
        let devnum = event.devnum().or_else(|| {
            event
                .parent_with_subsystem_devtype("drm", "drm_minor")
                .ok()
                .flatten()
                .and_then(|parent| parent.devnum())
        });
        let subsystem = event
            .subsystem()
            .and_then(|value| value.to_str())
            .map(str::to_owned);

        Self {
            action: DrmHotplugAction::from_str(event.action().and_then(|value| value.to_str())),
            dev_t: devnum.map(device_key_from_dev_t),
            devnode,
            subsystem,
            is_card_node,
            is_render_node,
            is_connector_subdevice,
            hotplug: event
                .property_value("HOTPLUG")
                .is_some_and(|value| value == "1"),
        }
    }
}

fn is_card_name(name: &str) -> bool {
    name.strip_prefix("card").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn is_render_name(name: &str) -> bool {
    name.strip_prefix("renderD").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[cfg(target_os = "linux")]
fn device_key_from_dev_t(dev_t: libc::dev_t) -> crate::platform::drm::DrmDeviceKey {
    crate::platform::drm::DrmDeviceKey {
        major: libc::major(dev_t) as u32,
        minor: libc::minor(dev_t) as u32,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrmHotplugDisposition {
    DeviceRemoved,
    DeviceAddedOrReplaced,
    ConnectorEdge,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassifiedDrmHotplugEvent {
    pub(crate) record: DrmHotplugRecord,
    pub(crate) disposition: DrmHotplugDisposition,
    pub(crate) device: Option<crate::platform::drm::DrmDeviceKey>,
    pub(crate) incarnation: Option<crate::kms::owner::identity::IncarnationId>,
    pub(crate) renderer_removed: bool,
}

/// Classify DRM uevents against the open card incarnations and the selected
/// Vulkan renderer. A renderer can use a primary node that is not an open KMS
/// card, so renderer identity is checked independently of the card map.
pub(crate) fn classify_drm_hotplug_records(
    records: impl IntoIterator<Item = DrmHotplugRecord>,
    open_cards: &std::collections::HashMap<
        crate::platform::drm::DrmDeviceKey,
        Option<crate::kms::owner::identity::IncarnationId>,
    >,
    renderer_nodes: &std::collections::HashSet<crate::platform::drm::DrmDeviceKey>,
) -> Vec<ClassifiedDrmHotplugEvent> {
    records
        .into_iter()
        .map(|record| {
            let dev_t = record.dev_t;
            let renderer_removed = record.subsystem.as_deref() == Some("drm")
                && record.action == DrmHotplugAction::Remove
                && (record.is_card_node || record.is_render_node)
                && dev_t.is_some_and(|key| renderer_nodes.contains(&key));
            let open =
                dev_t.and_then(|key| open_cards.get(&key).map(|incarnation| (key, *incarnation)));
            let (disposition, device, incarnation) = if record.subsystem.as_deref() != Some("drm") {
                (DrmHotplugDisposition::Ignored, None, None)
            } else {
                match record.action {
                    DrmHotplugAction::Remove if record.is_card_node => match open {
                        Some((key, Some(incarnation))) => (
                            DrmHotplugDisposition::DeviceRemoved,
                            Some(key),
                            Some(incarnation),
                        ),
                        Some((key, None)) => {
                            (DrmHotplugDisposition::ConnectorEdge, Some(key), None)
                        }
                        None => (DrmHotplugDisposition::Ignored, None, None),
                    },
                    DrmHotplugAction::Add if record.is_card_node && open.is_none() => {
                        (DrmHotplugDisposition::DeviceAddedOrReplaced, dev_t, None)
                    }
                    DrmHotplugAction::Change if open.is_some() => {
                        let (key, incarnation) = open.expect("open card checked above");
                        (DrmHotplugDisposition::ConnectorEdge, Some(key), incarnation)
                    }
                    _ if record.is_connector_subdevice && open.is_some() => {
                        let (key, incarnation) = open.expect("open card checked above");
                        (DrmHotplugDisposition::ConnectorEdge, Some(key), incarnation)
                    }
                    _ => (DrmHotplugDisposition::Ignored, None, None),
                }
            };
            ClassifiedDrmHotplugEvent {
                record,
                disposition,
                device,
                incarnation,
                renderer_removed,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) struct DrmHotplugMonitor {
    socket: Option<udev::MonitorSocket>,
    scripted_records: Option<Vec<DrmHotplugRecord>>,
}

#[cfg(target_os = "linux")]
impl DrmHotplugMonitor {
    pub(crate) fn new() -> std::io::Result<Option<Self>> {
        let builder = match udev::MonitorBuilder::new() {
            Ok(builder) => builder,
            Err(e) => {
                log::warn!("drm hotplug: udev monitor unavailable: {e}; hotplug disabled");
                return Ok(None);
            }
        };
        let socket = builder.match_subsystem("drm")?.listen()?;
        log::info!("drm hotplug: udev monitor listening on drm subsystem");
        Ok(Some(Self {
            socket: Some(socket),
            scripted_records: None,
        }))
    }

    #[cfg(test)]
    pub(crate) fn from_records(records: Vec<DrmHotplugRecord>) -> Self {
        Self {
            socket: None,
            scripted_records: Some(records),
        }
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.socket.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    pub(crate) fn drain(&mut self) -> Vec<DrmHotplugRecord> {
        if let Some(records) = self.scripted_records.take() {
            return records;
        }
        self.socket
            .as_mut()
            .into_iter()
            .flat_map(|socket| socket.iter())
            .map(|event| {
                let record = DrmHotplugRecord::from_udev_event(&event);
                log::debug!(
                    "drm hotplug: uevent action={:?} dev_t={:?} devnode={:?} card={} render={} connector={} HOTPLUG={}",
                    record.action,
                    record.dev_t,
                    record.devnode,
                    record.is_card_node,
                    record.is_render_node,
                    record.is_connector_subdevice,
                    record.hotplug,
                );
                record
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{
        ClassifiedDrmHotplugEvent, DrmHotplugAction, DrmHotplugDisposition, DrmHotplugRecord,
        classify_drm_hotplug_records,
    };
    use crate::{kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

    fn key(minor: u32) -> DrmDeviceKey {
        DrmDeviceKey { major: 226, minor }
    }

    fn record(
        action: DrmHotplugAction,
        dev_t: Option<DrmDeviceKey>,
        devnode: Option<&str>,
        subsystem: &str,
        is_card_node: bool,
        is_connector_subdevice: bool,
        hotplug: bool,
    ) -> DrmHotplugRecord {
        DrmHotplugRecord {
            action,
            dev_t,
            devnode: devnode.map(std::path::PathBuf::from),
            subsystem: Some(subsystem.to_owned()),
            is_card_node,
            is_render_node: devnode
                .is_some_and(|node| node.rsplit('/').next().is_some_and(super::is_render_name)),
            is_connector_subdevice,
            hotplug,
        }
    }

    fn classify(
        record: DrmHotplugRecord,
        open: &HashMap<DrmDeviceKey, Option<IncarnationId>>,
        renderer: &HashSet<DrmDeviceKey>,
    ) -> ClassifiedDrmHotplugEvent {
        classify_drm_hotplug_records([record], open, renderer)
            .pop()
            .expect("one input produces one typed classification")
    }

    #[test]
    fn c0_3cii_udev_events_classified() {
        let open = HashMap::from([(key(1), Some(IncarnationId::first())), (key(2), None)]);
        let renderer = HashSet::from([key(1)]);

        let removed = classify(
            record(
                DrmHotplugAction::Remove,
                Some(key(1)),
                Some("/dev/dri/card1"),
                "drm",
                true,
                false,
                false,
            ),
            &open,
            &renderer,
        );
        assert_eq!(removed.disposition, DrmHotplugDisposition::DeviceRemoved);
        assert_eq!(removed.device, Some(key(1)));
        assert_eq!(removed.incarnation, Some(IncarnationId::first()));
        assert!(removed.renderer_removed);

        let added = classify(
            record(
                DrmHotplugAction::Add,
                Some(key(3)),
                Some("/dev/dri/card3"),
                "drm",
                true,
                false,
                false,
            ),
            &open,
            &renderer,
        );
        assert_eq!(
            added.disposition,
            DrmHotplugDisposition::DeviceAddedOrReplaced
        );
        assert_eq!(added.device, Some(key(3)));

        let change = classify(
            record(
                DrmHotplugAction::Change,
                Some(key(2)),
                Some("/dev/dri/card2"),
                "drm",
                true,
                false,
                true,
            ),
            &open,
            &renderer,
        );
        assert_eq!(change.disposition, DrmHotplugDisposition::ConnectorEdge);
        assert_eq!(change.device, Some(key(2)));
        assert!(change.record.hotplug);

        let connector = classify(
            record(
                DrmHotplugAction::Other,
                Some(key(1)),
                None,
                "drm",
                false,
                true,
                true,
            ),
            &open,
            &renderer,
        );
        assert_eq!(connector.disposition, DrmHotplugDisposition::ConnectorEdge);
        assert_eq!(connector.device, Some(key(1)));

        let other_subsystem = classify(
            record(
                DrmHotplugAction::Change,
                Some(key(1)),
                Some("/dev/dri/card1"),
                "input",
                true,
                false,
                true,
            ),
            &open,
            &renderer,
        );
        assert_eq!(other_subsystem.disposition, DrmHotplugDisposition::Ignored);

        let unopened_change = classify(
            record(
                DrmHotplugAction::Change,
                Some(key(9)),
                Some("/dev/dri/card9"),
                "drm",
                true,
                false,
                true,
            ),
            &open,
            &renderer,
        );
        assert_eq!(unopened_change.disposition, DrmHotplugDisposition::Ignored);

        let legacy_remove = classify(
            record(
                DrmHotplugAction::Remove,
                Some(key(2)),
                Some("/dev/dri/card2"),
                "drm",
                true,
                false,
                false,
            ),
            &open,
            &renderer,
        );
        assert_eq!(
            legacy_remove.disposition,
            DrmHotplugDisposition::ConnectorEdge
        );

        let unopened_remove = classify(
            record(
                DrmHotplugAction::Remove,
                Some(key(99)),
                Some("/dev/dri/card99"),
                "drm",
                true,
                false,
                false,
            ),
            &open,
            &renderer,
        );
        assert_eq!(
            unopened_remove.disposition,
            DrmHotplugDisposition::Ignored,
            "a remove record for a card outside the open map is not DeviceRemoved"
        );
    }

    #[test]
    fn c0_3cii_renderer_identity_matches_separate_primary_and_render_nodes() {
        let open = HashMap::from([(key(2), Some(IncarnationId::first()))]);
        let renderer = HashSet::from([key(100), key(130)]);
        for (minor, node) in [(100, "card0"), (130, "renderD130")] {
            let classified = classify(
                record(
                    DrmHotplugAction::Remove,
                    Some(key(minor)),
                    Some(node),
                    "drm",
                    minor == 100,
                    false,
                    false,
                ),
                &open,
                &renderer,
            );
            assert!(classified.renderer_removed);
            assert_eq!(classified.disposition, DrmHotplugDisposition::Ignored);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn monitor_opens_and_drains_without_blocking() {
        match super::DrmHotplugMonitor::new() {
            Ok(Some(mut monitor)) => {
                assert!(monitor.raw_fd() >= 0);
                let _ = monitor.drain();
            }
            Ok(None) => {}
            Err(e) => panic!("monitor construction errored unexpectedly: {e}"),
        }
    }
}
