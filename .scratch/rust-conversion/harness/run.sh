#!/usr/bin/env bash
# CLI golden-corpus harness — ticket 19
# (.scratch/rust-conversion/issues/19-cli-golden-corpus-harness.md).
#
# Runs this repo's real, compiled CLI binaries against the real-driver-
# reachable subset of upstream's own tests/test_cli.py corpus and checks
# the result against upstream's tests/data/cli/compare/*.xml fixtures —
# not the compare files themselves (those are unified diffs; matching
# their exact text isn't the point), but the specific attribute changes
# each one documents. Each CASE below names the upstream compare file it
# was checked against, so a diff you're chasing can be found directly.
#
# Why this shape, not a straight upstream-diff-format comparison:
#   - Most of upstream's corpus depends on a Python-level libvirt.open()
#     interception (MagicURI) injecting fake per-arch capabilities XML,
#     which the real `virt` crate has no equivalent seam for (ticket 19's
#     main finding). virt-xml's --edit/--add-device/--remove-device don't
#     need that — they mutate an already-existing domain, no capability-
#     driven defaulting involved — so this harness covers virt-xml only
#     for now. virt-install's corpus needs that capability layer first.
#   - The real `test://` driver's state is per-process and ephemeral: a
#     second `virsh`/process invocation never sees what a first one
#     defined (checked directly — see virt-xml.rs's own doc comment on
#     --print-xml). So every case here uses --print-xml, which upstream
#     itself documents as pure local preview ("perform requested XML
#     edits locally, but don't submit them to libvirt") — the edit and
#     the inspection happen in the one process invocation that can see
#     both.
#   - VIRTINST_TEST_PREDICTABLE=1 is set on every invocation on general
#     principle (harmless no-op for the watchdog/vsock cases below, which
#     never touch generate_uuid/generate_mac) — real once a case creates
#     a fresh device/domain instead of editing an existing one.
#
# Adding a case: copy one of the existing `run_case` calls, point it at
# the real optstring + upstream compare-file name, and list the xpath
# checks the diff implies. Extending this to virt-install needs the
# capability-injection layer ticket 19 scoped as follow-up work first.
#
# Usage: .scratch/rust-conversion/harness/run.sh
# Env:   UPSTREAM_REF — path to the upstream virt-manager checkout
#        (default: ~/DEV/virt-manager-upstream-reference, see
#        memory/upstream-reference-restored.md for how to recreate it).

set -euo pipefail

UPSTREAM_REF="${UPSTREAM_REF:-$HOME/DEV/virt-manager-upstream-reference}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TESTSUITE_XML="$UPSTREAM_REF/tests/data/testdriver/testsuite.xml"
BIN="$REPO_ROOT/target/debug/virt-xml"

if [[ ! -f "$TESTSUITE_XML" ]]; then
    echo "harness: upstream reference not found at $UPSTREAM_REF" >&2
    echo "  (set UPSTREAM_REF, or see memory/upstream-reference-restored.md" >&2
    echo "  for how to recreate the checkout)" >&2
    exit 2
fi

echo "Building virt-xml..."
cargo build -q --manifest-path "$REPO_ROOT/Cargo.toml" -p virtinst-cli --bin virt-xml

pass=0
fail=0

# Runs virt-xml against test-for-virtxml (upstream's own fixture domain,
# tests/data/testdriver/testsuite.xml — the default backing state
# vixml.add_category's compare tests use) with the given --edit args,
# and prints the resulting XML. The seed file is never mutated by this
# (confirmed directly: test:// treats it as a read-only initial copy,
# and --print-xml never calls libvirt's defineXML at all) — safe to
# reuse across every case below with no reset step needed.
run_virt_xml() {
    VIRTINST_TEST_PREDICTABLE=1 "$BIN" --connect "test://$TESTSUITE_XML" \
        test-for-virtxml --edit --print-xml "$@"
}

# check_xpath XML XPATH EXPECTED LABEL
check_xpath() {
    local xml="$1" xpath="$2" expected="$3" label="$4"
    local actual
    actual="$(printf '%s' "$xml" | xmllint --xpath "$xpath" - 2>/dev/null || true)"
    if [[ "$actual" == "$expected" ]]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label"
        echo "        xpath:    $xpath"
        echo "        expected: $expected"
        echo "        actual:   $actual"
        fail=$((fail + 1))
    fi
}

echo
echo "=== edit-simple-watchdog ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-watchdog.xml"
echo "  cmd: --watchdog action=reset"
xml="$(run_virt_xml --watchdog action=reset)"
check_xpath "$xml" "string(//watchdog/@model)" "ib700" "model untouched"
check_xpath "$xml" "string(//watchdog/@action)" "reset" "action changed to reset"

echo
echo "=== edit-simple-vsock ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-vsock.xml"
echo "  cmd: --vsock model=virtio,cid.address=,cid.auto=on"
xml="$(run_virt_xml --vsock model=virtio,cid.address=,cid.auto=on)"
check_xpath "$xml" "string(//vsock/@model)" "virtio" "model untouched"
check_xpath "$xml" "string(//vsock/cid/@auto)" "yes" "cid.auto changed to yes"
check_xpath "$xml" "boolean(//vsock/cid/@address)" "false" "cid.address removed, not \"\""

echo
echo "$pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
