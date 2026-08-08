# GLX extension string terminator — design

**Status:** **approved** — rev 3 (Opus review rounds 1-2 closed:
REJECT → APPROVE WITH EDITS, all findings applied), 2026-08-07
**Base:** branch `glx-extension-string-terminator`, off `master` @
`f4c0b60c`. **All yserver line numbers in this document are against that
base.** Xorg citations are against `~/Projects/xserver` @ `5541a5c8`.
**Scope:** `crates/yserver-core/src/core_loop/process_request.rs`
(the builder and its tests) and `docs/status.md`.
`crates/yserver-protocol/src/x11/glx.rs` is **read** for its constants
and encoder but is deliberately **not modified** — see "Design".

## What changed in rev 3

Review round 2 returned **APPROVE WITH EDITS** — no blocking findings,
ten minors, all applied. The two that changed a factual claim:

- **The reply does not grow.** rev 2 said "the reply grows by exactly
  one byte". `encode_string_reply` pads to a 4-byte boundary, so in both
  real configurations only `n` changes and the padded reply is the same
  size. Corrected in "Both reply paths get it" with the arithmetic.
- **Rows 03/04/05 are XWayland carrying yserver's *string*, not
  yserver.** They prove the terminator is *sufficient* on a working
  server; they do not prove it is the last remaining difference on
  yserver. Stated plainly under the measurement table, and the hardware
  gate is what settles the real-server direction.

Also applied: `AGENTS.md:6` (not `:5`) is the `docs/status.md` rule;
`glx.rs` dropped from Scope since nothing there changes; "eight" causes
corrected to nine; four citation ranges tightened by a line; the
`n` column relabelled; two further tool limitations disclosed (the
`srvexts` dispatch is content-based like `srvvendor`, and every row is
n = 1 on one driver branch); and a second existing test that asserts on
`SERVER_EXTENSIONS`'s content noted as unaffected.

## What changed in rev 2

Review round 1 returned **REJECT**, on one finding that mattered more
than the rest: the two headline measurement tables in rev 1 had **no
preserved artifacts**. Those runs wrote to a session scratchpad and
passed `--quiet` to the mutation proxy, so nothing on disk recorded the
injected string — and the injected string's trailing space *was the
variable under test*. The tables were presented as "four of four,
controlled in both directions" on evidence that could not be
re-examined.

- The proxy now logs the injected string with `repr()` and its byte
  length **unconditionally** (`glxmangle.py`, `_mangle_serverstring`),
  because a trailing space is invisible in any other rendering.
- Every row was re-measured into
  `~/yserver-glx-logs/2026-08-07-terminator-evidence/`, three files per
  row, and each table cell below names its file.
- The re-run produced **better** evidence than rev 1 claimed: rows 02
  and 05 separate the two effects, which rev 1 asserted without a run
  that could distinguish them.
- Corrected: the Xorg and yserver line citations flagged as drifted, the
  consumer-parser citations (all three were misnamed), the contradiction
  between the Design and Testing sections about what the regression test
  asserts, and the false claim that no test encodes this string.
- Added: a Citation audit, a Documentation section, the `AGENTS.md:19`
  invocation that licenses the change, and the empty-list edge case.

## Discovery context

This is **defect D** of the NVIDIA GLX investigation, the last of four.
Defects A and C landed in `master` as PRs #118 and #119; defect B (the
`GLX_EXT_texture_from_pixmap` suppression) remains open and is
explicitly out of scope here.

The symptom is that `kwin_x11` aborts with SIGABRT under yserver on
NVIDIA:

```
No provider of glXGetProcAddressARB found.  Requires one of:
    GLX_ARB_get_proc_address
```

libepoxy raises this when `glXQueryExtensionsString` — the *client*
extension string, which `libGLX_nvidia` computes — does not contain
`GLX_ARB_get_proc_address`. Against a live XWayland on the same GPU and
the same driver (RTX 5060 Ti, NVIDIA 610.57.04, open kernel modules) the
same client string contains it.

Nine candidate causes were eliminated by measurement before the real
one was found: the GLX vendor string, the GLX version,
`GLX_EXT_import_context`, the size and content of the server extension
list, the core X11 vendor, `__GLX_VENDOR_LIBRARY_NAME`, the shape of the
`GetVisualConfigs` reply (18 → 40 props), the shape of the
`GetFBConfigs` reply (33 → 44 attribs), and `QUERY_CONTEXT`. Evidence
for those rounds is in `~/yserver-glx-logs/2026-08-06-defect-d-ab/`,
`~/yserver-glx-logs/2026-08-07-defect-d-ab2/` and
`~/yserver-glx-logs/2026-08-07-epoxyprobe/`.

The cause was isolated with `~/yserver-glx-probes/defect-d/glxmangle.py`
— an X proxy that sits between a client and a working XWayland and
**mutates** its replies, so a working server can be degraded toward
yserver's shape one variable at a time without needing a KMS session.

## Problem — measured on the wire

**yserver's GLX extension string is not space-terminated. Xorg's is.
`libGLX_nvidia` loses the last token when the terminator is missing, and
separately drops `GLX_ARB_get_proc_address` from its own built-in
client-side set.**

`glx_extension_string` (`process_request.rs:11691-11703`) joins tokens
with single separating spaces and ends the string at the last token:

```rust
let mut s = String::from(yserver_protocol::x11::glx::SERVER_EXTENSIONS);
s.push(' ');
s.push_str(yserver_protocol::x11::glx::SGIX_FBCONFIG_EXTENSION);
if tfp_supported {
    s.push(' ');
    s.push_str(yserver_protocol::x11::glx::TFP_EXTENSION);
}
s
```

On NVIDIA, where `glx_tfp_supported` is false, the string therefore ends
`... GLX_EXT_libglvnd GLX_SGIX_fbconfig` with no trailing space.

Xorg writes a space after **every** enabled extension. The emitting
block is `glx/extension_string.c:139-149`; the two lines that matter are
`:144-145`:

```c
buffer[length + len + 0] = ' ';
buffer[length + len + 1] = '\0';
```

so every Xorg extension string ends with `' '` before the NUL. Captured
from the live XWayland with `cat -A`
(`terminator-evidence/xwayland-srv-extensions-cat-A.txt`), where `$`
marks end of line:

```
... GLX_SGIX_pbuffer GLX_SGIX_visual_select_group $
```

### The measurement

`glxmangle.py` rewriting **only** the
`QueryServerString(GLX_EXTENSIONS)` reply of a live XWayland,
everything else untouched. Oracle is `glXQueryExtensionsString(dpy, 0)`,
the same call `epoxy_has_glx_extension` makes (libepoxy 1.5.10,
`src/dispatch_glx.c:134-144`). Artifacts in
`~/yserver-glx-logs/2026-08-07-terminator-evidence/`; each row has a
`.proxy` (the injected string as `repr()`, plus the reply's `n` — which
is the string length *including* the NUL, so it is one more than the
byte count of the string itself), a `.txt`
(`glxprobe`) and an `.epoxy` (`epoxyprobe`).

| row / artifact | injected tail | reply `n` | `get_proc_address` | `SGIX_fbconfig` |
|---|---|---|---|---|
| `00-control-sin-mutar` | *(unmutated XWayland)* | — | yes | yes |
| `01-xwl-con-espacio` | `…select_group ` | 638 | **yes** | yes |
| `02-xwl-sin-espacio` | `…select_group` | 637 | **no** | yes |
| `03-ys-con-espacio` | `…GLX_SGIX_fbconfig ` | 234 | **yes** | yes |
| `04-ys-sin-espacio` | `…GLX_SGIX_fbconfig` | 233 | **no** | **no** |
| `05-ys-token-dummy` | `…GLX_YSERVER_DUMMY` | 251 | no | yes |

Rows 01/02 and 03/04 are one-variable pairs: identical token content,
differing by the single trailing byte. Adding the terminator to
yserver's list makes the oracle flip to "present"; removing it from
XWayland's own list makes it flip to "absent".

**Read that precisely.** Rows 03/04/05 are a *working XWayland carrying
yserver's extension string* — its visuals, fbconfigs, GLX vendor,
version and `GLX_VENDOR_NAMES_EXT` are all still XWayland's. They
establish that the terminator is **sufficient** to flip the oracle on an
otherwise-working server. They do **not** establish that it is the last
remaining difference on yserver itself. That direction is unmeasured and
is exactly what the hardware gate in "Testing" exists to settle.

### The mechanism, isolated

The rows above separate two independent effects of the same missing
byte:

1. **The last token is lost.** Compare row 02 (XWayland's list, no
   terminator — `GLX_SGIX_fbconfig` is *not* last there, and it
   survives) against row 04 (yserver's list, no terminator —
   `GLX_SGIX_fbconfig` *is* last, and it is lost). Row 05 completes the
   separation: appending a dummy token to yserver's list makes
   `GLX_SGIX_fbconfig` reappear while the string is still unterminated.
   **It is position, not identity** — which is why `libGLX_nvidia` never
   credited us with an extension we do advertise.

2. **`GLX_ARB_get_proc_address` is lost regardless of position.** Rows
   02, 04 and 05 all lack it; rows 01 and 03 have it. That extension is
   never in any server string — Xorg deliberately excludes it, with the
   comment *"GLX_ARB_get_proc_address is implemented on the client"*
   (`glx/extension_string.c:74`) — and `libGLX_nvidia` carries it in a
   built-in client-side set, visible in the binary at offset `0xe5a10`:
   `"GLX_ARB_get_proc_address GLX_SGI_swap_control GLX_EXT_swap_control
   GLX_EXT_buffer_age GLX_NV_copy_image GLX_NV_copy_buffer "` (that
   string is itself space-terminated). Against yserver five of those six
   appear and only the first is missing, so the set is being applied and
   that one entry is dropped.

**Effect 2's internal logic is not explained here and this document does
not claim to explain it.** `libGLX_nvidia` is closed source. What is
claimed is the measured input/output relation, in six controlled rows
with preserved artifacts.

### Independent corroboration, from an earlier round

`~/yserver-glx-logs/2026-08-06-defect-d-ab/v3-xwayland-exts-probe.txt`
was captured a day earlier by a different tool
(`run-defect-d-ab.sh`, a patched yserver on real KMS — no proxy
involved) and shows the same thing against the real server: `srv
EXTENSIONS` ends at `GLX_SGIX_fbconfig` with no trailing space, that
final token is absent from `client EXTENSIONS`, and exactly five of the
six `0xe5a10` tokens appear. This matters because it removes the proxy
from the causal chain entirely for the parts it covers.

### Why this was invisible until now

- **Mesa tokenises correctly**, so `libGLX_mesa` never cared. Every
  measurement through the Mesa vendor reports
  `GLX_ARB_get_proc_address` present, which is why `yserver + mesa`
  never reproduced it and why the defect reads as NVIDIA-specific when
  it is really ours.
- **The earlier A/B expanded yserver's list to XWayland's** and still
  measured "no", which pointed the investigation away from the string.
  That variant appended tokens but still produced no trailing space, so
  it changed the content and preserved the defect. Row 02 above is the
  controlled form of the same observation.

### Known limitations of the mutation tool

Stated so the evidence can be weighed rather than trusted.

- **The `srvvendor` mutation is unsound** and is not used by any row
  above. Its heuristic picks which reply to rewrite from the reply's
  *content*, so it also clobbers the `GLX_VENDOR_NAMES_EXT` (0x20F6)
  reply, making libglvnd load a nonexistent vendor. That produced a
  false `no` in an earlier matrix run. The `srvexts` rows are not
  affected: their `.txt` artifacts all report a non-NULL client string,
  whereas the clobbered runs report `NULL`.
- **Mutation dispatch is content-based, not request-based.** The reply
  to `QueryServerString` does not carry the `name` that was asked for,
  and the proxy does not correlate it with the request body, so
  `_mangle_serverstring` decides what to rewrite by inspecting the
  returned string (`glxmangle.py:307`). For `srvexts` the discriminator
  is `"GLX_" in old`, which on this server matches only the extension
  list — the GLX vendor is `SGI` and the vendor-names reply is `nvidia`.
  Benign here, but it is a property of the dispatch, not a quirk of one
  mutation.
- **The proxy does not mutate opcode 18** (`QueryExtensionsString`),
  only 14, 19 and 21. Constant across all rows, so it is not a
  differential confound, but it means the mangled XWayland is not fully
  shape-equivalent to yserver, which serves the short string on both
  opcodes.
- **Every row is n = 1** — one run, one GPU, one driver branch
  (RTX 5060 Ti, 610.57.04, open modules). This project has already been
  bitten once by attributing a driver-branch behaviour to a GPU
  generation, so: the rows are internally consistent and mutually
  controlled, but nothing here is a claim about NVIDIA in general.

## Design

**Terminate every token with a space, exactly as Xorg does.**

`glx_extension_string` is rewritten so the terminator is applied when a
chunk is appended, rather than being added once at the end:

```rust
fn glx_extension_string(tfp_supported: bool) -> String {
    let mut s = String::new();
    let mut push = |chunk: &str| {
        s.push_str(chunk);
        s.push(' ');
    };
    push(SERVER_EXTENSIONS);
    push(SGIX_FBCONFIG_EXTENSION);
    if tfp_supported {
        push(TFP_EXTENSION);
    }
    s
}
```

The observable result for a non-empty list is the same as appending one
space at the end. The reason to prefer this shape is that **the
builder's output ends in a terminator by construction**, so a future
extension appended by the same mechanism cannot reintroduce the defect.

Note precisely what that does and does not guarantee: `SERVER_EXTENSIONS`
is itself a multi-token constant with its own internal single-space
separators, so the builder enforces per-*chunk* termination, not
per-*token*. This mirrors Xorg, where `known_glx_extensions[].name` are
bare tokens and `extension_string.c:144` supplies the space.

`SERVER_EXTENSIONS` (`glx.rs:201-204`) stays a space-*separated*
constant and does **not** gain a trailing space, and `TFP_EXTENSION` /
`SGIX_FBCONFIG_EXTENSION` stay bare tokens. Keeping the terminator in
one place — the builder — avoids a second convention in which callers
must remember whether a given constant is self-terminating.

### Why this is the Xorg-shaped fix, not a preference

`AGENTS.md:19` — *if Xorg deviates from spec, we need to follow Xorg*.
The GLX protocol describes a space-**separated** list, so a string
without a trailing space is arguably conformant. Xorg nonetheless emits
a trailing space, and a shipping client library depends on it. That is
exactly the case the rule governs.

### Both reply paths get it

The string is served by two opcodes and both call the same builder:

- `QUERY_SERVER_STRING` / `STRING_EXTENSIONS` (opcode 19) —
  `process_request.rs:12595`
- `QUERY_EXTENSIONS_STRING` (opcode 18) — `process_request.rs:12624`

The existing comment at `:12622-12623` already records that these two
must stay identical. Fixing the builder fixes both by construction; this
design adds no second code path.

`encode_string_reply` (`glx.rs:267-291`) sends `n = bytes.len() + 1` and
pads to a 4-byte boundary, so **`n` grows by one and the padded reply is
unchanged in size in both real configurations**: 232→233 bytes of string
with `tfp_supported = false` (n 233→234, padded 236, reply 268), and
260→261 with it true (n 261→262, padded 264, reply 296). No caller
computes that length independently. Xorg's equivalent sends
`strlen(GLXextensions) + 1` (`glxcmds.c:2370`).

### Empty-list edge case

With no extensions at all the proposed builder returns `""`, which has
no terminator. The case is unreachable: `SERVER_EXTENSIONS` is a
non-empty constant and `SGIX_FBCONFIG_EXTENSION` is unconditional. It is
deliberately not special-cased, and the regression test documents the
non-empty contract rather than pretending to cover the empty one.
(Xorg has the same unreachable hole from the other direction: its
builder returns length 1 for an empty set and `glxscreens.c:428-432`
then allocates a byte the second call never writes.)

## What this design does NOT claim

- **It does not fix defect B.** `GLX_EXT_texture_from_pixmap` stays
  suppressed on NVIDIA. Once KWin gets past `glXGetProcAddressARB` it
  may well reach `glXBindTexImageEXT` and fail there instead — that is
  the next defect, deliberately not addressed here, and the plan's
  measurement step is written to report it rather than to treat it as a
  failure of this change.
- **It does not claim KWin will composite.** The claim is that the two
  extensions NVIDIA withholds return, and that the specific abort in the
  discovery context stops. Anything past that point is unmeasured.
- **It does not explain `libGLX_nvidia`'s parser.** See "The mechanism".
- **It does not touch the config replies.** See "Deferred".

## Deferred — recorded here, implemented elsewhere

Found while investigating defect D, measured, and deliberately left out
so this stays a one-variable change.

### `GetVisualConfigs` declares 18 properties where Xorg declares 40

Xorg always sends `GLX_VIS_CONFIG_TOTAL` — defined as `18 + 22` at
`glxcmds.c:893-899` — the 18-field unpaired prefix followed by tagged
pairs, zero-padded. `encode_get_visual_configs_reply` hardcodes 18.
**Measured not to affect defect D**
(`2026-08-07-defect-d-ab2/v1-visuals-40-*`).

### `GetFBConfigs` declares 33 attributes where Xorg declares 44

Xorg always sends `__GLX_TOTAL_FBCONFIG_ATTRIBS = 44`
(`glxcmds.c:1005`), zero-padded with `(0, 0)` pairs.
`synthesise_glx_fb_configs` produces 33 (28 without TFP), missing
`GLX_RGBA`, the five `GLX_TRANSPARENT_*_VALUE` pairs,
`GLX_SWAP_METHOD_OML`, `GLX_VISUAL_SELECT_GROUP_SGIX` and the two
`GLX_OPTIMAL_PBUFFER_*_SGIX` pairs; and it gates the four
`GLX_BIND_TO_TEXTURE_*` pairs plus `GLX_Y_INVERTED_EXT` on
`tfp_supported` where Xorg emits them unconditionally with value 0.

**This one is not a drop-in.** Implemented as an A/B patch, it **broke
Mesa**: `v2`/`v3` of the 2026-08-07 round reported `No matching
fbConfigs or visuals found` and got no context, because `driConfigEqual`
scalar-compares those attributes against the client driver's
`__DRIconfig`, and emitting `GLX_BIND_TO_TEXTURE_RGB_EXT = 0` where the
pair used to be absent is enough for every config to be rejected. The
patch is preserved at
`~/yserver-glx-probes/defect-d/patchB-fbconfigs-44attribs.diff`.
Whoever picks this up must solve the Mesa interaction first.

`fbconfigs_omit_bind_to_texture_when_tfp_unsupported`
(`process_request.rs:57359-57360`) enshrines that divergence — it
asserts `GLX_BIND_TO_TEXTURE_RGB_EXT` is absent when TFP is unsupported.
If the parity work lands, that test is rewritten, not deleted.

### `QUERY_CONTEXT` returns zero attributes

Already recorded as D7 in the 2026-08-03 design. Xorg's `DoQueryContext`
sends five properties including `GLX_SCREEN_EXT`. **Measured not to be
the cause of defect D**: `libGLX_nvidia` supplies the screen number to
its caller regardless, verified with `epoxyprobe` (sentinel `0x5EED5EED`
overwritten with 0 — `2026-08-07-epoxyprobe/yserver-nvidia.txt`). Still
a divergence, still open.

## Risk and compatibility

**Low.** The change adds one byte to a reply whose length field is
computed by the encoder. The three consumers that parse this string were
read on this machine rather than assumed:

- **libglvnd 1.7.0** — `IsTokenInString`
  (`src/util/utils_misc.c:167-178`) over `FindNextStringToken`
  (`:100-117`). The latter skips leading separators and returns 0 for a
  zero-length token, so a trailing space yields no empty token. This is
  the load-bearing path: it is what detects `GLX_EXT_libglvnd`
  (`src/GLX/libglxmapping.c:654-655`), whose absence is what makes
  cogl/Cinnamon crash on Asahi. (`strtok_r` appears in libglvnd only on
  the `GLX_VENDOR_NAMES_EXT` reply, `libglxmapping.c:566-571` — not on
  this string.)
- **Mesa** — `__glXProcessServerString`
  (`mesa-26.0.8/src/glx/glxextensions.c:345-373`). Its inner loop skips
  trailing separators and the outer loop then sees NUL.
- **libepoxy 1.5.10** — `epoxy_extension_in_string`
  (`src/dispatch_common.c:512`) accepts `' '` or `'\0'` after a match.

Xorg has emitted a trailing space for the lifetime of this API, so a
client that could not tolerate it would already be broken everywhere.

- **One existing test encodes this string** and survives:
  `server_extensions_advertise_create_context` (`glx.rs:908-939`) feeds
  `SERVER_EXTENSIONS` through `encode_string_reply` and asserts with
  `contains`. `SERVER_EXTENSIONS` is unchanged by this design, so the
  test is untouched. A second test, `sgix_fbconfig_not_in_base_server_extensions`
  (`glx.rs:1201-1210`), asserts on the *constant*'s content rather than
  on the encoded string; it is likewise unaffected, since the constant
  does not change. No fixtures outside the Rust sources reference the
  string — the only other hits are prose in `docs/superpowers/`.
- **The Mesa path must be behaviourally unchanged**, which the plan
  verifies with `__GLX_VENDOR_LIBRARY_NAME=mesa` as a control: Mesa
  reported `GLX_ARB_get_proc_address` present before this change and
  must still report it after.
- **`glx_tfp_supported = true`** (AMD/Intel, and NVIDIA once defect B is
  resolved) changes the final token from `GLX_EXT_texture_from_pixmap`
  to the same token plus a space. No client keys on the final token.

## Testing

In-tree and hardware-free. **Test 1 is the regression test** and is the
only one required to be RED before the fix.

1. **`glx_extension_string_is_space_terminated`** — asserts
   `glx_extension_string(true)` and `glx_extension_string(false)` both
   end with `' '`. RED today: both end with an alphanumeric.
   `ends_with(' ')` *is* the invariant being defended — it is what
   catches a future extension appended without a terminator.
2. **`glx_extension_string_tokens_match_advertised_constants`** — after
   trimming, the token list equals, in order, `SERVER_EXTENSIONS`'s tokens plus
   `GLX_SGIX_fbconfig`, plus `GLX_EXT_texture_from_pixmap` when
   `tfp_supported`; and the string contains no `"  "` and does not start
   with `' '`. Guards against the terminator work dropping, duplicating
   or double-spacing a token. **Green before and after** — it is a
   non-regression guard, not a driver of the change, and is labelled as
   such rather than being sold as proof the fix was implemented one way
   rather than another.

`glx_extension_string_includes_tfp_only_when_capable`
(`process_request.rs:57285-57293`) already calls the real builder and
covers the TFP branch; test 2 is scoped to the delta and does not
restate it.

**`glx_extension_string_contains_sgix_fbconfig`
(`process_request.rs:58998`) must be repaired, not extended.** It does
not call `glx_extension_string` at all — `:58999` says so verbatim
("mirror its logic here") — it re-implements the builder's body in the
test and asserts against its own copy, so it would pass if the function
were deleted. It is changed to call the real function.

**Hardware measurement (gate, not a unit test).** From tty2:

```
~/yserver-glx-probes/defect-d/run-epoxyprobe.sh
```

Expected on the NVIDIA vendor: `get_proc_address=SI` and `CON EL SCREEN
CORRECTO: la extension ESTA`. The Mesa control must be unchanged. Then,
separately, a Plasma session to see how far KWin gets — with the
explicit expectation that it may now fail at `glXBindTexImageEXT`
instead, which is defect B and not a regression of this change.

## Documentation

- `docs/status.md` gains an entry dated 2026-08-07 (`AGENTS.md:6`
  requires it be kept current), recording the defect, the one-byte
  cause, and that defects B remains open.
- No man page change: the string is not user-configurable.

## Citation audit — 2026-08-07 (rev 2)

Every citation below was opened and read on this machine while writing
rev 2. Items marked **corrected** were wrong in rev 1.

| citation | status |
|---|---|
| `process_request.rs:11691-11703` builder | verified |
| `process_request.rs:12595`, `:12624` call sites; `:12622-12623` comment | verified |
| `process_request.rs:57285-57293` existing TFP test | **corrected** (rev 2 said 57286) |
| `process_request.rs:58998-58999` self-mirroring test | verified |
| `process_request.rs:57359-57360` bind-to-texture test | **corrected** (rev 1 said 57438) |
| `glx.rs:201-204` `SERVER_EXTENSIONS` | verified |
| `glx.rs:267-291` `encode_string_reply` | verified |
| `glx.rs:908-939` test that encodes the string | **corrected** (rev 1 claimed no test did; rev 2 mis-ranged it) |
| Xorg `extension_string.c:139-149`, terminator at `:144-145` | **corrected** (rev 1 said 141-145) |
| Xorg `extension_string.c:74` client-side exclusion | verified |
| Xorg `glxcmds.c:893-899` `GLX_VIS_CONFIG_TOTAL` | **corrected** (rev 1 said 893-897) |
| Xorg `glxcmds.c:1005`, `:2370` | verified |
| libepoxy `dispatch_glx.c:134-144`, `dispatch_common.c:512` | **corrected** (rev 2 said 133-143) |
| libglvnd `utils_misc.c:100-117`, `:167-178`; `libglxmapping.c:654-655`, `:566-571` | **corrected** (rev 1 said `strtok_r` parsed this string) |
| Mesa `glxextensions.c:345-373` | **corrected** (rev 1 named `__glXExtensionBitIsEnabled`) |
| `libGLX_nvidia.so.0` offset `0xe5a10` | verified, and the trailing space in that literal noted |

## Review history

- rev 1 — 2026-08-07, initial draft.
- **Opus review round 1 — REJECT.** Five blocking findings: (B1) neither
  evidence table had a preserved artifact and the one row that did was
  confounded; (B2) the tool could not record the variable under test
  because the runner passed `--quiet`; (B3) stronger on-disk evidence
  from an earlier round was demoted to a footnote; (B4) "position, not
  identity" had no run that separated the two; (B5) the Design and
  Testing sections contradicted each other on what the regression test
  asserts, and test 2's stated rationale was false. Twelve non-blocking
  findings, mostly citation drift and three misnamed consumer parsers.
- rev 2 — 2026-08-07, all blocking findings addressed by re-measuring
  with artifacts preserved; all twelve minors applied.
- **Opus review round 2 — APPROVE WITH EDITS.** B1-B5 all confirmed
  resolved, with the reviewer independently reconstructing every
  injected string from the `.proxy` artifacts and checking the
  one-variable pairing programmatically (`01 == 02 + " "`,
  `03 == 04 + " "`, `05 == 04 + " GLX_YSERVER_DUMMY"`), confirming row
  02's `GLX_SGIX_fbconfig` is at index 22 of 25 and therefore genuinely
  not last, and compiling the proposed builder to check it passes borrow
  check and clippy. Ten minors, two of them factual (see "What changed
  in rev 3").
- rev 3 — 2026-08-07, all ten minors applied. **Approved for
  planning.**
