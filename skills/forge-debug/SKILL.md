---
name: forge-debug
description: Fix bugs, failing tests and user reports at the root, with a reproduction first.
---

# Debugging and fixes

## Before touching code
1. Search Forge Memory (`memory_search`) for the symptom: it may have been solved before.
2. Reproduce it. The reported symptom is evidence; the reporter's explanation of the cause is only a hypothesis.
3. Write the reproduction as a failing test when the behavior can be tested. If the test passes on unmodified code, the diagnosis is wrong, not the report: make the test exercise the real path until it fails.
4. Map the mechanism with one `file:line` per claim. If the code contradicts your hypothesis, the code wins: change the plan, not the evidence.

## Fixing
- Fix the root, not the symptom. Several reports with the same root get one fix.
- Prefer, in this order: removing the mechanism that makes the bug possible, a test or check that makes it impossible to reintroduce, a local fix covered by the failing test. A new state, flag or setting is the last resort.
- If an error message is a dead end, the fix is to name a working next step in the message, and to check that step actually works.
- Check whether a test was written to agree with the bug; fix or remove it instead of keeping a test that pins the bug.

## Done
- The failing test now passes, and the original scenario works when run again by hand or end to end.
- Run the whole suite (`cargo test --workspace --no-fail-fast`) and read every group, not just the first lines.
- Save the error in Forge Memory (`memory_save`, type `error`: symptom, cause, fix) if another agent could hit it again.
