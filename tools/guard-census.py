#!/usr/bin/env python3
"""Guard-clause mutation census for the stage 2c-i resource service.

Enumerates every refusal guard in the resource-service files, neutralises
each one in turn, runs the c0_2ci suite, and classifies the guard. With tags
(see the plan's Task 1), a guard counts as proven only when the test the tag
is bound to fails, carrying the guard's own [census:MARKER].

Spec: docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md,
sections 2, 3.0 and 5.1.

This mutates source files and runs cargo. It refuses to start if the target
files have uncommitted changes or the unmutated suite is not green, and
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
        if subprocess.run(["git", "diff", "--quiet", "--", *rels], cwd=ROOT).returncode:
            sys.exit("REFUSING: target files have uncommitted changes; the tool restores them with git checkout.")
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
