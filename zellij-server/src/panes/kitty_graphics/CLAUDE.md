# Kitty graphics conformance

The kitty graphics suite is written against the protocol specification
(`docs/`-external: kitty's `graphics-protocol.rst`), not against the implementation, so a failure
means the protocol work is not done rather than that the test is wrong. It used to carry an
expected-red set in modules ending `_red`; all nine deviations are now closed and **nothing is
skipped** — `cargo x test` and `cargo x integration-test` are both fully green.

Write new conformance tests against the spec the same way, and quote the sentence being tested.
The module doc comment of `zellij-server/src/panes/kitty_graphics/mod.rs` records what was closed
and — more importantly — the eleven judgement calls that are deliberately pinned as-is, where the
spec does not say enough to call the behaviour wrong or two of its rules pull against each other.
Read those before changing anything they cover; the most load-bearing is that animation stores and
composes real frames but has no playback clock.
