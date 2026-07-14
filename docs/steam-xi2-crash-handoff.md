# Steam XI2 crash handoff

Date: 2026-07-14

## Symptom

On the KMS server, Steam's `steamwebhelper` starts and maps its window, then
often exits with `SIGSEGV` as soon as the window is clicked. `coredumpctl`
records the signal but no core. An attached GDB session stopped in Xlib:

```text
#4 _XLockDisplay
#5 _XFetchEventCookie at XlibInt.c:695
```

The crashing instruction had no symbol. The failure is therefore consistent
with corrupted or unexpected GenericEvent cookie data, but the current trace
does not prove that the server is the crashing process.

## Evidence

- `mate.xtrace` and `yserver-hw-mate.log` are the latest yserver traces.
- `mate-xorg.xtrace` is the Xorg comparison trace.
- The click reaches Steam's main connection and its child connection.
- The corrected yserver XI2 raw button packet is now structurally valid, and
  the trace shows one raw press followed by slave/master button events.
- Xorg emits `RawButtonPress` and `RawButtonRelease` with `length=2`, an
  eight-byte zero valuator mask, and no axis values. Yserver previously sent
  X/Y values for raw button events; this branch changes it to match Xorg.
- Xorg emits raw motion with X/Y valuators (`length=10`), which remains the
  yserver behavior.
- The passive synchronous pointer grab replay is filtered so the raw event is
  not sent a second time.
- XI2 device events are emitted slave-first, matching the recent yserver
  device-routing change.
- Present UST/MSC values in x11trace are intentionally not used as evidence;
  x11trace renders those fields in a misleading format on this setup.

## Changes on this branch

- Fixed the XI2 raw-event mask width and raw button payload shape.
- Added protocol tests for raw button and raw motion payloads.
- Suppressed duplicate raw events during synchronous pointer replay.
- Routed XI2 pointer events slave-first.
- Added the related status entry in `docs/status.md`.

## Still unresolved

Steam still crashes after these changes. The remaining likely areas are:

1. Another XI2 GenericEvent subtype delivered during the click/grab sequence,
   especially focus/crossing events or a later event after `XIGrabDevice`.
2. A semantic mismatch in the XI2 grab/focus transition that causes Steam or
   Xlib to mishandle an otherwise length-valid cookie.
3. A Steam/CEF failure unrelated to the event wire format.

The yserver crossing encoder currently emits a fixed one-word button mask for
XI2 crossing/focus events. In the Xorg trace, pointer Enter/Leave uses one
word, while keyboard FocusIn/FocusOut can carry a much wider mask. This is
worth comparing against the actual XI2 device button-class width before making
another change.

## Recommended next capture

Run the same click once under yserver and once under Xorg with the affected
Steam connection isolated. Compare every XI2 GenericEvent from the last
motion before the press through the first post-grab Present/request. If the
crash is reproducible under GDB attach, capture `x/32i $rip`, `info proc
 mappings`, and `thread apply all bt`; the current `bt full` is truncated by
 the corrupted stack and cannot identify the caller.
