# GLX extension string terminator — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Terminate yserver's GLX extension string with a space, as Xorg
does, so `libGLX_nvidia` stops losing the final token and stops dropping
`GLX_ARB_get_proc_address` from the client extension string.

**Architecture:** One private builder,
`glx_extension_string(tfp_supported: bool) -> String` in
`crates/yserver-core/src/core_loop/process_request.rs`, feeds both reply
paths (opcode 19 `QUERY_SERVER_STRING`/`STRING_EXTENSIONS` and opcode 18
`QUERY_EXTENSIONS_STRING`). The fix changes that builder so the space is
applied when a chunk is appended rather than only between chunks, which
makes "the output ends in a terminator" true by construction. No
constant, no encoder and no call site changes.

**Tech Stack:** Rust (edition 2024 workspace), `cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo +nightly fmt`.

**Status:** approved — Opus plan review returned APPROVE WITH EDITS
(three blocking, four minor); all applied. The blocking ones were worth
the round: Task 2's original rename-the-builder diagnostic could not
work (six call sites after Task 1, so the crate simply fails to compile,
and the predicted error message would have come from Task 1's tests —
an implementer matching it would have checked the box having proved
nothing), and its `git checkout --` undo would have discarded the
implementer's own uncommitted edit. Separately, the hardware gate as
written would have `tee`'d over
`~/yserver-glx-logs/2026-08-07-epoxyprobe/` — the pre-fix baseline the
design cites and the "before" side of the gate's own comparison. That
directory is now copied to `-prefix-baseline` and `run-epoxyprobe.sh`
refuses to overwrite an existing output directory.

**Design doc:**
`docs/superpowers/specs/2026-08-07-glx-extension-string-terminator-design.md`
(rev 3, approved after two Opus review rounds). Read its "Problem" and
"Design" sections before Task 1 — in particular, the design deliberately
does **not** modify `crates/yserver-protocol/src/x11/glx.rs`.

## Global Constraints

**Read `AGENTS.md` and `CLAUDE.md` before starting** — `CLAUDE.md`
requires it of every agent on this repo. The rest is copied from them;
every task's requirements implicitly include this section.

- `cargo clippy --all-targets -- -D warnings` must be clean — this is
  exactly what CI runs (`AGENTS.md:12`).
- `cargo +nightly fmt` before committing (`AGENTS.md:13`).
- The full workspace test suite must stay green: **2254 tests passing**
  on this base, 0 failing.
- If Xorg deviates from the written spec, follow Xorg (`AGENTS.md:19`).
  This is the rule that licenses this whole change.
- `docs/status.md` is kept current (`AGENTS.md:6`).
- Commits must be **signed** (`git log --format=%G?` must show `G`). The
  repo is configured for SSH signing already; do not pass `-S` manually.
- **Stage files explicitly. Never `git add -A` or `git add .`** —
  `.opencode/`, `openspec/` and `wlcopy` are pre-existing untracked
  paths that must not be committed.
- **Never put a Claude session URL in a commit message** (`CLAUDE.md`).
- **Do not push, merge or open a PR.** The hardware gate below runs
  first, and `AGENTS.md:18` requires Ariel's explicit confirmation before
  a squash merge. Commit to the local branch and stop there.
- Base is branch `glx-extension-string-terminator` off `master` @
  `f4c0b60c`. All line numbers below are against that base and will
  drift as you edit; locate by symbol name, not by line.

## File Structure

Only two files change.

| file | responsibility | change |
|---|---|---|
| `crates/yserver-core/src/core_loop/process_request.rs` | the builder `glx_extension_string` (~line 11691) and its unit tests in the file's `mod tests` | Task 1 rewrites the builder and adds two tests; Task 2 repairs one existing test |
| `docs/status.md` | project status log, newest entry first under `## Where we are` | Task 3 adds one entry |

**Read but not modified:** `crates/yserver-protocol/src/x11/glx.rs` —
`SERVER_EXTENSIONS` (~line 201), `SGIX_FBCONFIG_EXTENSION`,
`TFP_EXTENSION`, `encode_string_reply` (~line 267). Leaving these alone
is a design decision, not an oversight: the terminator lives in exactly
one place, so no caller has to remember whether a given constant is
self-terminating.

---

### Task 1: Terminate the extension string

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` — the
  function `glx_extension_string` (~11691-11703)
- Test: same file, inside `mod tests`, next to the existing
  `glx_extension_string_includes_tfp_only_when_capable` (~57285)

**Interfaces:**
- Consumes: `yserver_protocol::x11::glx::{SERVER_EXTENSIONS,
  SGIX_FBCONFIG_EXTENSION, TFP_EXTENSION}` — all `&'static str`, all
  unchanged by this plan.
- Produces: `fn glx_extension_string(tfp_supported: bool) -> String`,
  signature unchanged. Its output now ends with `' '`. Task 2 depends on
  this function existing with this exact signature.

- [ ] **Step 1: Write the failing regression test**

Add to `mod tests` in `process_request.rs`, immediately after
`glx_extension_string_includes_tfp_only_when_capable`:

```rust
    /// Xorg writes a space after **every** enabled extension
    /// (`glx/extension_string.c:144-145`), so its GLX extension string
    /// always ends with `' '` before the NUL. yserver's did not, and
    /// `libGLX_nvidia` responds by losing the final token *and* dropping
    /// `GLX_ARB_get_proc_address` from the client extension string — which
    /// makes libepoxy abort and takes `kwin_x11` down with SIGABRT.
    ///
    /// Measured 2026-08-07; see
    /// `docs/superpowers/specs/2026-08-07-glx-extension-string-terminator-design.md`
    /// and `~/yserver-glx-logs/2026-08-07-terminator-evidence/`.
    #[test]
    fn glx_extension_string_is_space_terminated() {
        for tfp_supported in [false, true] {
            let s = glx_extension_string(tfp_supported);
            assert!(
                s.ends_with(' '),
                "GLX extension string must end with Xorg's space terminator; \
                 tfp_supported={tfp_supported} produced {s:?}"
            );
        }
    }
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cargo test -p yserver-core glx_extension_string_is_space_terminated -- --nocapture
```

Expected: **FAIL** on the `tfp_supported=false` iteration, with the
message showing a string ending `...GLX_EXT_libglvnd GLX_SGIX_fbconfig"`
— no trailing space. This is RED-as-assertion-failure, not a compile
error; if it compiles-and-passes, stop and re-read the builder, because
the fix is already present and something is wrong with your base.

- [ ] **Step 3: Rewrite the builder**

Replace the body of `glx_extension_string`. The existing in-body comment
naming the three VendorPrivate opcodes is **deliberately folded into the
doc comment** below rather than dropped — keep the opcode names:

```rust
/// Build the GLX extension string.  The base extensions are always
/// present; `GLX_EXT_texture_from_pixmap` is appended only when the
/// backend confirmed at init that it can export a BGRA8 dma-buf.
/// `GLX_SGIX_fbconfig` is always appended: `GetFBConfigsSGIX`,
/// `CreateContextWithConfigSGIX` and `CreateGLXPixmapWithConfigSGIX`
/// are fully dispatched (VendorPrivate arms) — advertise-after-implement.
///
/// **Every chunk is space-terminated, so the string ends with `' '`.**
/// This mirrors Xorg, which writes `' '` then `'\0'` after each enabled
/// extension (`glx/extension_string.c:144-145`). It is not cosmetic:
/// `libGLX_nvidia` loses the final token of an unterminated list — it
/// silently dropped the `GLX_SGIX_fbconfig` we advertise — and also
/// withholds `GLX_ARB_get_proc_address` from the client extension
/// string, which aborts libepoxy and crashes `kwin_x11`. Appending via
/// this closure keeps the invariant true for any extension added later.
fn glx_extension_string(tfp_supported: bool) -> String {
    let mut s = String::new();
    let mut push = |chunk: &str| {
        s.push_str(chunk);
        s.push(' ');
    };
    push(yserver_protocol::x11::glx::SERVER_EXTENSIONS);
    push(yserver_protocol::x11::glx::SGIX_FBCONFIG_EXTENSION);
    if tfp_supported {
        push(yserver_protocol::x11::glx::TFP_EXTENSION);
    }
    s
}
```

Note on the closure: it borrows `s` mutably, and the function then
returns `s`. This is fine under NLL — the borrow ends at the closure's
last use. It was compiled and clippy-checked during design review. If
clippy does object on your toolchain, the equivalent without a closure
is acceptable and behaviourally identical:

```rust
        let mut s = String::new();
        for chunk in [
            Some(yserver_protocol::x11::glx::SERVER_EXTENSIONS),
            Some(yserver_protocol::x11::glx::SGIX_FBCONFIG_EXTENSION),
            tfp_supported.then_some(yserver_protocol::x11::glx::TFP_EXTENSION),
        ]
        .into_iter()
        .flatten()
        {
            s.push_str(chunk);
            s.push(' ');
        }
        s
```

- [ ] **Step 4: Run the test and confirm it passes**

```bash
cargo test -p yserver-core glx_extension_string_is_space_terminated -- --nocapture
```

Expected: **PASS**, both iterations.

- [ ] **Step 5: Add the non-regression guard test**

This one is **green before and after** the fix — it is not a driver of
the change. It exists so the terminator work cannot silently drop,
duplicate or double-space a token. Add it directly below the test from
Step 1:

```rust
/// Companion to `glx_extension_string_is_space_terminated`: the
/// terminator must not come at the cost of the token list. Green both
/// before and after that fix — this is a guard, not a regression test.
#[test]
fn glx_extension_string_tokens_match_advertised_constants() {
    use yserver_protocol::x11::glx as g;
    for tfp_supported in [false, true] {
        let s = glx_extension_string(tfp_supported);
        assert!(
            !s.starts_with(' '),
            "no leading separator; tfp_supported={tfp_supported} produced {s:?}"
        );
        assert!(
            !s.contains("  "),
            "no doubled separator; tfp_supported={tfp_supported} produced {s:?}"
        );
        let mut expected: Vec<&str> = g::SERVER_EXTENSIONS.split_whitespace().collect();
        expected.push(g::SGIX_FBCONFIG_EXTENSION);
        if tfp_supported {
            expected.push(g::TFP_EXTENSION);
        }
        let tokens: Vec<&str> = s.split_whitespace().collect();
        assert_eq!(
            tokens, expected,
            "token list must be the advertised constants, in order; \
             tfp_supported={tfp_supported}"
        );
    }
}
```

- [ ] **Step 6: Run both new tests plus the existing builder test**

```bash
cargo test -p yserver-core glx_extension_string -- --nocapture
```

Expected: **4 passed** —
`glx_extension_string_is_space_terminated`,
`glx_extension_string_tokens_match_advertised_constants`,
`glx_extension_string_includes_tfp_only_when_capable`,
`glx_extension_string_contains_sgix_fbconfig` (the last one still passes
for the wrong reason; Task 2 fixes that).

- [ ] **Step 7: Run the full workspace suite**

```bash
cargo test --workspace 2>&1 | grep -E "^test result"
```

Expected: **0 failed** across every crate, and the total passing count is
**2256** (2254 on the base plus the two new tests). If any test fails,
stop: something depends on the old unterminated string, which the design
said nothing does, and that is a finding worth reporting rather than
patching around.

- [ ] **Step 8: Lint and format**

```bash
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt
git diff --stat
```

Expected: clippy clean; `fmt` may reflow the code you added.

- [ ] **Step 9: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "fix(glx): terminate the extension string with a space, as Xorg does

libGLX_nvidia loses the final token of an unterminated GLX extension
list and additionally withholds GLX_ARB_get_proc_address from the
client extension string. The first cost us the GLX_SGIX_fbconfig we
do advertise; the second aborts libepoxy and kills kwin_x11 with
SIGABRT.

Xorg writes a space after every enabled extension
(glx/extension_string.c:144-145). Apply the terminator when a chunk is
appended so the invariant holds for extensions added later."
git log --format='%G? %s' -1
```

Expected: `G` and the subject line.

---

### Task 2: Repair the self-mirroring `GLX_SGIX_fbconfig` test

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` —
  `glx_extension_string_contains_sgix_fbconfig` (~58998, locate by name)

**Interfaces:**
- Consumes: `glx_extension_string` from Task 1.
- Produces: nothing new.

**Why this is its own task:** the test does not call
`glx_extension_string` at all. Its body says so verbatim — *"glx_extension_string()
is a private fn; mirror its logic here"* — and then re-implements the
builder and asserts against its own copy. It would pass if the function
were deleted, and it passed all through the defect this plan fixes. A
reviewer can reasonably accept Task 1 and reject this, or the reverse.

- [ ] **Step 1: Read the existing test**

```bash
grep -n "fn glx_extension_string_contains_sgix_fbconfig" -A 16 \
  crates/yserver-core/src/core_loop/process_request.rs
```

Confirm it builds its own `String` from `SERVER_EXTENSIONS` +
`SGIX_FBCONFIG_EXTENSION` and never calls the real builder.

Step 1's `grep` is the whole diagnosis: the test never names the
builder. Do not try to prove it by renaming the function — after Task 1
there are six references to `glx_extension_string` in this file (the
definition, two production call sites at ~12595 and ~12624, and three
tests), the crate would simply fail to compile, and a compile error
cannot distinguish which test is wired to the real code.

- [ ] **Step 2: Replace the body so it exercises the real builder**

```rust
    /// After both dispatch and extension-string advertisement are live,
    /// the computed GLX extension string must advertise
    /// `GLX_SGIX_fbconfig` as a whole token, under both TFP settings.
    ///
    /// This test used to re-implement `glx_extension_string`'s body and
    /// assert against its own copy, so it would have passed even if the
    /// builder were deleted. It calls the real function now.
    #[test]
    fn glx_extension_string_contains_sgix_fbconfig() {
        use yserver_protocol::x11::glx as x11glx;
        for tfp_supported in [false, true] {
            let s = glx_extension_string(tfp_supported);
            assert!(
                s.split_whitespace()
                    .any(|token| token == x11glx::SGIX_FBCONFIG_EXTENSION),
                "glx_extension_string must advertise GLX_SGIX_fbconfig as a \
                 whole token; tfp_supported={tfp_supported} produced {s:?}"
            );
        }
    }
```

Note `split_whitespace().any(|t| t == …)` rather than `contains(…)`:
whole-token matching is the point, since the defect this plan fixes was
precisely a token that a substring scan would still have "found".

- [ ] **Step 3: Run it**

```bash
cargo test -p yserver-core glx_extension_string_contains_sgix_fbconfig -- --nocapture
```

Expected: **PASS**.

- [ ] **Step 4: Prove the test is now wired to the real builder**

A behavioural mutation, not a rename — it is specific to this test and
undoes in one edit. Temporarily change the builder's body to
`String::new()`, leaving its name and signature alone, then:

```bash
cargo test -p yserver-core glx_extension_string_contains_sgix_fbconfig -- --nocapture
```

Expected: **FAIL** on the assertion. Before this task the same mutation
left it passing, which is exactly the defect being repaired.

Now undo that one-line mutation **by hand**. Do **not** run
`git checkout -- crates/yserver-core/src/core_loop/process_request.rs`:
Task 1 is already committed, so that would revert the file to
post-Task-1 HEAD and silently discard your Step 2 edit.

- [ ] **Step 5: Full suite, lint, format**

```bash
cargo test --workspace 2>&1 | grep -E "^test result"
cargo clippy --all-targets -- -D warnings
cargo +nightly fmt
```

Expected: `0 failed` on every line and the total still **2256** — this
task adds no tests. **If you see fewer than 7 `test result` lines the
build failed**; read the full output rather than trusting the filter.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "test(glx): make the SGIX_fbconfig test call the real builder

glx_extension_string_contains_sgix_fbconfig re-implemented the
builder's body in the test and asserted against its own copy, so it
would have passed if the function were deleted -- and it did pass
throughout the missing-terminator defect. Call the real function, and
match whole tokens rather than substrings."
git log --format='%G? %s' -1
```

---

### Task 3: Record it in `docs/status.md`

**Files:**
- Modify: `docs/status.md` — insert as the newest entry under
  `## Where we are`, above the `2026-08-06 GLX_VENDOR_NAMES_EXT` entry

**Interfaces:** none.

- [ ] **Step 1: Find the insertion point**

```bash
grep -n "^## Where we are" -A 4 docs/status.md
```

The newest entry is first. Insert immediately after the `## Where we
are` heading and its blank line, before the `2026-08-06` bullet.

- [ ] **Step 2: Write the entry**

Match the surrounding style: a bolded date + title, then prose.

```markdown
- **2026-08-07 GLX extension string is space-terminated, as Xorg's is:**
  `glx_extension_string` joined tokens with separating spaces and ended
  the string at the last token. Xorg writes `' '` then `'\0'` after
  *every* enabled extension (`glx/extension_string.c:144-145`), so its
  string always ends with a space. `libGLX_nvidia` loses the final token
  of an unterminated list — which silently cost us the
  `GLX_SGIX_fbconfig` we do advertise — and additionally withholds
  `GLX_ARB_get_proc_address` from the client extension string, so
  libepoxy aborts and `kwin_x11` dies with SIGABRT. Mesa's libGLX
  tokenises correctly and never noticed, which is why this read as an
  NVIDIA problem for four days. The builder now terminates every chunk,
  so the invariant survives future extensions. Design (rev 3, two Opus
  review rounds) at
  `docs/superpowers/specs/2026-08-07-glx-extension-string-terminator-design.md`;
  evidence, six controlled rows with preserved artifacts, in
  `~/yserver-glx-logs/2026-08-07-terminator-evidence/`. **Defect B, the
  `GLX_EXT_texture_from_pixmap` suppression on NVIDIA, remains open** —
  KWin may now reach `glXBindTexImageEXT` and fail there instead.
```

- [ ] **Step 3: Sanity-check the file survived**

```bash
grep -c "" docs/status.md
```

Expected: **5810 + the lines you added** (the base is 5810). If the count
dropped, you truncated the file — `git checkout -- docs/status.md` and
redo. This check is here because that exact accident happened on this
file during the defect-A merge.

- [ ] **Step 4: Commit**

```bash
git add docs/status.md
git commit -m "docs(glx): record the extension-string terminator fix"
git log --format='%G? %s' -1
```

---

## Hardware gate — run after Task 3, before any PR

Not a task: it needs a real KMS session and cannot run from the desktop.
**Ariel runs this from tty2.**

```bash
cd ~/Projects/yserver && git checkout glx-extension-string-terminator
kwriteconfig6 --file kwinrc --group Compositing --key LastFailureTimestamp --delete
~/yserver-glx-probes/defect-d/run-epoxyprobe.sh
```

The pre-fix baseline is preserved at
`~/yserver-glx-logs/2026-08-07-epoxyprobe-prefix-baseline/` — that is the
"before" side of this comparison and it is what the design cites for the
`QUERY_CONTEXT` finding. `run-epoxyprobe.sh` now derives its output
directory from the date and branch and **refuses to start if that
directory already exists**, so a re-run cannot overwrite evidence. The
earlier version hardcoded one path and `tee`'d over it, which would have
destroyed the baseline on first use.

**Pass condition**, NVIDIA vendor: `get_proc_address=SI` and
`CON EL SCREEN CORRECTO: la extension ESTA`. The Mesa control must be
unchanged from its current `SI`.

**Then** a Plasma session, to see how far KWin gets. Deleting
`LastFailureTimestamp` first is mandatory: KWin writes it on any
compositing failure and afterwards refuses to attempt GL at all, so
without deleting it you measure inertia rather than the change.

**Expected partial success.** Defect B is untouched, so KWin may now
abort at `glXBindTexImageEXT` instead of `glXGetProcAddressARB`. That is
progress, not a regression of this change, and it is the next thing to
work on — not a reason to revert this one.

---

## Self-review

**Spec coverage.** Design §"Design" → Task 1 Step 3. §"Both reply paths
get it" → no work needed; both call sites already route through the one
builder, and Task 1 Step 7's full-suite run is what proves nothing else
broke. §"Testing" tests 1 and 2 → Task 1 Steps 1 and 5. §"Testing"'s
repair of `glx_extension_string_contains_sgix_fbconfig` → Task 2.
§"Documentation" → Task 3. §"Testing" hardware gate → the gate section.
§"Empty-list edge case" → deliberately not implemented, per the design;
no task. §"Deferred" items → out of scope by design; no tasks. Scope
line "`glx.rs` is not modified" → honoured; no task touches it.

**Placeholder scan.** No TBDs. Every code step carries the literal code.
Task 2 repeats the assertion style rather than saying "like Task 1".

**Type consistency.** `glx_extension_string(bool) -> String` is used with
that exact signature in Tasks 1 and 2. Constant names
`SERVER_EXTENSIONS`, `SGIX_FBCONFIG_EXTENSION`, `TFP_EXTENSION` are
spelled identically in the builder, both new tests and the repaired
test, and match `glx.rs`.
