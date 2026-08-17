#!/usr/bin/env bash
#
# Run every test gate CI runs, log all of it, and summarise what broke.
#
# Each gate runs even if an earlier one failed — a red build should not hide a
# test failure you also need to know about. The one exception is a gate that
# names a prerequisite: the docker suites are skipped when the gate that stands
# their containers up did not pass, since there is nothing for them to talk to.
# The exit code is non-zero if any gate failed.
#
#   scripts/run-all-tests.sh              # everything
#   scripts/run-all-tests.sh --skip-build # straight to the tests
#   scripts/run-all-tests.sh --with-e2e   # add the dockerized e2e suite
#   scripts/run-all-tests.sh --with-remote # add the remote-pane suite (real ssh)
#   scripts/run-all-tests.sh --with-ignored # both of the above: every ignored test,
#                                           # run by the gate that provides its fixtures
#
# Logs land in target/test-logs/<timestamp>/ — one file per gate, plus
# summary.txt. The path is printed at the end.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

skip_build=0
with_e2e=0
with_remote=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    --with-e2e) with_e2e=1 ;;
    --with-remote) with_remote=1 ;;
    # Every ignored test in the tree belongs to one of these two suites, and both
    # need fixtures — a container, an ssh keypair, a musl build — that only their
    # own gate sets up. So "with ignored" means "run those gates", not
    # `--include-ignored`: that spelling runs the same tests with no far machine
    # to talk to, and multi-threaded besides, which both suites forbid.
    --with-ignored) with_e2e=1; with_remote=1 ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' \
      "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
  esac
done

log_dir="target/test-logs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$log_dir" || exit 1
summary="$log_dir/summary.txt"

# Gates in CI's order, each as "name<TAB>command" — plus an optional third field
# naming a gate this one needs to have passed. See the loop below.
gates=()
[[ $skip_build -eq 0 ]] && gates+=("build	cargo x build")
gates+=(
  "test	cargo x test"
  "test-no-web	cargo x test --no-web"
  "integration-test	cargo x integration-test"
  "assets-check	cargo x assets --check"
  "format-check	cargo x format --check"
  "xtask	cargo test -p xtask"
)
# Both docker suites split into a fixture gate and a test gate, so a container
# that never came up is reported as that, rather than as every test failing.
# `ci e2e --test` builds what it needs itself; all it is missing is the machine.
[[ $with_e2e -eq 1 ]] && gates+=(
  "e2e-up	docker compose up -d --wait"
  "e2e	cargo x ci e2e --test	e2e-up"
)
[[ $with_remote -eq 1 ]] && gates+=(
  "remote-test-build	cargo x remote-test --build"
  "remote-test	cargo x remote-test --test	remote-test-build"
)

failed=()
skipped=()
printf 'Test run: %s\n\n' "$(date)" | tee "$summary"

# Is $1 the name of a gate that already failed?
gate_failed() {
  local name
  for name in ${failed[@]+"${failed[@]}"}; do
    [[ "$name" == "$1" ]] && return 0
  done
  return 1
}

for gate in "${gates[@]}"; do
  IFS=$'\t' read -r name command needs <<< "$gate"
  log="$log_dir/$name.log"

  # A test gate whose fixture gate failed has nothing to run against. Running it
  # anyway is worse than not: it reports a wall of test failures against whatever
  # stale container happened to still be listening.
  if [[ -n "${needs:-}" ]] && gate_failed "$needs"; then
    skipped+=("$name")
    printf '=== %-18s SKIP (needs %s)\n' "$name" "$needs"
    printf '    SKIP  %-18s       (prerequisite %s failed)\n' "$name" "$needs" >> "$summary"
    continue
  fi

  printf '=== %-18s %s\n' "$name" "$command"
  start=$SECONDS
  # A real pipe rather than process substitution: tee is finished, and the log
  # fully flushed, before the grep below reads it back.
  eval "$command" 2>&1 | tee "$log"
  status=${PIPESTATUS[0]}
  elapsed=$((SECONDS - start))

  if [[ $status -eq 0 ]]; then
    printf '    PASS  %-18s %3ds  %s\n' "$name" "$elapsed" "$log" >> "$summary"
  else
    failed+=("$name")
    printf '    FAIL  %-18s %3ds  (exit %d)  %s\n' \
      "$name" "$elapsed" "$status" "$log" >> "$summary"
    # The lines worth reading first, so the summary is usable on its own.
    {
      printf '\n--- %s: what went wrong ---\n' "$name"
      grep -E "test result: FAILED|^ *FAIL \[|^error(\[|:)|panicked at|assertion .*failed" \
        "$log" | head -40
      printf -- '--- end %s ---\n\n' "$name"
    } >> "$summary"
  fi
done

printf '\n' | tee -a "$summary"
if [[ ${#failed[@]} -eq 0 ]]; then
  printf 'All %d gates passed.\n' "${#gates[@]}" | tee -a "$summary"
  exit_code=0
else
  printf '%d of %d gates FAILED: %s\n' \
    "${#failed[@]}" "${#gates[@]}" "${failed[*]}" | tee -a "$summary"
  exit_code=1
fi
if [[ ${#skipped[@]} -gt 0 ]]; then
  printf '%d skipped for a failed prerequisite: %s\n' \
    "${#skipped[@]}" "${skipped[*]}" | tee -a "$summary"
fi

# Two suites are known to flake under parallel load and pass on a rerun. Worth
# checking before chasing either as a real break.
printf '\nKnown flaky: zellij-client web-client websocket tests, zellij-server scroll snapshots.\n' \
  | tee -a "$summary"
printf 'Logs: %s\n' "$log_dir" | tee -a "$summary"

exit "$exit_code"
