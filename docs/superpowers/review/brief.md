You are performing an ADVERSARIAL review of an implementation plan before it is executed. Your job is to find defects that would make an engineer implementing this plan write wrong, non-compiling, or spec-violating code. Be harsh. A finding is worth more than a compliment.

TARGET PLAN:
  {{PLAN}}

AUTHORITATIVE SPEC (the plan must obey it; cite line numbers):
  {{SPEC}}

{{PRIOR}}

{{CONTEXT}}

Perform ALL FIVE of the following, and say explicitly what you did for each:

1. INCORPORATION AUDIT. For each finding in the prior review, state one of:
   APPLIED (and how), PARTIAL (and exactly what is still missing), TRADED
   (fixed one thing, broke another), or NOT APPLIED. The plan may make explicit
   claims about this in a "Self-review notes" section or in per-task
   "Corrections from review" headers. Check those claims against the actual
   task text; an overstated claim is itself a finding.
   If there is no prior review, say so and skip this check.

2. VERIFY EVERY CLAIM THE PLAN MAKES ABOUT EXISTING CODE. The plan cites many
   file:line anchors. OPEN THOSE FILES. Report every anchor that is wrong,
   every type or field the plan names that does not exist, and every assertion
   about current behaviour that is false. A plan that asserts something untrue
   about the tree is the error class that sank two prior revisions of this work.

3. CROSS-TASK INTERFACE CONSISTENCY. Each task declares "Interfaces:
   Consumes/Produces". An implementer sees only their own task. Report every
   case where a later task uses a type, field, method, enum variant, test
   helper, or constant that no earlier task produces; where two tasks name the
   same thing differently; or where a signature shown in one task contradicts
   its use in another. Include test-only helpers, and include any type whose
   visibility would break a test placed in an external test crate.

4. DOES THE SHOWN CODE COMPILE AND DO THE SHOWN TESTS PASS? Read every code
   block as if you had to compile it. Report type errors, wrong enum-variant
   syntax, borrow/move errors (including field moves out of a type that
   implements Drop), missing trait bounds, blocking I/O in a test helper that
   must not block, tests that assert something the described implementation
   cannot produce, and tests whose setup contradicts the stub or helper
   behaviour they rely on.

5. SPEC COMPLIANCE. Check the plan against the spec's normative invariants.
   Report anything the plan permits that the spec forbids, or requires that the
   spec contradicts.

EXPLICITLY OUT OF SCOPE - do not report these as defects:
{{OUT_OF_SCOPE}}

OUTPUT FORMAT. Markdown. Start with a "## Verdict" line giving counts as
"N blocking, N major, N minor". Then "## Incorporation audit" as a table of the
prior findings. Then "## Findings", grouped "### Blocking" / "### Major" /
"### Minor", each finding with a stable id (B-1, M-1, m-1), a one-line title,
and a body citing plan and code line numbers. End with "## Notes on the rest"
recording what you checked and found SOUND, so the author can tell verified
ground from unexamined ground.

Definitions: BLOCKING = an engineer following this plan produces wrong,
non-compiling, or spec-violating code. MAJOR = a real defect that degrades the
result or a test that cannot prove what it claims. MINOR = cosmetic or
stylistic.
