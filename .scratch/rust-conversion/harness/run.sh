#!/usr/bin/env bash
# CLI golden-corpus harness — ticket 19
# (.scratch/rust-conversion/issues/19-cli-golden-corpus-harness.md).
#
# Runs this repo's real, compiled CLI binaries against the real-driver-
# reachable subset of upstream's own tests/test_cli.py corpus and checks
# the result against upstream's tests/data/cli/compare/*.xml fixtures —
# not the compare files themselves (those are unified diffs; matching
# their exact text isn't the point), but the specific attribute changes
# each one documents. Each --edit case below names the upstream compare
# file it was checked against, so a diff you're chasing can be found
# directly. upstream's own corpus has no --add-device/--remove-device
# fixtures for watchdog/vsock at all (checked directly); those four
# cases are self-verified against the real driver instead — labeled as
# such at the point they're defined, same rigor as the analogous
# virtinst-cli/src/lib.rs unit tests, one level up through the real
# compiled binary instead of virtinst-core's library API directly.
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
# Adding a case: for --edit, copy one of the existing blocks, point it
# at the real optstring + upstream compare-file name, and list the
# xpath checks the diff implies. For --add-device/--remove-device,
# check tests/test_cli.py for a real fixture using that device first
# (grep the device name near "--add-device"/"--remove-device") — only
# fall back to a self-verified case, clearly labeled, if none exists.
# Extending this to virt-install needs the capability-injection layer
# ticket 19 scoped as follow-up work first.
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

# run_virt_xml_action ACTION ARGS... — like run_virt_xml, but for
# --add-device/--remove-device instead of --edit (those two don't take
# a selector positionally the way --edit does, so this is the same
# --print-xml wiring with a different action flag).
run_virt_xml_action() {
    local action="$1"
    shift
    VIRTINST_TEST_PREDICTABLE=1 "$BIN" --connect "test://$TESTSUITE_XML" \
        test-for-virtxml "$action" --print-xml "$@"
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

# The four cases below aren't tied to a specific upstream compare file —
# checked directly: upstream's --add-device/--remove-device corpus has
# no watchdog/vsock fixtures at all (grepped tests/test_cli.py for
# both). Self-verified against the real driver instead, same rigor
# add_then_remove_device_against_a_real_domain/add_then_remove_vsock_
# against_a_real_domain (virtinst-cli/src/lib.rs) already established —
# this is that same proof one level up, through the real compiled
# binary via --print-xml instead of virtinst-core's library API
# directly. test-for-virtxml already has one watchdog and one vsock
# (see the two edit cases above), so --add-device must land a *second*
# one without disturbing the first, and --remove-device 1 against that
# same starting state must remove it entirely.

echo
echo "=== add-device-watchdog (self-verified, no upstream fixture for this device+action) ==="
echo "  cmd: --add-device --watchdog model=i6300esb,action=poweroff"
xml="$(run_virt_xml_action --add-device --watchdog model=i6300esb,action=poweroff)"
check_xpath "$xml" "count(//watchdog)" "2" "two watchdogs now"
check_xpath "$xml" "string(//watchdog[1]/@model)" "ib700" "original watchdog untouched"
check_xpath "$xml" "string(//watchdog[1]/@action)" "poweroff" "original watchdog untouched"
check_xpath "$xml" "string(//watchdog[2]/@model)" "i6300esb" "new watchdog has the given model"
check_xpath "$xml" "string(//watchdog[2]/@action)" "poweroff" "new watchdog has the given action"

echo
echo "=== add-device-vsock (self-verified, no upstream fixture for this device+action) ==="
echo "  cmd: --add-device --vsock model=virtio,cid.address=7"
xml="$(run_virt_xml_action --add-device --vsock model=virtio,cid.address=7)"
check_xpath "$xml" "count(//vsock)" "2" "two vsocks now"
check_xpath "$xml" "string(//vsock[1]/cid/@address)" "5" "original vsock untouched"
check_xpath "$xml" "string(//vsock[2]/cid/@address)" "7" "new vsock has the given address"

echo
echo "=== remove-device-watchdog (self-verified, no upstream fixture for this device+action) ==="
echo "  cmd: --remove-device --watchdog 1"
xml="$(run_virt_xml_action --remove-device --watchdog 1)"
check_xpath "$xml" "count(//watchdog)" "0" "the only watchdog is gone"

echo
echo "=== remove-device-vsock (self-verified, no upstream fixture for this device+action) ==="
echo "  cmd: --remove-device --vsock 1"
xml="$(run_virt_xml_action --remove-device --vsock 1)"
check_xpath "$xml" "count(//vsock)" "0" "the only vsock is gone"

echo
echo "$pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
