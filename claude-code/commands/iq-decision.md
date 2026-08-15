---
description: Record a new IQ (Investigation Question) decision record
argument-hint: <topic>
---

Record a new IQ decision record for: $ARGUMENTS

1. Read `docs/iq/` to find the next IQ number and match the established
   record format (context, question, options considered, decision,
   consequences).
2. Investigate the topic enough to present real options — cite paper
   sections, code (file:line), and measurements, not vibes.
3. Write `docs/iq/IQ-<n>-<short-slug>.md` in that format.
4. If the decision creates or modifies a load-bearing invariant, update
   CLAUDE.md §Load-bearing invariants in the same PR (that section
   requires an IQ first — this is it).
5. Present the draft to the human for review before committing.
