# Dynamic XInput device registry for KMS input

**Status:** design for user review, 2026-09-29.  **Branch:** `feat/xi-dynamic-registry`.

## Purpose and observed failure

yserver currently publishes masters 2/3 and one slave pointer/keyboard pair
4/5. On each libinput device add, a pointer with available acceleration can
replace the metadata and libinput property owner of device 4. On this machine,
both the Razer DeathAdder V3 `event4` and the HyperX Alloy Origins 65 Mouse
`event9` advertise a pointer with acceleration. The latter can therefore own
device 4 even while motion comes from the Razer. The i3 command
`xinput set-prop 4 "libinput Accel Profile Enabled" 0 1 0` then configures the
wrong libinput source. `xinput list` exposes only one physical pointer facade.

The objective is for every usable libinput keyboard, pointer, and touch
function to have its own XInput identity and for all clients to see and
configure the same live devices. No exact device selector or heuristic for a
"primary mouse" is part of the design. A global default can set `flat` for
every eligible mouse function, including newly connected devices.

## Grounding and compatibility

- Linux can expose several `/dev/input/event*` nodes for one USB HID device.
  A libinput device has independent keyboard, pointer, and touch capabilities;
  a libinput device group correlates nodes but does not select a preferred
  pointer. Razer and HyperX each expose genuine mouse-capable event nodes.
- [wlroots' libinput backend](https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/master/backend/libinput/events.c)
  creates an input object for each announced capability and retains their
  common source. It does not select one physical pointer for the cursor.
- Xorg has a master pointer/keyboard pair and an XTEST slave pair; physical
  devices are separate slaves. Its [core device initialization](https://gitlab.freedesktop.org/xorg/xserver/-/blob/master/dix/devices.c)
  creates the XTEST pair after the masters. yserver will reserve 2/3 for
  masters and 4/5 for virtual XTEST slaves, then allocate physical facets
  starting at 6. The IDs 4/5 remain present, but they will not alias a
  physical device or expose its libinput properties.
- The existing [touchpad XI2 property design](2026-06-02-touchpad-xi2-properties-design.md)
  intentionally assumes one physical slave pointer. This design supersedes
  that topology while retaining its property encoding, XI1 touchpad atom,
  device node, and XI1/XI2 enumeration consistency requirements.
- [MATE's mouse manager](https://github.com/mate-desktop/mate-settings-daemon/blob/master/plugins/mouse/msd-mouse-manager.c)
  calls `XListInputDevices`, probes each device's libinput properties, and
  uses XI1 `DevicePresenceNotify` to reapply settings after hotplug. XI2-only
  registration is insufficient.

This scope is the direct KMS/libinput backend. Nested and synthetic input
retain their existing behavior unless they have a distinct host source that
can be represented by the same registry interface.

## Model and ownership

The input backend assigns a monotonic `SourceId` to every libinput
`DeviceAdded` instance. `SourceId` is internal and lives only for that
attachment, including its removal event. The evdev node is a descriptive
property and a fallback diagnostic key, not the runtime identity; `eventN`
can change after reconnection. The libinput group is optional physical
correlation metadata, never a key for XI routing or a reason to merge event
nodes. Vendor/product and name are metadata, not device selectors.

The central registry owns:

- `SourceId -> SourceRecord`: live capability set, group metadata if
  available, evdev node, name, vendor/product, libinput configuration
  snapshot, and the XI facet IDs created from that source.
- `XiId -> XiDevice`: role, attached master, source/facet reference for
  physical slaves, classes, enabled state, and independent property map.
- `SourceId + facet -> XiId`: event attribution and property routing.

One libinput source may create one keyboard facet and one pointer/touch
facet. The facets have different XI IDs even when they share the same
`SourceId`. `Keyboard` creates a slave keyboard attached to master 3.
`Pointer` creates a slave pointer attached to master 2. `Touch` creates a
slave pointer with XI2 TouchClass attached to master 2; if the same source
also has `Pointer`, its touch and pointer classes share that XI pointer
facet. A touchpad is a pointer classified as `TOUCHPAD`; ordinary libinput
touchpad gestures do not imply an XI2 TouchClass. A source with keyboard
and pointer capabilities, such as Razer `event5` or HyperX `event11`, gets
both facets even if it rarely emits pointer motion. Pointer capability does
not imply available acceleration.

IDs 0/1 keep their XI wildcard meaning, 2/3 are masters, and 4/5 are
virtual XTEST slaves. Physical IDs are allocated from 6 through 127. XI1
`xDeviceInfo.id` is eight bits, but the high bit of XI1 event `deviceid` is
reserved for `MORE_EVENTS`; this range keeps every advertised slave usable
by XI1 event clients as well as XI2 clients. A freed ID may be reused only
after the old source and all its facets are removed and the removal has
been published. The internal `SourceId` prevents a queued event or backend
configuration request from reaching a later device that reuses an XI ID or
an `eventN`. If IDs are exhausted, log and keep that source in the input
inventory for core/master delivery only; omit device-specific XI events
for its unpublished facet. Do not overwrite a live facet or report a
nonexistent slave.

## Device lifecycle and events

At `DeviceAdded`, the libinput backend captures all three capability bits,
the source ID, input metadata, and the full supported configuration
snapshot. It applies the configured global acceleration default before
capturing that snapshot. It stores a libinput handle by `SourceId` for
writable properties, including sources with several facets. Registration
creates all facets for one source before publishing a hierarchy change.

Every translated keyboard, pointer, scroll, and touch event carries its
`SourceId` into the core loop. The registry selects the corresponding
facet; core keyboard/pointer delivery continues through masters 3/2.
XI2 device and raw events use the actual physical facet as their source,
with Xorg-compatible `deviceid`/`sourceid` fields and selection behavior.
XTEST events use 4/5. Unknown or already removed source IDs are dropped;
known sources without an allocated facet retain core/master delivery where
core delivery exists, but have no physical XI source event. They must never
be attributed to another device. Touch begin/update/end preserve contact
IDs for the lifetime of each contact. Removing a source
releases held keys/buttons and active contacts through the existing
master/focus paths before the facets disappear.

At `DeviceRemoved`, remove all facets for that `SourceId` atomically, drop
their properties and live backend handle, then publish the new hierarchy.
Suspend/resume and server reset rebuild the registry from the live input
inventory without retaining stale source bindings. A libinput remove/add
sequence is a new source instance even if name, group, or evdev node match.

Publish XI2 `XI_HierarchyChanged` on physical facet add/remove and
`XI_DeviceChanged` when an existing device's classes change. Publish XI1
`DevicePresenceNotify` at `first_event + 15`, with its special
`0x10000 | _devicePresence` event class and `DeviceEnabled`/`DeviceRemoved`
transitions, so MATE reapplies settings after hotplug. XI1
`SelectExtensionEvent` must retain this selection rather than silently
discard it. Property changes continue to emit XI1
`DevicePropertyNotify` and XI2 `XI_PropertyEvent` for the affected facet.

## Query and property protocol

`XIQueryDevice` and XI1 `XListInputDevices` iterate one registry snapshot.
They return the same live ID set, names, roles, attachments, and compatible
class/type information. XI1 reports `MOUSE`, `KEYBOARD`, `TOUCHPAD`, or
`TOUCHSCREEN` as appropriate; type atoms are interned at server start.
XI2 emits the classes supported by each facet, including TouchClass only
where libinput reports touch. Queries for one ID and XI wildcard IDs obey
the same registry. A hotplugged device is visible in both APIs before its
presence/hierarchy notification is delivered. The XI1 encoder must accept
an arbitrary list; its current four-entry layout cannot remain.

`XIListProperties`, `XIGetProperty`, XI1 property requests,
`xinput list-props`, `XIChangeProperty`, and `XIDeleteProperty` resolve the requested
XI ID to its own property map. Physical pointer/touch facets expose only
properties supported by that libinput source, including `Device Node` and
`Device Product ID`. Keyboard facets do not inherit pointer acceleration
properties just because the same source also has pointer capability.
Writes to recognized libinput properties validate, apply to that source's
live handle, and commit the XI value only after backend success, preserving
the existing Tier 2b rule. XI device IDs, rather than a global pointer
slot, select the target. Virtual devices 4/5 have no physical `Device Node`
or libinput acceleration property.

Audit yserver code paths that hardcode 4/5 as physical slave IDs: XI1
device validity/open/grabs and event classes; XI2 selections, hierarchy,
raw/device events and grabs; property access, reset inventory, source
removal, and device-changed fanout. This is an internal code audit, not a
survey of third-party X11 clients. A physical event must never be stamped
as source 4 solely because 4 is the only current slave. Preserve core
event behavior via masters 2/3.

## Global mouse acceleration default

The KMS backend accepts `YSERVER_MOUSE_ACCEL_PROFILE=default|flat|adaptive`
at startup; unset means `default`. `flat` and `adaptive` apply at each
libinput `DeviceAdded` to every non-touchpad source with pointer capability
and a supported acceleration profile. This deliberately includes the
HyperX Mouse event node; no attempt is made to guess whether it is a
"real" mouse. Sources without acceleration support are skipped. Touchpad
settings remain independent. If a requested profile is unavailable on a
source, retain its libinput default and log the skipped source. Invalid
values are startup errors with the allowed values in the diagnostic.

The global setting is an initial default. Later XI property writes from a
client such as MATE override that source until removal. On reconnection,
the global default is applied to the new source before its properties are
published. This is the yserver counterpart of Xorg's `InputClass` plus
`AccelProfile` setting; it does not require an i3 script or an exact device
selector. The existing i3 `set-prop 4` line should be removed from the
user's configuration when this behavior is deployed; changing yserver
cannot make that line configure a physical mouse while preserving 4 as a
virtual device.

## Acceptance criteria for the implementation plan

- With the provided Razer/HyperX inventory, `xinput list` shows distinct
  pointer IDs for Razer `event4` and HyperX `event9`, keyboard facets for
  keyboard-capable nodes, and both facets for mixed nodes. The exact
  physical IDs may change across restarts; 2/3/4/5 retain their roles.
- XI1 and XI2 enumerate identical live IDs and names before and after
  add/remove. MATE receives XI1 device-presence notifications and can
  configure every pointer that exposes acceleration.
- A property write to the Razer pointer changes only the Razer libinput
  handle. The same write to the HyperX pointer changes only HyperX. A
  write to one mixed source's keyboard facet cannot alter its pointer
  facet; writes to 4/5 cannot alter physical devices.
- With `YSERVER_MOUSE_ACCEL_PROFILE=flat`, both acceleration-capable
  non-touchpad pointers report flat immediately after add and after
  reconnection, regardless of add order. Without the setting, libinput's
  default remains in force until a client changes it.
- Pointer and keyboard events carry their actual source IDs; master/core
  behavior, focus, grabs, scroll, XTEST, and existing touchpad properties
  continue to work. Touch contacts from a touch-capable source generate
  begin/update/end with a stable contact ID and are removed cleanly.
- Capacity exhaustion, unsupported properties, duplicate/stale events,
  and rapid unplug/replug do not rebind an old XI facet to another source.

The implementation plan should split registry/lifecycle, XI1 and XI2
enumeration, source-aware event routing, touch delivery, hotplug notices,
and global acceleration into reviewable stages. Each stage must preserve
the invariants above; the feature is complete only when all stages are
integrated. No implementation or tests are run by this design document.
