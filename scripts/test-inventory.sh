#!/usr/bin/env bash
#
# Enumerate every automated test in the tree as a markdown checklist.
#
#   scripts/test-inventory.sh                  # everything, to docs/TEST_INVENTORY.md
#   scripts/test-inventory.sh --out /tmp/x.md  # somewhere else
#   scripts/test-inventory.sh --fast           # skip --all-features (drops the e2e
#                                              # and postgres groups; see below)
#   scripts/test-inventory.sh --no-scan        # nextest only, no static cross-check
#
# One `- [ ]` per test, grouped by the gate that runs it, then by crate and test
# binary. `#[ignore]`d tests are tagged; a group where every test is ignored says
# so in its heading instead.
#
# Two passes. `cargo nextest list` is authoritative: one listing over the whole
# workspace with --all-features covers every gate, because each gate differs only
# by a cargo feature or an #[ignore], and *listing* needs no docker, no /dev/kvm
# and no Postgres - only running does. Doctests need a second command, since
# nextest cannot list them at all.
#
# The second pass greps the sources for #[test] and reports any test function
# nextest never listed, which is how a suite that failed to build, or code behind
# a cfg the listing did not enable, shows up rather than silently vanishing. It
# undercounts on purpose in one place: tests generated inside macro_rules! (see
# default-plugins/hocus-proto/src/store.rs, and
# zellij-storage/src/plugin_data_store/tests/, which runs one file per engine)
# have no #[test] in the source at all. That is why nextest is the primary source
# and this pass is only a cross-check.
#
# --fast drops --all-features, which otherwise builds libssh2 + OpenSSL (the e2e
# feature's ssh2) and curl (vendored_curl) on a cold tree. The report names the
# groups it lost.
#
# Needs cargo-nextest and jq. Exits non-zero if the listing failed.

set -uo pipefail
# Deterministic grouping and ordering, independent of the caller's locale.
export LC_ALL=C

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

out="docs/TEST_INVENTORY.md"
fast=0
scan=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) out="${2:?--out needs a path}"; shift 2 ;;
    --fast) fast=1; shift ;;
    --no-scan) scan=0; shift ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' \
      "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1 (try --help)" >&2; exit 2 ;;
  esac
done

command -v jq >/dev/null || { echo "test-inventory needs jq (apt install jq)" >&2; exit 2; }
cargo nextest --version >/dev/null 2>&1 || {
  echo "test-inventory needs cargo-nextest (cargo install cargo-nextest)" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tests="$work/tests.tsv"      # order<TAB>gate<TAB>group<TAB>test<TAB>ignored
: > "$tests"

# ---------------------------------------------------------------- nextest pass
feature_args=(--all-features)
[[ $fast -eq 1 ]] && feature_args=()

echo ">> listing tests (this compiles every test binary; first run is slow)" >&2
listing_failed=0
if ! cargo nextest list --workspace "${feature_args[@]}" --run-ignored all \
      --message-format json --list-type full > "$work/list.json" 2> "$work/list.err"; then
  listing_failed=1
  echo ">> nextest listing FAILED; see the report's banner" >&2
  tail -20 "$work/list.err" >&2
fi

if [[ $listing_failed -eq 0 ]]; then
  jq -r '
    def gate($pkg; $kind; $bin; $name):
      if $pkg == "zellij-integration-tests" then
        ["2", "cargo x integration-test", ""]
      elif $pkg == "zellij-server" and $kind == "test" and $bin == "remote_pane_ssh" then
        ["4", "cargo x remote-test --build && cargo x remote-test --test", ""]
      elif $pkg == "zellij-server" and $kind == "test" and $bin == "firecracker_vm" then
        ["5", "cargo x firecracker-test --build && cargo x firecracker-test --test", ""]
      elif ($name | test("(^|::)e2e::")) then
        ["3", "cargo x ci e2e --test", " — the `tests::e2e` half"]
      elif $pkg == "zellij-storage" and ($name | test("plugin_data_store::(tests::)?postgres::")) then
        ["6", "cargo x postgres-test --build && cargo x postgres-test --test",
         " — the `postgres` half"]
      else
        ["1a", "cargo x test", ""]
      end;
    ."rust-suites" | to_entries[]
    | .value as $s
    | ($s["package-name"]) as $pkg
    | ($s.kind) as $kind
    | ($s["binary-name"]) as $bin
    | $s.testcases | to_entries[]
    | .key as $name
    | gate($pkg; $kind; $bin; $name) as $g
    | [$g[0], $g[1], ($pkg + " (" + $kind + "/" + $bin + ")" + $g[2]), $name,
       (if .value.ignored then "1" else "0" end)]
    | @tsv
  ' "$work/list.json" >> "$tests"
fi

# Doctests: nextest runs one process per test and so cannot list them.
echo ">> listing doctests" >&2
if cargo test --doc --workspace "${feature_args[@]}" -- --list > "$work/doc.txt" 2>"$work/doc.err"; then
  # libtest prints "src/lib.rs - path::to::item (line 42): test"
  sed -n 's/^\(.*\): test$/\1/p' "$work/doc.txt" \
    | awk -F'\t' '{printf "1b\tcargo x test\tdoctests (the `cargo test --doc` pass inside `cargo x test`)\t%s\t0\n", $0}' >> "$tests"
else
  echo ">> doctest listing failed (non-fatal)" >&2
fi

# ------------------------------------------------------------- static cross-check
unmatched="$work/unmatched.txt"
: > "$unmatched"
if [[ $scan -eq 1 ]]; then
  echo ">> scanning sources for #[test]" >&2
  cut -f4 "$tests" | awk -F'::' '{print $NF}' | sed 's/ (line [0-9]*)$//' | sort -u > "$work/known.txt"
  # Every #[test]/#[tokio::test] function in the tree, as "path:line<TAB>name".
  find . -name target -prune -o -name '*.rs' -print 2>/dev/null | sort | while read -r f; do
    awk -v F="$f" '
      /^[[:space:]]*#\[(tokio::)?test([(\]])/ { pending = 1; next }
      pending && /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?(unsafe[[:space:]]+)?fn[[:space:]]/ {
        if (match($0, /fn[[:space:]]+[A-Za-z0-9_]+/)) {
          n = substr($0, RSTART + 3, RLENGTH - 3)
          gsub(/^[[:space:]]+/, "", n)
          printf "%s:%d\t%s\n", F, FNR, n
        }
        pending = 0
      }
    ' "$f"
  done > "$work/static.tsv"
  # A name nextest listed anywhere counts as covered: the scan cannot rebuild
  # module paths reliably, so it checks the leaf name only.
  awk -F'\t' 'FILENAME == ARGV[1] { known[$0]=1; next } !($2 in known) { print }' \
    "$work/known.txt" "$work/static.tsv" > "$unmatched"
fi

# ------------------------------------------------------------------- the report
mkdir -p "$(dirname "$out")" || exit 1
rev="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
{
  echo "# Test inventory"
  echo
  echo "Generated $(date +%Y-%m-%d) from \`$rev\` by \`scripts/test-inventory.sh\`."
  echo "One item per automated test; the heading above each block is the gate that runs it."
  echo
  if [[ $listing_failed -eq 1 ]]; then
    echo "> **The nextest listing failed.** Everything below the totals is the static"
    echo "> scan alone and is neither complete nor authoritative. Re-run once"
    echo "> \`cargo nextest list --workspace --all-features\` builds."
    echo
  fi
  if [[ $fast -eq 1 ]]; then
    echo "> Generated with \`--fast\`: no \`--all-features\`, so the **e2e** group"
    echo "> (feature \`e2e\`) and the **postgres** half of the plugin-data store are"
    echo "> missing. Re-run without \`--fast\` for the whole tree."
    echo
  fi

  echo "## Totals"
  echo
  echo "| gate | tests | ignored |"
  echo "|---|---:|---:|"
  sort -t"$(printf '\t')" -k1,1 -k2,2 "$tests" | awk -F'\t' '
    { n[$2]++; if ($5 == "1") ig[$2]++; if (!($2 in seen)) { seen[$2]=1; order[++k]=$2 } }
    END { for (i = 1; i <= k; i++) printf "| `%s` | %d | %d |\n", order[i], n[order[i]], ig[order[i]]+0
          for (i = 1; i <= k; i++) { t += n[order[i]]; g += ig[order[i]]+0 }
          printf "| **total** | **%d** | **%d** |\n", t, g }'
  echo

  sort -t"$(printf '\t')" -k1,1 -k3,3 -k4,4 "$tests" | awk -F'\t' '
    function flush() {
      if (!group) return
      allig = (gcount > 0 && gig == gcount)
      printf "## %s\n\n", heading (allig ? " — all `#[ignore]`d" : "")
      printf "Run: `%s`\n\n", gate
      for (i = 1; i <= gcount; i++)
        printf "- [ ] `%s`%s\n", name[i], (!allig && ign[i] == "1" ? " `#[ignore]`" : "")
      print ""
      gcount = 0; gig = 0
    }
    {
      if ($2 "|" $3 != group) { flush(); group = $2 "|" $3; gate = $2; heading = $3 }
      gcount++; name[gcount] = $4; ign[gcount] = $5
      if ($5 == "1") gig++
    }
    END { flush() }'

  echo "## Not run by any gate above"
  echo
  echo "Hand-maintained; these are the tests that are not Rust test functions."
  echo
  echo "- [ ] \`python3 extensions/tests/smoke.py\` — acceptance script for the"
  echo "  extension surface. Also run inside the VM by"
  echo "  \`zellij-server/tests/firecracker_vm.rs\`, so \`cargo x firecracker-test\`"
  echo "  covers it there; the standalone run is separate."
  echo
  echo "\`zellij-integration-tests/tests/swamp_exit_fixture.lua\` and the Lua agent"
  echo "harness are *not* listed here: they are driven by Rust integration tests,"
  echo "which appear under \`cargo x integration-test\` above."
  echo

  if [[ $scan -eq 1 ]]; then
    echo "## Unmatched by nextest"
    echo
    if [[ -s "$unmatched" ]]; then
      echo "Source \`#[test]\` functions whose name nextest never listed. Each is either"
      echo "behind a \`cfg\` this run did not enable, or in a suite that failed to build."
      echo
      awk -F'\t' '{printf "- [ ] `%s` — `%s`\n", $2, $1}' "$unmatched"
    else
      echo "None: every \`#[test]\` function in the sources appears in the listing above."
    fi
    echo
    printf 'Scanned %s source test functions; %s unmatched.\n\n' \
      "$(wc -l < "$work/static.tsv")" "$(wc -l < "$unmatched")"
  fi
} > "$out"

echo ">> wrote $out ($(grep -c '^- \[ \]' "$out") checklist items)"
[[ $listing_failed -eq 1 ]] && exit 1
exit 0
