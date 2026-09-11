# AGENTS.md - Conventions for agent-written text and PRs

Conventions for anything an agent writes in this repo: docs, PR and issue
text, commit messages, review findings. Behavioural rules (design discipline,
git, review) are in `CLAUDE.md`, which imports this file.

## Documentation

- Every doc you add or edit (rustdoc, READMEs, design text, code comments,
  PR and issue text) is simple and straight to the point. Say what the
  reader needs, once. No fluff, no restating the code or the diff, no
  boilerplate sections, no marketing tone.
- Public APIs need rustdoc.

## Minimize AI slop; use plain, simple language

Do not use invented shorthands or heavy jargon. Say what something actually is.
Never use metaphors or rhetorical flourishes. Never anthropomorphize.
For example, a file does not "sit" in a directory; it "exists" there ("sit" implies it could also "stand"). A problem does not "bite"; it "occurs" (a problem has no mouth).
No proverb symmetry ("teams change, topics stay"). No balanced contrast ("is a copy, not a rewrite"). No novelist's diction ("enters", "the latter case"). No wordplay.
Be concrete. Do not use vague imperatives like "name them", "belongs elsewhere" or "that's all it takes".
Never use fancy vocabulary. Use dry, technical, non-literary words. For example:
  - do not say "carry"; say "continue"
  - do not say "load-bearing"; say "critical"
  - do not say "survives"; say "remains"
  - do not say "asked"; say "requested"
  - do not say "refuses"; say "rejects"
  - do not say "holds"; say "contains"
Use direct, dry, technical language. Avoid phrases and names that read as sentences or narrate. For example:
  - do not say "asked to think"; say "thinking enabled"
  - do not say "what was checked, not assumed"; say "what I checked"
  - do not say "room to answer"; say "remaining capacity"
  - do not say "where it stopped"; say "stopping point"
  - do not say "for a reason worth writing down"; say "for an important reason"
  - do not say "was never written down"; say "was never documented"
Write like a software engineer with no literary skill.
Never use abstract, soft phrasing that does not say what something is, or that only passively refers to something.
Never use passive voice. Use active voice. For example, do not say "the last message wasn't written down"; say "the last message doesn't exist".
Cut filler. Never editorialize. Use simple structure and simple vocabulary.
This applies to everything you output: messages to the user, strings in code, method names, variable names, commit messages, and your own notes and status files.
Do not match existing style when it disagrees with these guidelines, but do
not restyle existing text outside the change you are making.

In code, name things in plain technical terms and follow FRAME conventions
where they exist: past-tense event names (`AgreementAccepted`), hold and
reserve vocabulary, and the domain terms used in `docs/design/`.

## Fix the root cause, not the symptom

When fixing a bug, figure out what is its root cause, not just what directly caused it.
Is the issue you're fixing a consequence of a particular architectural decision?
Is there a more *fundamental* fix you could apply which not only fixes this issue, but also either fixes similar issues, or prevents the issue from reappearing in the future?
Figure out *if* there is a fundamental root cause to what you're fixing, and what that root cause is.
NEVER patch the symptoms when a root cause exists.
When in doubt, ask the user to decide.

## Pull requests

- PR description: one or two sentences on what the PR does and why, then
  bulleted sections as needed: **Changes**, **Cleanup**, **Follow-ups**,
  **Open questions**. Skip empty sections.
- Stacked PRs only when the upper PR genuinely depends on the lower one, and
  each PR in the stack is still a single, self-contained, reviewable change.
  Do NOT stack unrelated work (a feature on a bug fix on a docs fix) just to
  avoid waiting for a merge or to dodge conflicts in generated files
  (bindings, metadata, weights) — that makes the stack unreviewable. The
  description names the base PR and the intended merge order and is kept
  current as the PR changes.
