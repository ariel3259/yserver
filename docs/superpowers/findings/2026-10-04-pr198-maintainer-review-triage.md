# PR #198 maintainer review: triage (2026-10-04)

Review by joske on PR #198 (dynamic XI device registry). Each item was checked
against `xorg-server-21.1.24` (`../xserver`, tag checked out read-only) and the
branch at `018834ad` (rebased onto `joske/master` `dbf0d403`).

Verdicts: **real** = divergence confirmed in source, needs a fix;
**done** = fixed on the branch; **answer** = needs a reply, not code;
**hw** = needs a supervised hardware run.

## Process

| # | Item | Verdict | Evidence / action |
|---|------|---------|-------------------|
| P1 | Rebase instead of merging master | done | 48 commits replayed onto `dbf0d403`, no merge commits. Every commit passes `cargo check --workspace --all-targets`; the merge-time adaptations moved into the commit that broke them (`origin` fields → "preserve source identity", `device_id` → "scope XI grabs", `DEVICEID_SLAVE_*` / ReplayPointer paired-master thaw → "leave 4/5 as virtual XTEST"). Full gate green on the tip. Backup tag `backup/xi-before-rebase`. |
| P2 | `border_press_reports_negative_content_coords_on_the_wire` fails | done (test bug) | The test pressed button 1 eight times without a release. XTEST 4 now keeps its own button state and a second press of a held button is dropped exactly like Xorg `UpdateDeviceState` (`Xi/exevents.c:948`). Reproduced without Vulkan: presses delivered `[true, false]` without release, `[true, true]` with. Fixed in `018834ad`. Not run under lavapipe here (no lavapipe ICD on this box). |

## Differences from Xorg

| # | Item | Verdict | Xorg | Branch today |
|---|------|---------|------|--------------|
| X1 | Disabled / newly added slaves float until enabled, also across VT | real | `DisableDevice` sets `dev->master = NULL` (`dix/devices.c:539`); `EnableDevice` attaches (`:389`); VT leave = `xf86ReleaseKeys` + `DisableDevice` (`xf86Events.c:310-312`) | slaves keep `attached_master` while disabled/suspended |
| X2 | "Device Enabled" property on every device | real | `AddInputDevice` creates it (`devices.c:308`), `EnableDevice`/`DisableDevice` update it (`:414`, `:529`), writes go through the property handler (`:149`) | no such property anywhere |
| X3 | XIGrabDevice on XTEST 4/5 floats them | real | `ActivatePointerGrab`: any non-master XI2 explicit grab → `DetachFromMaster` (`dix/events.c:1622`) | not traced yet |
| X4 | XIQueryDevice button state; master class sourceid = last slave | real | `ButtonInfoData` reports `down` bits (`Xi/xiquerydevice.c:282`); `DeepCopyDeviceClasses` sets `sourceid = from->id` (`Xi/exevents.c:592`) | not traced yet |
| X5 | Masters/XTEST have no device type; XTEST = CorePointerProc (10 buttons, 2 axes, no scroll); XTEST button 10 dropped | real | `CorePointerProc` `NBUTTONS 10` (`devices.c:662`) | not traced yet |
| X6 | XTestFakeInput with unknown device → BadDevice | real | `dixLookupDevice(..., deviceid & 0177)` → error, `errorValue = deviceid & 0177` (`Xext/xtest.c:182-185`) | unknown id is passed on and dropped later (`fake_input_device_id`) |
| X7 | Hierarchy/DevicePresence: one per selecting window; one device per facet with its own Added→Enabled | real | `XISendDeviceHierarchyEvent` → `SendEventToAllWindows` (`Xi/xichangehierarchy.c:119`) | not traced yet |
| X8 | The two documented limitations are divergences | real | scroll Motion under owner-events grab falls back to the grab window (`dix/events.c:4431-4464`) | documented in `docs/status.md` as accepted |

## Testing

| # | Item | Verdict | Notes |
|---|------|---------|-------|
| T1 | Touchpad laptop + grab-heavy desktop (Cinnamon/XFCE/KDE) | hw | after the fixes |
| T2 | VT round trip holding a key; xkb reset (`0849769a`) removed | answer + hw | Intentional, replaced by "release physical state once across VT suspend": at VtRelease every physical source releases its held keys/buttons through the normal path (Xorg `xf86ReleaseKeys` + `DisableDevice`), so depressed modifiers clear through xkb while locks (Caps/Num) survive, as on Xorg. The old reset also dropped Caps Lock. Hold-a-key VT run to be done after the fixes. |
| T3 | XI property write while switched away blocks until resume | real (to confirm against xf86-input-libinput) | A write submitted before suspend waits for resume; a write after suspend fails BadMatch. Xorg's driver accepts the value while the device is off and applies it on `DEVICE_ON`. |

## Smaller

| # | Item | Verdict |
|---|------|---------|
| S1 | `xinput set-prop 4 …` now hits the virtual XTEST pointer → release notes/docs | doc |
| S2 | Fold the four adversarial-review findings files into the branch-review summary | doc |
