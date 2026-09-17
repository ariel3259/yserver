# Stage 2c-i debt, session 1 — refusal proof (tests only) Implementation Plan

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. Execute tasks in order, one at a time; tick steps (`- [ ]` → `- [x]`) only with the evidence each names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. Steps marked **[H]** need GPU and DRM access, which this sandbox does not have (see *Execution split*): at an [H] step, stop and hand off. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox (probed: `Read-only file system` on `~/Projects/yserver/.git`). At each "hand off for commit" step, stop with the tree dirty; the coordinating session verifies and commits with the message given.

**Goal:** Prove every session-1 refusal guard of the stage 2c-i resource service with a test that fails when that guard is deleted, and ship the census that measures it.

**Architecture:** A committed mutation-census tool (`tools/guard-census.py`) neutralises each guard and runs the `c0_2ci` suite; a guard counts as proven only when a test **tagged** for it fails with that guard's own assertion marker. New tests live in one focused module, `resources/guard_tests.rs`, one family per task. No production code changes in this session.

**Tech Stack:** Rust (`cargo test`, `ash`, the existing `resources` test helpers), Python 3 for the tool.

**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (revision 3 + part 3; `dfdb32d1`, `7e85536d`). Sections 2, 3 and 5 govern this plan. Session 2 and part 3 get their own plans after this session lands.

## Global Constraints

- **R8:** nothing built here is production-active. No production code change in this session.
- **F3:** do not mock `ResourceService`; tests drive the real service.
- **F8:** if a test cannot pass because the code is wrong, stop, report the exact guard and failure, and leave that family open. Never change a mechanism inside this session.
- **R12:** hardware tests use a `_vulkan`/`_drm` suffix and `#[ignore]`, and panic on a missing device. None of this session's tests need hardware.
- Every new test name starts with `c0_2ci_` (the census runs `cargo test -p yserver --lib c0_2ci`).
- Every guard assertion's message contains `[census:<MARKER>]`, and the test carries the matching `/// census:` tag (format in Task 1).
- Guards are identified by **file + function + condition text (+ occurrence)**, never by line number.
- Gate before each commit: `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`; `cargo test -p yserver --lib c0_2ci`.
- The implementer does not run `git commit`, `git add`, `git checkout`, `git stash` or `rm -f` (the sandbox's git directory is read-only, and codex's command policy rejects `rm -f`). The coordinating session commits every task after verifying it, using the message in the task, whose trailer records provenance: `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)`. Never a session URL in a commit message.
- Nobody pushes, squashes, rebases or amends.

## Execution split

The census runs the `c0_2ci` suite with `--include-ignored`, which includes 18 hardware tests (real DRM nodes, a real Vulkan ICD). **Codex's `workspace-write` sandbox has neither**, probed on 2026-09-16: `/dev/dri` does not exist, the `_drm` test panics and the `_vulkan` test reports `environmental skip: no live Vulkan ICD available`. The tool's green-baseline precondition therefore refuses a full census there — correctly, since otherwise every mutation would look caught.

So:

- **The implementer** writes the tool, the tests and the spec amendments, runs every non-hardware gate, and proves each family's oracle with `tools/guard-census.py --deterministic-only --require-oracle`. That mode runs the suite without `--include-ignored` and checks **only tagged sites**; it is valid there because every session-1 test is deterministic, and it is never valid for counting survivors.
- **The coordinating session** (which has GPU and DRM access) runs the steps marked **[H]**: the fidelity and baseline censuses of Task 1, and the full acceptance census and hardware gate of Task 8. It commits their transcripts, then hands back.

Do not work around an [H] step by dropping `--include-ignored` from a full census.

## Scope corrections to the spec (applied in Task 1)

Found while writing this plan; the spec is amended in Task 1's commit so plan and spec agree:

1. **Family B loses `consume_owner_write`.** Testing "refuses unless Owner" needs a grant issued *in* Owner, and today Owner is reached only through `issue_handover_permit` with the empty `WriterCoverageProof::new_for_tests()` / `RecipientReservation::new_for_tests()` that session 2 (spec §4.4) reshapes. Written now, the test would be rewritten there. It moves to session 2 with family A. **Session 1 is 27 guards.**
2. **Family C wording.** Spec §3.1 says `consume` "re-arms the direct role". The two `consume` guards are in `CompletionRetired`: one returns the error when moving the old `Current` into its reserved retirement slot fails, the other when moving the new `Submitted` into `Current` fails. Nothing is re-armed.
3. **Session-1 acceptance (spec §5.1).** §5.1 requires "zero survivors over the enumerated baseline of section 2", but that baseline includes session 2's guards; it was written before the stage was split. Amended so session 1 is accepted with all 27 of its guards proven by oracle and exactly the eight session-2 survivors left.
4. **Guard 437's observable.** `on_available` checks `transition_error` twice. Deleting the first `return` still returns the same error at the second, so "returns `Err`" does not prove it. What the first guard protects is that **rejected resources are not processed after a releasing-half error**; that is its test's assertion.

## File Structure

- Create: `tools/guard-census.py` — the census (enumerate, mutate, run, classify, oracle).
- Create: `crates/yserver/src/kms/render/resources/guard_tests.rs` — all session-1 guard tests, grouped by family.
- Modify: `crates/yserver/src/kms/render/resources/mod.rs` — declare `#[cfg(test)] mod guard_tests;` beside the existing test modules (lines 14–17).
- Create: `docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-baseline.md` (Task 1) and `…-census-session-1.md` (Task 8).
- Modify: `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (Task 1, corrections above; Task 8, status).

---

### Task 1: The census tool, the spec amendments, and the baseline

**Files:**
- Create: `tools/guard-census.py`
- Modify: `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`
- Create ([H], coordinator): `docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-baseline.md`

**Interfaces:**
- Produces: `tools/guard-census.py [--files …] [--fn SUBSTR] [--list] [--legacy-enumeration] [--json PATH] [--require-oracle] [--deterministic-only]`.
- Produces the **tag format** every later task uses, in the test's doc comment, directly above its `fn` (other doc lines and attributes may sit between):
  ```
  /// census: <MARKER> <file>.rs <fn> `<condition>`
  /// census: <MARKER> <file>.rs <fn> `<condition>` #<n>
  ```
  `<condition>` is the guard's condition with whitespace collapsed — for an `if` guard, the text between `if ` and ` {`; for a refusing match arm, the pattern followed by ` =>`. `#<n>` is the 1-based occurrence among identical conditions in the same function, omitted when unique there. The guard's assertion message must contain `[census:<MARKER>]`.
- **Binding rules, enforced by the tool:** every tag is bound to the `fn` directly below it; a marker appears once; a site is tagged at most once. One site, one marker, one test.
- **Preconditions:** every run executes the suite once unmutated and refuses unless it compiles and is green. A full census (authoritative, attributable to a commit) also refuses uncommitted changes in its target files; a `--deterministic-only` oracle check does not, because it is never authoritative and must run over an implementer's uncommitted edits, which restoring from memory preserves.
- **Mutation strategies**, tried in order until one compiles: an `if` guard's condition forced to `false`; only its refusal `return` replaced by a use of the payload (for `if let` guards whose bindings the body uses); its whole body replaced the same way (when statements before the `return` move a value the function still needs). A refusing match arm becomes `=> Ok(())`. A site no strategy compiles is `A_MANO` and never counts.
- **Verdicts:** `SURVIVES`; `CAUGHT` (untagged site); `CAUGHT_BY_ORACLE` (the site's bound test failed and its failure carries the site's marker); `CAUGHT_NOT_BY_ORACLE` (tagged, but its own test did not fail with its marker); `CAUGHT_WHOLE_BODY` (tagged and killed only under the whole-body strategy, which also deletes the statements before the refusal — **not** an oracle proof); `A_MANO`; `ORPHAN_TAG` (a tag names no enumerated site).

- [ ] **Step 1: Write the tool**

Create `tools/guard-census.py`:

```python
#!/usr/bin/env python3
"""Guard-clause mutation census for the stage 2c-i resource service.

Enumerates every refusal guard in the resource-service files, neutralises
each one in turn, runs the c0_2ci suite, and classifies the guard. With tags
(see the plan's Task 1), a guard counts as proven only when the test the tag
is bound to fails, carrying the guard's own [census:MARKER].

Spec: docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md,
sections 2, 3.0 and 5.1.

This mutates source files and runs cargo. A full census refuses to start if
the target files have uncommitted changes; every run refuses if the unmutated
suite is not green; and it
restores every file it touches from its original contents held in memory --
not with git, which a sandboxed implementer may not be able to write to --
even on failure or Ctrl-C. It is a developer
tool, not a CI step. A full run over the four default files takes about an
hour and needs GPU and DRM access for the hardware tests; see
--deterministic-only for what can run without them.
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RESOURCES = ROOT / "crates/yserver/src/kms/render/resources"
DEFAULT_FILES = ["mod.rs", "commit.rs", "gpu.rs", "transport.rs"]
BASE_CMD = ["cargo", "test", "-p", "yserver", "--lib", "c0_2ci"]

REFUSAL = re.compile(r"\breturn (Err\(|false\b|None\b)")
ARM = re.compile(r"^(\s*)(.+?)\s*=>\s*Err\((.+)\),\s*$")
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?fn\s+(\w+)")
TAG = re.compile(
    r"^\s*///\s*census:\s*(\S+)\s+(\S+\.rs)\s+(\w+)\s+`([^`]+)`(?:\s+#(\d+))?\s*$"
)


def norm(text):
    return " ".join(text.split())


def enclosing_fn(lines, idx):
    for j in range(idx, -1, -1):
        m = FN.match(lines[j])
        if m:
            return m.group(1)
    return "?"


def sites(path, legacy):
    """Return (lines, sites). `legacy` reproduces the 2026-09-15/16 census,
    which did not recognise `} else if` guards."""
    if_start = re.compile(r"^\s*if " if legacy else r"^\s*(?:\}\s*else\s+)?if ")
    lines = path.read_text().split("\n")
    found, seen = [], set()
    for i, line in enumerate(lines):
        m = ARM.match(line)
        if m and "return" not in line:
            found.append(dict(kind="arm", start=i, end=i, ret=i,
                              fn=enclosing_fn(lines, i), cond=norm(m.group(2)) + " =>"))
            continue
        if not REFUSAL.search(line):
            continue
        ind = len(line) - len(line.lstrip())
        for j in range(i - 1, max(-1, i - 12), -1):
            lj = lines[j]
            if not lj.strip():
                continue
            if len(lj) - len(lj.lstrip()) < ind and if_start.match(lj):
                k = j
                while k < len(lines) and not lines[k].rstrip().endswith("{"):
                    k += 1
                    if k - j > 8:
                        k = None
                        break
                if k is not None and j not in seen:
                    seen.add(j)
                    cond = norm(" ".join(lines[j:k + 1]))
                    cond = re.sub(r"^\}?\s*(else\s+)?if\s+", "", cond)
                    cond = re.sub(r"\s*\{$", "", cond)
                    found.append(dict(kind="if", start=j, end=k, ret=i,
                                      fn=enclosing_fn(lines, j), cond=cond))
                break
    totals = {}
    for s in found:
        totals[(s["fn"], s["cond"])] = totals.get((s["fn"], s["cond"]), 0) + 1
    running = {}
    for s in found:
        key = (s["fn"], s["cond"])
        running[key] = running.get(key, 0) + 1
        s["occ"] = running[key] if totals[key] > 1 else None
    return lines, found


def ident(fname, s):
    return f'{fname} {s["fn"]} `{s["cond"]}`' + (f' #{s["occ"]}' if s["occ"] else "")


def mutate_if_false(lines, s):
    ind = len(lines[s["start"]]) - len(lines[s["start"]].lstrip())
    prefix = "} else if false {" if lines[s["start"]].lstrip().startswith("}") else "if false {"
    return lines[:s["start"]] + [" " * ind + prefix] + lines[s["end"] + 1:]


def _payload(line):
    m = re.search(r"return\s+(.*);\s*$", line)
    if not m:
        return None
    inner = re.fullmatch(r"Err\((.*)\)", m.group(1))
    return inner.group(1) if inner else m.group(1)


def mutate_swallow(lines, s):
    """Neutralise only the refusal `return`, keeping the guard's bindings used."""
    payload = _payload(lines[s["ret"]])
    if payload is None:
        return None
    r = s["ret"]
    ind = len(lines[r]) - len(lines[r].lstrip())
    return lines[:r] + [" " * ind + f"let _ = &({payload});"] + lines[r + 1:]


def body_range(lines, s):
    """[first, close): the guard block's inner lines, by brace counting from
    the opening `{` that ends line s["end"]."""
    depth = 1
    for i in range(s["end"] + 1, len(lines)):
        for ch in lines[i]:
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return s["end"] + 1, i
    return None


def mutate_swallow_body(lines, s):
    """Replace the whole guard body with a use of the refusal payload. Needed
    when statements before the `return` move a value the function still uses
    (commit.rs discharge_commit_kms_obligations' validate-all guard). It also
    deletes those statements, so a kill under this strategy never counts as an
    oracle proof: see CAUGHT_WHOLE_BODY."""
    rng = body_range(lines, s)
    payload = _payload(lines[s["ret"]])
    if rng is None or payload is None:
        return None
    first, close = rng
    ind = len(lines[s["ret"]]) - len(lines[s["ret"]].lstrip())
    return lines[:first] + [" " * ind + f"let _ = &({payload});"] + lines[close:]


def mutate_arm(lines, s):
    i = s["start"]
    return lines[:i] + [re.sub(r"=>\s*Err\(.+\),\s*$", "=> Ok(()),", lines[i])] + lines[i + 1:]


def run_suite(deterministic_only):
    cmd = BASE_CMD if deterministic_only else BASE_CMD + ["--", "--include-ignored"]
    p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
    out = p.stdout + p.stderr
    if "test result:" not in out:
        return None, out  # did not compile: nothing ran
    return re.findall(r"^test (\S+) \.\.\. FAILED$", out, re.M), out


def failure_block(out, test):
    m = re.search(r"^---- " + re.escape(test) + r" stdout ----\n(.*?)(?=^---- |^failures:$|\Z)",
                  out, re.M | re.S)
    return m.group(1) if m else ""


def load_tags():
    """Bind every tag to the fn directly below it (past doc comments and
    attributes). Returns (by_marker: marker -> (site, test_fn), by_site: site
    -> marker). Refuses unbound tags, duplicate markers, and sites tagged twice:
    one site, one marker, one test."""
    by_marker, by_site = {}, {}
    for path in (ROOT / "crates/yserver/src").rglob("*.rs"):
        lines = path.read_text().split("\n")
        for i, line in enumerate(lines):
            m = TAG.match(line)
            if not m:
                continue
            marker, fname, fn, cond, occ = m.groups()
            test_fn = None
            for j in range(i + 1, min(i + 40, len(lines))):
                stripped = lines[j].strip()
                fm = re.match(r"(?:pub(?:\([^)]*\))?\s+)?fn\s+(\w+)\s*\(", stripped)
                if fm:
                    test_fn = fm.group(1)
                    break
                if not (stripped.startswith("///") or stripped.startswith("#[") or not stripped):
                    break
            if test_fn is None:
                sys.exit(f"REFUSING: census tag {marker} in {path} is not directly above a fn")
            site = (fname, fn, norm(cond), int(occ) if occ else None)
            if marker in by_marker:
                sys.exit(f"REFUSING: duplicate census marker {marker}")
            if site in by_site:
                sys.exit(f"REFUSING: site {site} is tagged twice ({by_site[site]}, {marker})")
            by_marker[marker] = (site, test_fn)
            by_site[site] = marker
    return by_marker, by_site


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--files", nargs="*", default=DEFAULT_FILES)
    ap.add_argument("--fn", dest="fn_filter", help="only sites whose enclosing function contains this")
    ap.add_argument("--list", action="store_true", help="print site identities and exit")
    ap.add_argument("--legacy-enumeration", action="store_true",
                    help="reproduce the published census (no `} else if` guards)")
    ap.add_argument("--json", type=pathlib.Path, help="write results as JSON")
    ap.add_argument("--require-oracle", action="store_true",
                    help="exit 1 unless every tagged site in scope is CAUGHT_BY_ORACLE and no tag is orphaned")
    ap.add_argument("--deterministic-only", action="store_true",
                    help="run without --include-ignored and check only tagged sites' oracles; "
                         "requires --require-oracle. Never valid for survivor accounting.")
    args = ap.parse_args()
    if args.deterministic_only and not args.require_oracle:
        sys.exit("REFUSING: --deterministic-only only proves tagged oracles; pass --require-oracle.")

    by_marker, by_site = load_tags()
    rels = [str((RESOURCES / f).relative_to(ROOT)) for f in args.files]
    if not args.list:
        # A full census is authoritative evidence and must be attributable to a
        # commit, so it requires the target files clean. A --deterministic-only
        # oracle check is never authoritative and runs while an implementer's
        # edits are uncommitted; restoring from memory preserves those edits.
        if not args.deterministic_only and subprocess.run(
                ["git", "diff", "--quiet", "--", *rels], cwd=ROOT).returncode:
            sys.exit("REFUSING: a full census must run on committed target files; "
                     "commit first, or use --deterministic-only --require-oracle for an oracle check.")
        # A mutation counts as caught when a test fails. If the unmutated suite
        # already fails -- typically hardware tests run where there is no GPU or
        # DRM access, e.g. inside a sandbox -- every mutation would look caught,
        # survivors included, with no warning. Require a green baseline.
        failed, out = run_suite(args.deterministic_only)
        if failed is None:
            sys.exit("REFUSING: the unmutated suite does not compile.")
        if failed:
            sys.exit("REFUSING: the unmutated suite already fails (" + ", ".join(failed[:5]) + "); "
                     "every mutation would look caught. Hardware tests need GPU and DRM access; "
                     "without them use --deterministic-only --require-oracle.")

    matched, results = set(), []
    for fname in args.files:
        path = RESOURCES / fname
        original = path.read_text()
        lines, found = sites(path, args.legacy_enumeration)
        for s in found:
            site = (fname, s["fn"], s["cond"], s["occ"])
            marker = by_site.get(site)
            if marker:
                matched.add(site)
            if args.fn_filter and args.fn_filter not in s["fn"]:
                continue
            if args.list:
                print(ident(fname, s))
                continue
            if args.deterministic_only and not marker:
                continue
            test_fn = by_marker[marker][1] if marker else None
            strategies = ([mutate_arm] if s["kind"] == "arm"
                          else [mutate_if_false, mutate_swallow, mutate_swallow_body])
            verdict, killers, used = "A_MANO", [], None
            try:
                for strategy in strategies:
                    mutated = strategy(lines, s)
                    if mutated is None:
                        continue
                    path.write_text("\n".join(mutated))
                    failed, out = run_suite(args.deterministic_only)
                    if failed is None:
                        continue
                    used = strategy.__name__
                    if not failed:
                        verdict = "SURVIVES"
                    elif not marker:
                        verdict = "CAUGHT"
                    else:
                        own = [t for t in failed if t.endswith("::" + test_fn)
                               and f"[census:{marker}]" in failure_block(out, t)]
                        if own and used == "mutate_swallow_body":
                            verdict = "CAUGHT_WHOLE_BODY"
                        elif own:
                            verdict = "CAUGHT_BY_ORACLE"
                        else:
                            verdict = "CAUGHT_NOT_BY_ORACLE"
                    killers = failed[:3]
                    break
            finally:
                path.write_text(original)
            results.append(dict(site=ident(fname, s), verdict=verdict, marker=marker,
                                test=test_fn, strategy=used, killers=killers))
            print(f"{verdict:<22} {ident(fname, s)}" + (f"  [{marker}]" if marker else ""), flush=True)

    if args.list:
        return 0
    for site, marker in by_site.items():
        if site[0] in args.files and site not in matched:
            results.append(dict(site=f"{site[0]} {site[1]} `{site[2]}`" + (f" #{site[3]}" if site[3] else ""),
                                verdict="ORPHAN_TAG", marker=marker, test=by_marker[marker][1],
                                strategy=None, killers=[]))
            print(f"{'ORPHAN_TAG':<22} {results[-1]['site']}  [{marker}]", flush=True)

    counts = {}
    for r in results:
        counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    print("\n=== SUMMARY ===")
    for v, n in sorted(counts.items()):
        print(f"{v}: {n}")
    if args.json:
        args.json.write_text(json.dumps(results, indent=1))
    if args.require_oracle:
        bad = [r for r in results if r["marker"] and r["verdict"] != "CAUGHT_BY_ORACLE"]
        if bad:
            print(f"\nFAIL: {len(bad)} tagged site(s) not proven by their own test's oracle")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

Make it executable: `chmod +x tools/guard-census.py`. Check: `python3 -m py_compile tools/guard-census.py` succeeds and `tools/guard-census.py --legacy-enumeration --list | wc -l` prints `67`.

- [ ] **Step 2: Amend the spec**

In `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`:
- §3.1 family **B**: remove the `consume_owner_write` sentence, set the count to 1, and add: "`consume_owner_write`'s non-Owner refusal moves to session 2 with family A (section 4.4): proving it requires a grant issued in Owner, reached today only through the handover tokens 4.4 reshapes."
- §3.1 family **C**: replace "`consume` propagates a recovered transition error and re-arms the direct role" with "`consume`, on `CompletionRetired`, returns the error when moving the old `Current` into its reserved retirement slot fails, and when moving the new `Submitted` into `Current` fails".
- §3.1 closing count: "That is 27 of the 35 survivors."
- §4.4: add `consume_owner_write`'s non-Owner guard after family A's five.
- §5.1 **Session 1**: replace its acceptance paragraph with: "**Session 1:** each of its 27 guards is `CAUGHT_BY_ORACLE` — killed by the test its tag is bound to, carrying its own marker, under a strategy other than whole-body replacement. A census over the legacy enumeration of section 2 (67 sites) then reports exactly eight survivors, all session-2 scope: `issue_handover_permit` (2) and `publish_owner` (3), `consume_owner_write`'s non-Owner check, and the error arms of `cancel_pre_submit_batch` and `freeze_uncertain_batch`. Guards the full enumeration finds beyond the legacy 67 are reported with their verdicts and are not accepted or rejected by session 1: their scope is a separate decision."

- [ ] **Step 3: Hand off for commit**

The coordinator commits, after verifying, with:

```bash
git add tools/guard-census.py docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md
git commit -m "tools: add the resource-service guard census; amend the debt spec for session 1

The census enumerates refusal guards, mutates each, and proves a guard
only when the test its tag is bound to fails with the guard's own marker.
Spec amendments: consume_owner_write moves to session 2, family C's
description is corrected, and session 1's acceptance is stated as 27
guards proven with exactly the eight session-2 survivors left.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

**Stop here and hand off.** The coordinator commits the tool and the spec amendments with the message above, then runs Steps 4–6 ([H]).

- [ ] **Step 4 [H]: Reproduce the published census**

The tool's acceptance test: it must reproduce what the spec's section 2 measured by hand.

Run: `tools/guard-census.py --legacy-enumeration --json /tmp/census-legacy.json` (about an hour)
Expected summary: `CAUGHT: 32`, `SURVIVES: 35`, nothing else.

If any site classifies differently from the spec's section 2.1 lists, **stop**: the tool is not faithful, and no later task may rely on it.

- [ ] **Step 5 [H]: Record what the legacy enumeration could not see**

Run: `diff <(tools/guard-census.py --legacy-enumeration --list) <(tools/guard-census.py --list)`
Expected: only added lines, each a `} else if` guard (one, in `commit.rs` `consume`, when this plan was written).

Run each new site with `--fn <its function>` and record its verdict. New survivors are reported, not added to session 1.

- [ ] **Step 6 [H]: Write the baseline finding and commit**

Create `docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-baseline.md` with the tool commit, the command lines, the legacy summary (must be 32/35 over 67), the `} else if` sites and their verdicts, and the statement that session 1's target is the 27 guards of Tasks 2–7. Commit it with the coordinator's trailer, then hand back to the implementer for Task 2.

---

### Task 2: Family E — cross-incarnation isolation (10 guards)

**Files:**
- Create: `crates/yserver/src/kms/render/resources/guard_tests.rs`
- Modify: `crates/yserver/src/kms/render/resources/mod.rs:14-17`

**Interfaces:**
- Consumes: `super::tests::{spy_service, SpyAllocation}`; `ResourceService::{register, freeze, cancel, validate_proof_target, record_kms_discharged, register_kms, apply_teardown_release, validate_gpu_batch}`; `RetainingSupervisor::{new, issue_teardown_release}`; `IncarnationId::next`.
- Produces: helpers `wrong_device(AllocationKey) -> AllocationKey`, `wrong_incarnation(AllocationKey) -> AllocationKey`, `spy(&mut ResourceService) -> AllocationLease`, `member() -> GroupMember` — used by Tasks 3–6.

**Why this assertion shape:** each guard is `key.device != self.device || key.incarnation != self.incarnation`. The existing `c0_2ci_stale_key_and_old_evidence_rejected` presents only a wrong *device*. Each test here presents both a wrong device and a wrong incarnation, and asserts exactly `WrongIncarnation`: with the guard deleted, the key finds no entry and the call returns `Detached`, so the assertion distinguishes the two.

- [ ] **Step 1: Declare the module**

In `crates/yserver/src/kms/render/resources/mod.rs`, after `pub(crate) mod tests;` (line 17), add:

```rust
#[cfg(test)]
mod guard_tests;
```

- [ ] **Step 2: Write the family-E tests**

Create `crates/yserver/src/kms/render/resources/guard_tests.rs`:

```rust
//! Stage 2c-i debt, session 1: one test per refusal guard the census found
//! unproven. Each guard's assertion carries `[census:<MARKER>]` and the test a
//! matching `/// census:` tag bound to the test, so `tools/guard-census.py --require-oracle`
//! can confirm the guard is killed by its own assertion. Spec:
//! docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md.

use std::{cell::Cell, num::NonZeroU32, rc::Rc};

use super::{
    tests::{SpyAllocation, spy_service},
    *,
};
// `CommitId` and `CrtcKey` are not re-exported by `resources`; `tests.rs`
// imports them explicitly too.
use crate::{
    kms::{
        owner::identity::{CommitId, IncarnationId},
        render::platform::CrtcKey,
    },
    platform::drm::DrmDeviceKey,
};

fn wrong_device(key: AllocationKey) -> AllocationKey {
    AllocationKey {
        device: DrmDeviceKey {
            major: key.device.major,
            minor: key.device.minor + 1,
        },
        ..key
    }
}

fn wrong_incarnation(key: AllocationKey) -> AllocationKey {
    AllocationKey {
        incarnation: key.incarnation.next(),
        ..key
    }
}

fn spy(service: &mut ResourceService) -> AllocationLease {
    service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::new(Cell::new(0)),
        }))
        .unwrap()
}

fn member() -> GroupMember {
    let crtc = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(NonZeroU32::new(10).unwrap()),
    );
    GroupMember::new(crtc, 1, 1)
}

/// A second service on another incarnation, holding one spy allocation with a
/// pending read obligation: the only way to obtain a lease whose key is
/// foreign to the first service, since `reserve`/`adopt` enforce identity.
fn foreign_read_lease(
    device: DrmDeviceKey,
    incarnation: IncarnationId,
) -> (ResourceService, AllocationLease, ObligationId) {
    let mut other = ResourceService::new(device, incarnation);
    let lease = spy(&mut other);
    let ob = other.register(lease.key(), ObligationKind::Read).unwrap();
    (other, lease, ob)
}

/// census: E-register mod.rs register `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_register_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.register(foreign, ObligationKind::Gpu),
            Err(ResourceError::WrongIncarnation),
            "register must refuse a foreign key [census:E-register]"
        );
    }
}

/// census: E-freeze mod.rs freeze `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_freeze_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.freeze(foreign),
            Err(ResourceError::WrongIncarnation),
            "freeze must refuse a foreign key [census:E-freeze]"
        );
    }
}

/// census: E-cancel mod.rs cancel `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_cancel_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.cancel(foreign, ob),
            Err(ResourceError::WrongIncarnation),
            "cancel must refuse a foreign key [census:E-cancel]"
        );
    }
}

/// census: E-validate-proof-target mod.rs validate_proof_target `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_validate_proof_target_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.validate_proof_target(foreign, ob),
            Err(ResourceError::WrongIncarnation),
            "validate_proof_target must refuse a foreign key [census:E-validate-proof-target]"
        );
    }
}

/// census: E-record-kms-discharged mod.rs record_kms_discharged `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_record_kms_discharged_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let commit = CommitId::for_tests(910);
    let ob = service.register_kms(held.key(), commit, member()).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.record_kms_discharged(foreign, ob, commit, member()),
            Err(ResourceError::WrongIncarnation),
            "record_kms_discharged must refuse a foreign key [census:E-record-kms-discharged]"
        );
    }
}

/// census: E-teardown-proof mod.rs apply_teardown_release `proof.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_teardown_release_refuses_foreign_proof() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    // Deleting the guard lets the valid key reach the frozen check, which
    // returns InvalidState, not WrongIncarnation.
    let proof = supervisor.issue_teardown_release(IncarnationId::first().next(), vec![held.key()]);
    assert_eq!(
        service.apply_teardown_release(proof),
        Err(ResourceError::WrongIncarnation),
        "teardown release must refuse a proof for another incarnation [census:E-teardown-proof]"
    );
}

/// census: E-teardown-key mod.rs apply_teardown_release `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_teardown_release_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        let proof = supervisor.issue_teardown_release(IncarnationId::first(), vec![foreign]);
        assert_eq!(
            service.apply_teardown_release(proof),
            Err(ResourceError::WrongIncarnation),
            "teardown release must refuse a foreign key [census:E-teardown-key]"
        );
    }
}

/// census: E-batch-entry mod.rs validate_gpu_batch `key.device != self.device || key.incarnation != self.incarnation` #1
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_entry() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
        batch.bind_ticket(GpuObligation::for_tests_stub(
            vec![(foreign, ob)],
            crate::kms::render::platform::FenceTicket::for_tests_stub(),
        ));
        assert_eq!(
            service.validate_gpu_batch(batch).err().map(|(e, _)| e),
            Some(ResourceError::WrongIncarnation),
            "a GPU batch entry with a foreign key must be refused [census:E-batch-entry]"
        );
    }
}

/// census: E-batch-read-source mod.rs validate_gpu_batch `key.device != self.device || key.incarnation != self.incarnation` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_read_source() {
    let (service, held, _drops) = spy_service();
    let key = held.key();
    let foreign_ids = [
        (wrong_device(key).device, key.incarnation),
        (key.device, key.incarnation.next()),
    ];
    for (device, incarnation) in foreign_ids {
        let (_other, lease, ob) = foreign_read_lease(device, incarnation);
        let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
        batch.bind_read_obligation(super::gpu::ReadObligation::new(lease, ob, None, None));
        assert_eq!(
            service.validate_gpu_batch(batch).err().map(|(e, _)| e),
            Some(ResourceError::WrongIncarnation),
            "a read obligation whose source is foreign must be refused [census:E-batch-read-source]"
        );
    }
}

/// census: E-batch-read-staging mod.rs validate_gpu_batch `s_key.device != self.device || s_key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_read_staging() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let source_ob = service.register(key, ObligationKind::Read).unwrap();
    let (_other, staging, staging_ob) = foreign_read_lease(key.device, key.incarnation.next());
    let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
    // The spy's own lease is the source: a fresh Read reservation beside its
    // Retain use is not something this test should depend on.
    batch.bind_read_obligation(super::gpu::ReadObligation::new(
        held,
        source_ob,
        Some(staging),
        Some(staging_ob),
    ));
    assert_eq!(
        service.validate_gpu_batch(batch).err().map(|(e, _)| e),
        Some(ResourceError::WrongIncarnation),
        "a read obligation whose staging lease is foreign must be refused [census:E-batch-read-staging]"
    );
}
```

- [ ] **Step 3: Run the tests; they pass**

These guards are correct today; the tests prove them, so they pass. The "red" is Step 4.

Run: `cargo test -p yserver --lib c0_2ci_guard_`
Expected: `10 passed; 0 failed`.

If a test fails, a guard does not behave as the spec states: that is an **F8 stop**. Report it; do not change `mod.rs`.

- [ ] **Step 4: Confirm the tags match enumerated sites, then run the oracle**

Run: `tools/guard-census.py --list | grep -E " (register|freeze|cancel|validate_proof_target|record_kms_discharged|apply_teardown_release|validate_gpu_batch) "`
Expected: every condition used in this task's tags appears verbatim. If a condition differs (spacing, `#n`), copy the listed text into the tag.

Run: `tools/guard-census.py --files mod.rs --deterministic-only --require-oracle --fn register` and the same with `--fn freeze`, `--fn cancel`, `--fn validate_proof_target`, `--fn record_kms_discharged`, `--fn apply_teardown_release`, `--fn validate_gpu_batch`.
Expected: every tagged site `CAUGHT_BY_ORACLE`, no `ORPHAN_TAG`, exit 0. Deterministic mode skips untagged sites, so guards later tasks cover do not appear yet.

- [ ] **Step 5: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`
Expected: fmt clean, clippy clean (remove any import clippy reports unused), `c0_2ci` all pass.

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/mod.rs crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove the ledger refuses foreign keys at every entry point

Family E of the stage 2c-i debt: ten identity guards across register,
freeze, cancel, validate_proof_target, record_kms_discharged,
apply_teardown_release and validate_gpu_batch's three check sets. Each
test presents both a wrong device and a wrong incarnation and asserts
WrongIncarnation, which a deleted guard turns into Detached. Proven by
tools/guard-census.py --deterministic-only --require-oracle.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 3: Family F — read-obligation validation (4 guards)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `spy` (Task 2); `ResourceService::{register, freeze, cancel, reserve, validate_gpu_batch}`; `super::gpu::ReadObligation::new`.

**Why:** `validate_gpu_batch` checks frozen and pending for the GPU entries (proven), the read source (`#2`) and the read staging lease. With a guard deleted, the batch validates and the call returns `Ok`, so asserting the exact refusal distinguishes them.

- [ ] **Step 1: Append the family-F tests**

```rust
fn read_batch(
    source: AllocationLease,
    source_ob: ObligationId,
    staging: Option<(AllocationLease, ObligationId)>,
) -> CoreRetirementBatch {
    let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
    let (staging_lease, staging_ob) = match staging {
        Some((lease, ob)) => (Some(lease), Some(ob)),
        None => (None, None),
    };
    batch.bind_read_obligation(super::gpu::ReadObligation::new(
        source,
        source_ob,
        staging_lease,
        staging_ob,
    ));
    batch
}

/// census: F-read-source-frozen mod.rs validate_gpu_batch `avail.frozen` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_frozen_read_source() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let ob = service.register(key, ObligationKind::Read).unwrap();
    service.freeze(key).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, ob, None))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::Frozen),
        "a read obligation on a frozen source must be refused [census:F-read-source-frozen]"
    );
}

/// census: F-read-source-pending mod.rs validate_gpu_batch `!avail.pending_obligations.contains_key(&obligation_id)` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_read_source_without_pending_obligation() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let ob = service.register(key, ObligationKind::Read).unwrap();
    service.cancel(key, ob).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, ob, None))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::InvalidProof),
        "a read obligation that is no longer pending must be refused [census:F-read-source-pending]"
    );
}

/// census: F-read-staging-frozen mod.rs validate_gpu_batch `s_avail.frozen`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_frozen_read_staging() {
    let (mut service, held, _drops) = spy_service();
    let source_ob = service.register(held.key(), ObligationKind::Read).unwrap();
    let staging = spy(&mut service);
    let staging_key = staging.key();
    let staging_ob = service.register(staging_key, ObligationKind::Read).unwrap();
    service.freeze(staging_key).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, source_ob, Some((staging, staging_ob))))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::Frozen),
        "a read obligation with a frozen staging lease must be refused [census:F-read-staging-frozen]"
    );
}

/// census: F-read-staging-pending mod.rs validate_gpu_batch `!s_avail.pending_obligations.contains_key(&staging_ob)`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_read_staging_without_pending_obligation() {
    let (mut service, held, _drops) = spy_service();
    let source_ob = service.register(held.key(), ObligationKind::Read).unwrap();
    let staging = spy(&mut service);
    let staging_key = staging.key();
    let staging_ob = service.register(staging_key, ObligationKind::Read).unwrap();
    service.cancel(staging_key, staging_ob).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, source_ob, Some((staging, staging_ob))))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::InvalidProof),
        "a staging obligation that is no longer pending must be refused [census:F-read-staging-pending]"
    );
}
```

- [ ] **Step 2: Run the tests; they pass**

Run: `cargo test -p yserver --lib c0_2ci_guard_gpu_batch_refuses_`
Expected: `7 passed` (Task 2's three plus these four). A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `tools/guard-census.py --files mod.rs --deterministic-only --require-oracle --fn validate_gpu_batch`
Expected: all seven tagged `validate_gpu_batch` sites `CAUGHT_BY_ORACLE`, exit 0. (The GPU-entry `frozen` and pending sites are already proven and untagged; deterministic mode skips them.)

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove GPU batch validation on the read-obligation path

Family F of the stage 2c-i debt: a read obligation is refused when its
source or its staging lease is frozen or holds no matching pending
obligation. Only the GPU-entry check set had coverage.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 4: Families G, H, I — exhaustion, file-owned adoption, teardown precondition (5 guards)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `ResourceService::{force_exhausted_for_tests, adopt, reserve, register, apply_teardown_release}`; `FileOwnedBacking::new`, `DrmCleanupRight::new`, `SharedBacking::mock`, `ScanoutAllocation::new` (the shapes used by `tests.rs`'s `c0_2ci_scanout_discharging_file_owned_leaves_shared_intact`).

**Why:** in `reserve` and `register` the exhaustion check follows the identity check, so a valid key isolates it. `force_exhausted_for_tests` exists because real exhaustion needs `u64::MAX` allocations. `FileOwnedBacking` has no `Drop`, so a refused payload drops without side effects.

- [ ] **Step 1: Append the tests**

```rust
/// census: G-adopt-exhausted mod.rs adopt_unchecked `self.exhausted`
#[test]
fn c0_2ci_guard_adopt_refuses_when_exhausted() {
    let (mut service, _held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    let result = service.adopt(AllocationPayload::Spy(SpyAllocation {
        drops: Rc::new(Cell::new(0)),
    }));
    assert!(
        matches!(result, Err((ResourceError::Exhausted, _))),
        "an exhausted service must refuse adoption [census:G-adopt-exhausted]"
    );
}

/// census: G-reserve-exhausted mod.rs reserve `self.exhausted`
#[test]
fn c0_2ci_guard_reserve_refuses_when_exhausted() {
    let (mut service, held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    assert_eq!(
        service.reserve(held.key(), UseKind::Read).err(),
        Some(ResourceError::Exhausted),
        "an exhausted service must refuse a reservation [census:G-reserve-exhausted]"
    );
}

/// census: G-register-exhausted mod.rs register `self.exhausted`
#[test]
fn c0_2ci_guard_register_refuses_when_exhausted() {
    let (mut service, held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    assert_eq!(
        service.register(held.key(), ObligationKind::Gpu),
        Err(ResourceError::Exhausted),
        "an exhausted service must refuse a new obligation [census:G-register-exhausted]"
    );
}

/// census: H-adopt-file-owned mod.rs adopt `payload.file_owned_alias_present()`
#[test]
fn c0_2ci_guard_adopt_refuses_live_file_owned_payload() {
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let right = DrmCleanupRight::new(device_key, IncarnationId::first(), 70, 71, GemOwner::Right);
    let file_owned = FileOwnedBacking::new(right, None, device).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let payload = AllocationPayload::Scanout(ScanoutAllocation::new(Some(file_owned), shared));
    let mut service = ResourceService::new(device_key, IncarnationId::first());
    assert!(
        matches!(service.adopt(payload), Err((ResourceError::InvalidState, _))),
        "adopt must refuse a payload with a live file-owned alias [census:H-adopt-file-owned]"
    );
}

/// census: I-teardown-requires-frozen mod.rs apply_teardown_release `!avail.frozen`
#[test]
fn c0_2ci_guard_teardown_release_refuses_unfrozen_entry() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    let proof = supervisor.issue_teardown_release(IncarnationId::first(), vec![held.key()]);
    assert_eq!(
        service.apply_teardown_release(proof),
        Err(ResourceError::InvalidState),
        "teardown release must refuse an entry that is not frozen [census:I-teardown-requires-frozen]"
    );
}
```

- [ ] **Step 2: Run the tests; they pass**

Run: `cargo test -p yserver --lib c0_2ci_guard_`
Expected: `19 passed`. A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `tools/guard-census.py --files mod.rs --deterministic-only --require-oracle --fn adopt` (matches `adopt` and `adopt_unchecked`), then the same with `--fn reserve`, `--fn register`, `--fn apply_teardown_release`.
Expected: every tagged site `CAUGHT_BY_ORACLE`, exit 0.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove ledger exhaustion, file-owned adoption and teardown precondition

Families G, H and I of the stage 2c-i debt: adopt, reserve and register
refuse once exhausted; adopt refuses a payload with a live file-owned
alias; apply_teardown_release refuses an unfrozen entry.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 5: Family C — consumer error propagation (4 guards)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `spy`; `CommitResourceConsumer::{new, consume, on_available, prereserve_retirement}` and its `pub(crate)` fields `releasing_resources`, `rejected_resources`, `released_presents`, `capacity`; `CommitResources`' `pub(crate)` field `allocations`; `RoleReservation::new_for_test(DirectRole, u64, Rc<Cell<bool>>)`; `CommitResources::{new, with_direct_role}`; `crate::kms::owner::ledger::Submitted::{new, accepted}`; `crate::kms::owner::device::OwnerEvent::CompletionRetired` (fully qualified: `OwnerEvent` is not re-exported by `resources`).

**Why:** a `RoleReservation` built with `new_for_test` is never reserved in the consumer's `DirectCapacity`, so `move_into_reserved`, `move_role` and `finish_role` reject it with `InvalidState` — the same technique as `tests.rs`'s `c0_2ci_capacity_on_available_error_restores_all_resources_safely`. Dropping an undischarged `RoleReservation` only sets its `closed` cell; it does not panic. These four sites need a swallow strategy: the `if let` bindings make `if false` uncompilable.

- [ ] **Step 1: Append the tests**

```rust
fn closed_cell() -> Rc<Cell<bool>> {
    Rc::new(Cell::new(false))
}

/// census: C-retire-move-into-reserved commit.rs consume `let Err((err, recovered)) = self.capacity.move_into_reserved(role, reserved)`
#[test]
fn c0_2ci_guard_completion_retired_returns_failed_move_into_reserved() {
    let (mut service, old_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    let commit = CommitId::for_tests(920);
    // Never reserved in the consumer's capacity: move_into_reserved rejects it.
    consumer.prereserve_retirement(
        commit,
        RoleReservation::new_for_test(DirectRole::OrdinaryRetirement, 998, closed_cell()),
    );
    let old = CommitResources::new(vec![old_alloc], None, None, None, vec![], vec![])
        .with_direct_role(RoleReservation::new_for_test(DirectRole::Current, 999, closed_cell()));
    let accepted = crate::kms::owner::ledger::Submitted::new(vec![old], vec![]).accepted();
    assert_eq!(
        consumer.consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit,
                resources: accepted,
            },
            &mut service,
        ),
        Err(ResourceError::InvalidState),
        "a failed move of the old Current into its reserved slot must be returned \
         [census:C-retire-move-into-reserved]"
    );
    assert!(consumer.capacity.is_admission_closed());
}

/// census: C-retire-submitted-to-current commit.rs consume `let Some(ref mut role) = res.direct_role && role.role == DirectRole::Submitted && let Err(err) = self.capacity.move_role(role, DirectRole::Current)`
#[test]
fn c0_2ci_guard_completion_retired_returns_failed_submitted_to_current() {
    let (mut service, new_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    let new = CommitResources::new(vec![new_alloc], None, None, None, vec![], vec![])
        .with_direct_role(RoleReservation::new_for_test(DirectRole::Submitted, 997, closed_cell()));
    let accepted = crate::kms::owner::ledger::Submitted::new(vec![], vec![new]).accepted();
    assert_eq!(
        consumer.consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: CommitId::for_tests(921),
                resources: accepted,
            },
            &mut service,
        ),
        Err(ResourceError::InvalidState),
        "a failed move of the new Submitted into Current must be returned \
         [census:C-retire-submitted-to-current]"
    );
    assert!(consumer.capacity.is_admission_closed());
}

/// census: C-on-available-releasing-early-return commit.rs on_available `let Some(err) = transition_error` #1
#[test]
fn c0_2ci_guard_on_available_leaves_rejected_untouched_after_releasing_error() {
    let (mut service, releasing_alloc, _drops) = spy_service();
    let rejected_alloc = spy(&mut service);
    let rejected_key = rejected_alloc.key();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![CommitResources::new(
        vec![releasing_alloc],
        None,
        None,
        None,
        vec![],
        vec![],
    )
    .with_direct_role(RoleReservation::new_for_test(DirectRole::Preparing, 999, closed_cell()))];
    // Releasable and roleless: processing it would drop it.
    consumer.rejected_resources = vec![CommitResources::new(
        vec![rejected_alloc],
        None,
        None,
        None,
        vec![],
        vec![],
    )];
    // Both the first and the second transition_error check return the same
    // error, so the return value cannot tell them apart. What the first one
    // protects is the rejected half: it must not be processed after an error.
    assert_eq!(
        consumer.on_available(&[], &mut service),
        Err(ResourceError::InvalidState)
    );
    // "Untouched", not just "still one": the same resource, still holding the
    // same allocation, and nothing released on its behalf.
    let rejected: Vec<Vec<AllocationKey>> = consumer
        .rejected_resources
        .iter()
        .map(|r| r.allocations.iter().map(|a| a.key()).collect())
        .collect();
    assert_eq!(
        (rejected, consumer.released_presents.len()),
        (vec![vec![rejected_key]], 0),
        "rejected resources must be left untouched after a releasing-half error \
         [census:C-on-available-releasing-early-return]"
    );
}

/// census: C-on-available-rejected-error commit.rs on_available `let Some(err) = transition_error` #2
#[test]
fn c0_2ci_guard_on_available_returns_rejected_half_error() {
    let (mut service, rejected_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    consumer.rejected_resources = vec![CommitResources::new(
        vec![rejected_alloc],
        None,
        None,
        None,
        vec![],
        vec![],
    )
    .with_direct_role(RoleReservation::new_for_test(DirectRole::Preparing, 999, closed_cell()))];
    assert_eq!(
        consumer.on_available(&[], &mut service),
        Err(ResourceError::InvalidState),
        "a failed finish_role on a rejected resource must be returned \
         [census:C-on-available-rejected-error]"
    );
}
```

- [ ] **Step 2: Run the tests; they pass**

Run: `cargo test -p yserver --lib c0_2ci_guard_`
Expected: `23 passed`. A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `tools/guard-census.py --files commit.rs --deterministic-only --require-oracle --fn consume`, then the same with `--fn on_available`.
Expected: the four tagged sites `CAUGHT_BY_ORACLE`, exit 0. `CAUGHT_WHOLE_BODY` for one means only the whole-body strategy compiled there, which does not prove the guard: stop and report the site, as for `A_MANO`.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove the commit consumer reports transition failures

Family C of the stage 2c-i debt. CompletionRetired returns the error when
moving the old Current into its reserved slot fails and when moving the
new Submitted into Current fails; on_available returns its rejected-half
error, and after a releasing-half error leaves rejected resources
untouched -- the observable its first transition_error check protects,
since the second check returns the same error.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 6: Family D — releasability and uniqueness (3 guards)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `spy`, `member` (Task 2); `super::storage::{StorageLease, PixelIdentity}` (all fields `pub`); `crate::kms::render::target::PaintTarget::new(DrawableId, (i32, i32), Option<vk::Rect2D>, u8)`; `crate::kms::render::store::DrawableId::for_tests(u64)`; `register_commit_dependencies`.

**Why:** `is_resource_releasable` is private; it is observed through `on_available`, which keeps an unreleasable resource in `releasing_resources` and drops a releasable one. A `StorageLease` is built directly over a spy allocation with null Vulkan handles, so no hardware is needed.

- [ ] **Step 1: Append the tests**

```rust
fn storage_lease(allocation: AllocationLease) -> super::storage::StorageLease {
    let key = allocation.key();
    super::storage::StorageLease {
        allocation,
        pixels: super::storage::PixelIdentity {
            target: crate::kms::render::target::PaintTarget::new(
                crate::kms::render::store::DrawableId::for_tests(1),
                (0, 0),
                None,
                24,
            ),
            allocation: key,
            content_offset: (0, 0),
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
            format: ash::vk::Format::B8G8R8A8_UNORM,
            image_view: ash::vk::ImageView::null(),
            sample_view: ash::vk::ImageView::null(),
            image: ash::vk::Image::null(),
        },
    }
}

/// census: D-releasable-source commit.rs is_resource_releasable `let Some(source) = &res.source && !service.is_releasable(&source.allocation.key())`
#[test]
fn c0_2ci_guard_on_available_retains_resource_with_busy_source() {
    let (mut service, held, _drops) = spy_service();
    let _pending = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![CommitResources::new(
        vec![],
        Some(storage_lease(held)),
        None,
        None,
        vec![],
        vec![],
    )];
    consumer.on_available(&[], &mut service).unwrap();
    assert_eq!(
        consumer.releasing_resources.len(),
        1,
        "a resource whose source allocation is not releasable must stay releasing \
         [census:D-releasable-source]"
    );
}

/// census: D-releasable-fallback commit.rs is_resource_releasable `let Some(fallback) = &res.fallback && !service.is_releasable(&fallback.allocation.key())`
#[test]
fn c0_2ci_guard_on_available_retains_resource_with_busy_fallback() {
    let (mut service, held, _drops) = spy_service();
    let _pending = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![CommitResources::new(
        vec![],
        None,
        Some(storage_lease(held)),
        None,
        vec![],
        vec![],
    )];
    consumer.on_available(&[], &mut service).unwrap();
    assert_eq!(
        consumer.releasing_resources.len(),
        1,
        "a resource whose fallback allocation is not releasable must stay releasing \
         [census:D-releasable-fallback]"
    );
}

/// census: D-unique-new-members commit.rs register_commit_dependencies `!GroupMember::validate_unique(&new_members)`
#[test]
fn c0_2ci_guard_commit_dependencies_refuse_duplicate_new_members() {
    let (mut service, old_alloc, _drops) = spy_service();
    let new_alloc = spy(&mut service);
    let old = vec![CommitResources::new(vec![old_alloc], None, None, None, vec![member()], vec![])];
    let new = vec![CommitResources::new(
        vec![new_alloc],
        None,
        None,
        None,
        vec![member(), member()],
        vec![],
    )];
    assert!(
        matches!(
            register_commit_dependencies(CommitId::for_tests(930), old, new, &mut service),
            Err((ResourceError::InvalidProof, _, _))
        ),
        "a commit whose new set repeats a member must be refused [census:D-unique-new-members]"
    );
}
```

- [ ] **Step 2: Run the tests; they pass**

Run: `cargo test -p yserver --lib c0_2ci_guard_`
Expected: `26 passed`. A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `tools/guard-census.py --files commit.rs --deterministic-only --require-oracle --fn is_resource_releasable`, then the same with `--fn register_commit_dependencies`.
Expected: the three tagged sites `CAUGHT_BY_ORACLE`, exit 0.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove releasability of source and fallback pins and new-member uniqueness

Family D of the stage 2c-i debt: a commit resource stays releasing while
its source or fallback storage allocation is not releasable, the
siblings of the allocation and kms_obligations branches; and
register_commit_dependencies refuses a new set that repeats a member, as
it already provably does for the old set.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 7: Family B — writes refused while the transport is closed (1 guard)

**Files:**
- Modify: `crates/yserver/src/kms/render/resources/guard_tests.rs` (append)

**Interfaces:**
- Consumes: `TransportGate::{new_legacy, force_close, authorize_write}`; `FakeDirectOwnershipState::new` (it is `Clone`); `WriterClass` (nine variants).

- [ ] **Step 1: Append the test**

```rust
/// census: B-authorize-write-closed transport.rs authorize_write `TransportState::Closed =>`
#[test]
fn c0_2ci_guard_authorize_write_refuses_every_class_when_closed() {
    let ownership = FakeDirectOwnershipState::new();
    let mut gate = TransportGate::new_legacy(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
        Box::new(ownership.clone()),
    );
    gate.force_close();
    for class in [
        WriterClass::Primary,
        WriterClass::Unflip,
        WriterClass::Modeset,
        WriterClass::Dpms,
        WriterClass::Vt,
        WriterClass::Topology,
        WriterClass::Cursor,
        WriterClass::Gamma,
        WriterClass::HelperMutation,
    ] {
        assert_eq!(
            gate.authorize_write(class, None),
            Err(ResourceError::Detached),
            "a closed transport must refuse every writer class [census:B-authorize-write-closed]"
        );
    }
}
```

- [ ] **Step 2: Run the test; it passes**

Run: `cargo test -p yserver --lib c0_2ci_guard_authorize_write_refuses_every_class_when_closed`
Expected: `1 passed`. A failure is an F8 stop.

- [ ] **Step 3: Run the oracle**

Run: `tools/guard-census.py --files transport.rs --deterministic-only --require-oracle --fn authorize_write`
Expected: the `TransportState::Closed =>` site `CAUGHT_BY_ORACLE`, exit 0.

- [ ] **Step 4: Gate, then hand off for commit**

Run: `cargo +nightly fmt && cargo clippy --all-targets -- -D warnings && cargo test -p yserver --lib c0_2ci`

The coordinator commits, after verifying, with:

```bash
git add crates/yserver/src/kms/render/resources/guard_tests.rs
git commit -m "test(kms): prove a closed transport refuses every writer class

Family B of the stage 2c-i debt. Only the Quiescing arm of
authorize_write had coverage. consume_owner_write's non-Owner refusal
moves to session 2, with family A.

Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)"
```

---

### Task 8: Session acceptance — full census, hardware gate, record

**Files:**
- Create ([H], coordinator): `docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-session-1.md`
- Modify ([H], coordinator): `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` (status line)

- [ ] **Step 1: Implementer's final checks, then hand off**

Run and keep the output for the coordinator:
- `cargo +nightly fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo clippy -p yserver --all-targets --features tcp-transport -- -D warnings`
- `cargo clippy -p yserver --all-targets --features xdmcp -- -D warnings`
- `cargo test -p yserver --lib c0_2ci`
- `for i in $(seq 1 12); do cargo test -p yserver --lib c0_2ci 2>&1 | grep "test result"; done`
- `cargo test --workspace`
- `tools/guard-census.py --deterministic-only --require-oracle --json /tmp/census-oracle.json` — every one of the 27 tagged sites `CAUGHT_BY_ORACLE`, exit 0.

Expected: all clean; `c0_2ci` gains 27 tests with zero failures over twelve runs. **Stop and hand off.**

- [ ] **Step 2 [H]: Full acceptance census**

Run: `tools/guard-census.py --legacy-enumeration --require-oracle --json /tmp/census-session-1.json` (about an hour)
Expected:
- all 27 tagged sites `CAUGHT_BY_ORACLE`; no `CAUGHT_NOT_BY_ORACLE`, `CAUGHT_WHOLE_BODY`, `ORPHAN_TAG` or `A_MANO`;
- `CAUGHT`: 32, the already-proven untagged sites;
- `SURVIVES`: exactly **8** — `issue_handover_permit` (2) and `publish_owner` (3) and `consume_owner_write`'s non-Owner check in `transport.rs`, and the error arms of `cancel_pre_submit_batch` and `freeze_uncertain_batch` in `gpu.rs`;
- exit 0.

Any other survivor, or any of the 27 not proven by its own oracle, means session 1 is not complete.

- [ ] **Step 3 [H]: Hardware gate**

Run: `cargo test -p yserver --lib c0_2ci -- --ignored`. Expected: all hardware tests pass.

- [ ] **Step 4 [H]: Record and commit**

Create `docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-session-1.md` with: the census summary and a table of site, verdict, bound test and strategy; the eight remaining survivors and the session-2 section each belongs to; the Task 1 `} else if` findings; any F8 stops; and both the implementer's and the coordinator's gate transcripts. In the spec's status line add: "Session 1 executed: 27 guards proven by oracle; see `…-census-session-1.md`." Commit with the coordinator's trailer.
