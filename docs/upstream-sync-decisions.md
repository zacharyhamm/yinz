# Upstream Sync Decisions

## 2026-08-11 — Realign raw-byte buffer with keyboard parser

- Upstream commits: `1d1d5c1101e608dea8cfc429af44ed455b3b06ee`
- Upstream range: `5254e4fc1dd784ef872644190dc5e2bcb0981bed..c428ae93b9f21fc79f89b4377b3f561ec66cda7d`
- Fork baseline: `main` at `33e881122`
- Decision: Skip
- Reason: The fork already fixes the same root cause in commit `0f34b3737` ("fix: preserve enter after bracketed paste"), inside `InputParser::parse_with_consumed` in the vendored termwiz parser. The fork's fix corrects the byte-accounting at its source; upstream's fix realigns a separate buffer externally. Adding upstream's patch on top would introduce a second, competing correction mechanism for an already-solved bug. Verified by running upstream's three regression tests against the fork's existing fix: all three passed unmodified.
- Reconsider only if: A future regression shows the fork's `unreported_consumed` accounting approach misses a case that upstream's `buffered_len` approach catches, or the user reopens this decision.
