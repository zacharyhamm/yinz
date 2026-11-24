"""Guards for the lifecycle skill, its references and the design guide.

These are cheap text checks: every relative link and anchor resolves (including
every rule-map row), the qualified-completion section has a single home, the
discard attestation is never described as required, and no instruction tells
controllers to leave out the handoff id. They do not check that the instructions
are semantically complete.
"""
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]
SKILL_DIR = ROOT / ".agents" / "skills" / "issue-lifecycle"
GUIDE = ROOT / "docs" / "ISSUE_LIFECYCLE.md"
RULE_MAP = SKILL_DIR / "references" / "rule-map.md"
QUALIFIED = "Qualified completion after closure"

FENCE = re.compile(r"^(```|~~~)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
INLINE_CODE = re.compile(r"`[^`\n]*`")
# Link text may wrap across lines; nested brackets do not occur in these files.
LINK = re.compile(r"\[[^\[\]]*\]\(([^()\s]+)\)")
SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")
SENTENCE_END = re.compile(r"(?<=[.!?])\s+|\n\s*\n|\|")
REQUIREMENT = re.compile(r"\b(requir\w*|must|needs?)\b", re.IGNORECASE)
NEGATION = re.compile(r"\b(no|not|never|nor|only|optional|optionally|unless|without)\b", re.IGNORECASE)
HANDOFF = r"`?(?:--handoff|handoffId)`?"
OMIT_HANDOFF = [
    re.compile(r"\b(omit|omits|omitting|omitted|drop|drops|skip|skips|leave out)\b[^.|]{0,60}" + HANDOFF, re.IGNORECASE),
    re.compile(HANDOFF + r"[^.|]{0,40}\b(is|are) (optional|not required|unnecessary|not needed)", re.IGNORECASE),
    re.compile(HANDOFF + r"[^.|]{0,40}\b(can|may|should) be (omitted|left out|dropped)", re.IGNORECASE),
    re.compile(r"\bwithout (a |an |the )?" + HANDOFF, re.IGNORECASE),
    re.compile(r"\b(fills|fill in|fills in|infers|derives)\s+" + HANDOFF, re.IGNORECASE),
    re.compile(r"revisionApproval:\s*\{\s*request\s*\}"),
]
# resume-closeout is not the closeout decision, so it gets no exemption.
CLOSEOUT = re.compile(r"(?<![-\w])closeout\b", re.IGNORECASE)
# Other commands whose handoff stays mandatory; a closeout sentence naming one is not exempt.
OTHER_HANDOFF_COMMANDS = re.compile(r"\b(approve|adjudicate-plan|decide-review|approve-review-fixes|ready|drive)\b",
                                    re.IGNORECASE)
NEGATED_BEFORE = re.compile(r"\b(never|not|no|cannot)\s+$", re.IGNORECASE)


def documents():
    return [SKILL_DIR / "SKILL.md", *sorted((SKILL_DIR / "references").glob("*.md")), GUIDE]


def prose(text):
    """The text outside fenced code blocks, with line structure preserved."""
    lines, fenced = [], False
    for line in text.splitlines():
        if FENCE.match(line.strip()):
            fenced = not fenced
            lines.append("")
            continue
        lines.append("" if fenced else line)
    return "\n".join(lines)


def slug(heading):
    """GitHub's anchor rule for these headings: lowercase, drop punctuation, spaces to '-'."""
    text = INLINE_CODE.sub(lambda match: match.group(0)[1:-1], heading).lower()
    return re.sub(r"[^a-z0-9 _-]", "", text).replace(" ", "-")


def headings(path):
    return [match.group(2) for line in prose(path.read_text()).splitlines() if (match := HEADING.match(line))]


def anchors(path):
    seen, result = {}, set()
    for heading in headings(path):
        base = slug(heading)
        count = seen.get(base, 0)
        seen[base] = count + 1
        result.add(base if count == 0 else f"{base}-{count}")
    return result


def links(text):
    # Inline code may contain brackets; keep a placeholder so link text stays nonempty.
    return LINK.findall(INLINE_CODE.sub("code", prose(text)))


def broken_link(source, target):
    """A description of why `target` (as written in `source`) does not resolve, or None."""
    if SCHEME.match(target):
        return None
    path, _, anchor = target.partition("#")
    resolved = (source.parent / path).resolve() if path else source
    if not resolved.exists():
        return f"{target}: {resolved} does not exist"
    if anchor:
        if resolved.suffix != ".md" or not resolved.is_file():
            return f"{target}: anchors are checked only in Markdown files"
        if anchor not in anchors(resolved):
            return f"{target}: no heading in {resolved.relative_to(ROOT)} has anchor #{anchor}"
    return None


def sentences(text):
    return [part.strip() for part in SENTENCE_END.split(text) if part.strip()]


def requires_confirmed_stopped(sentence):
    return "confirmedStopped" in sentence and bool(REQUIREMENT.search(sentence)) \
        and not NEGATION.search(sentence)


def omits_handoff(sentence):
    # "never fills --handoff" states the rule this guard protects. Closeout is
    # the deliberate exception: a user's cleanup request is its decision, and
    # the behavior revalidates every target, so its handoffId is optional.
    if CLOSEOUT.search(sentence) and not OTHER_HANDOFF_COMMANDS.search(sentence):
        return False
    return any(not NEGATED_BEFORE.search(sentence[:match.start()])
               for pattern in OMIT_HANDOFF for match in pattern.finditer(sentence))


class SkillLinkTests(unittest.TestCase):
    def test_every_relative_link_and_anchor_resolves(self):
        problems, count = [], 0
        for source in documents():
            for target in links(source.read_text()):
                count += 1
                problem = broken_link(source, target)
                if problem:
                    problems.append(f"{source.relative_to(ROOT)}: {problem}")
        self.assertGreater(count, 300)
        self.assertEqual(problems, [])

    def test_review_evaluation_is_linked_and_resolves(self):
        evaluation = SKILL_DIR / "references" / "review-evaluation.md"
        linked = [source for source in documents()
                  if any((source.parent / target.partition("#")[0]).resolve() == evaluation
                         for target in links(source.read_text()))]
        self.assertIn(SKILL_DIR / "SKILL.md", linked)
        self.assertIn(GUIDE, linked)
        for target in links(evaluation.read_text()):
            self.assertIsNone(broken_link(evaluation, target))

    def test_every_rule_map_row_links_a_resolving_destination(self):
        rows = [line for line in RULE_MAP.read_text().splitlines() if re.match(r"^\| [SDR]\d{3} \|", line)]
        ids = [row.split("|")[1].strip() for row in rows]
        self.assertEqual(len(ids), len(set(ids)), "rule-map IDs must be unique")
        for prefix, count in (("S", 114), ("D", 92)):
            self.assertEqual([i for i in ids if i[0] == prefix], [f"{prefix}{n:03d}" for n in range(1, count + 1)])
        revision = [i for i in ids if i[0] == "R"]
        self.assertEqual(revision, [f"R{n:03d}" for n in range(1, len(revision) + 1)])
        self.assertTrue(revision, "the determinism revision must have rows")
        for row in rows:
            destinations = links(row.split("|")[4])
            self.assertTrue(destinations, f"row without a destination link: {row}")
            for target in destinations:
                self.assertIsNone(broken_link(RULE_MAP, target), row)

    def test_determinism_revision_names_its_baseline(self):
        text = RULE_MAP.read_text()
        self.assertIn("## Determinism revision", text)
        self.assertIn("b03d974383900c02fa73cd27fb311a7a534fdfaf", text)

    def test_qualified_completion_has_a_single_home(self):
        homes = [(path.relative_to(ROOT), heading) for path in documents()
                 for heading in headings(path) if heading == QUALIFIED]
        self.assertEqual(homes, [(GUIDE.relative_to(ROOT), QUALIFIED)])

    def test_legacy_states_section_exists(self):
        self.assertIn("legacy-states", anchors(GUIDE))

    def test_confirmed_stopped_is_never_described_as_required(self):
        offending = [(path.name, sentence) for path in [*documents(), ROOT / "docs" / "AGENT_CLI.md"]
                     for sentence in sentences(path.read_text()) if requires_confirmed_stopped(sentence)]
        self.assertEqual(offending, [])

    def test_no_instruction_omits_the_handoff_id(self):
        offending = [(path.name, sentence) for path in documents()
                     for sentence in sentences(path.read_text()) if omits_handoff(sentence)]
        self.assertEqual(offending, [])


class GuardPredicateTests(unittest.TestCase):
    """The wording guards reject the phrasings they exist to prevent."""

    def test_slug_matches_github_anchors(self):
        self.assertEqual(slug("Dialogs hooks and sessions"), "dialogs-hooks-and-sessions")
        self.assertEqual(slug("Plugins, hooks, and skills"), "plugins-hooks-and-skills")
        self.assertEqual(slug("Durable implementation review continuity (assignment contract 2)"),
                         "durable-implementation-review-continuity-assignment-contract-2")

    def test_baseline_attestation_wording_is_rejected(self):
        for sentence in [
            "If a worker completed but returned unusable evidence, `discard-agent` requires "
            "the operator's actual `request` and `confirmedStopped: true`.",
            "All discard cases require the actual operator request, `confirmedStopped: true`, "
            "matching original task identity and unchanged lifecycle snapshot:",
            "Discard must pass `confirmedStopped: true`.",
        ]:
            self.assertTrue(requires_confirmed_stopped(sentence), sentence)
        for sentence in [
            "The actual user request authorizes superseding any prior outcome: no successful resume, "
            "non-delivery proof, `confirmedStopped` or renewed approval is required.",
            "Add `confirmedStopped: true` only when the behavior says it cannot observe a completed worker.",
        ]:
            self.assertFalse(requires_confirmed_stopped(sentence), sentence)

    def test_omitted_handoff_wording_is_rejected(self):
        for sentence in [
            "Omit --handoff; the launcher uses the current handoff.",
            "`handoffId` is optional for decide-review.",
            "Run approve without `--handoff`.",
            "The launcher fills `--handoff` from status.",
            "drive {revisionApproval: {request}} binds the current handoff.",
            "Omit `--handoff` when cleaning up; the launcher uses the current offer.",
            "Omit `--handoff` for approve and closeout alike.",
        ]:
            self.assertTrue(omits_handoff(sentence), sentence)
        for sentence in [
            "With a current closeout offer, an omitted `handoffId` answers that offer.",
            "The launcher never fills `--handoff`: pass the id of the handoff you presented.",
            "`approve --handoff <id> --request '<approval>'` records explicit approval.",
        ]:
            self.assertFalse(omits_handoff(sentence), sentence)


if __name__ == "__main__":
    unittest.main()
