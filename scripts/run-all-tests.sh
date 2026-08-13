#!/usr/bin/env bash
#
# Run every test gate CI runs, log all of it, and summarise what broke.
#
# Each gate runs even if an earlier one failed — a red build should not hide a
# test failure you also need to know about. The exit code is non-zero if any
# gate failed.
#
#   scripts/run-all-tests.sh              # everything
#   scripts/run-all-tests.sh --skip-build # straight to the tests
#   scripts/run-all-tests.sh --with-e2e   # add the dockerized e2e suite
#   scripts/run-all-tests.sh --with-ignored # include ignored tests
#
# Logs land in target/test-logs/<timestamp>/ — one file per gate, plus
# summary.txt. The path is printed at the end.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

skip_build=0
with_e2e=0
with_ignored=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) skip_build=1 ;;
    --with-e2e) with_e2e=1 ;;
    --with-ignored) with_ignored=1 ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' \
      "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
  esac
done

log_dir="target/test-logs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$log_dir" || exit 1
summary="$log_dir/summary.txt"

# Gates in CI's order, each as "name<TAB>command".
gates=()
[[ $skip_build -eq 0 ]] && gates+=("build	cargo x build")
test_args=""
if [[ $with_ignored -eq 1 ]]; then
  test_args=" -- --include-ignored"
fi
gates+=(
  "test	cargo x test$test_args"
  "test-no-web	cargo x test --no-web$test_args"
  "integration-test	cargo x integration-test"
  "assets-check	cargo x assets --check"
  "format-check	cargo x format --check"
  "xtask	cargo test -p xtask"
)
[[ $with_e2e -eq 1 ]] && gates+=("e2e	cargo x ci e2e --test")

failed=()
printf 'Test run: %s\n\n' "$(date)" | tee "$summary"

for gate in "${gates[@]}"; do
  name="${gate%%	*}"
  command="${gate#*	}"
  log="$log_dir/$name.log"

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

# Two suites are known to flake under parallel load and pass on a rerun. Worth
# checking before chasing either as a real break.
printf '\nKnown flaky: zellij-client web-client websocket tests, zellij-server scroll snapshots.\n' \
  | tee -a "$summary"
printf 'Logs: %s\n' "$log_dir" | tee -a "$summary"

exit "$exit_code"
