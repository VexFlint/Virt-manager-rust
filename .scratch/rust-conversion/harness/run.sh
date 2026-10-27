#!/usr/bin/env bash
# CLI golden-corpus harness — ticket 19
# (.scratch/rust-conversion/issues/19-cli-golden-corpus-harness.md).
#
# Runs this repo's real, compiled CLI binaries against the real-driver-
# reachable subset of upstream's own tests/test_cli.py corpus and checks
# the result against upstream's tests/data/cli/compare/*.xml fixtures —
# not the compare files themselves (those are unified diffs; matching
# their exact text isn't the point), but the specific attribute changes
# each one documents. Each case below either names the upstream compare
# file it was checked against (so a diff you're chasing can be found
# directly), or is explicitly labeled self-verified when no real
# upstream fixture exists for that device+action combination (checked
# directly against tests/test_cli.py, not assumed) — same rigor as the
# analogous virtinst-cli/src/lib.rs unit tests, one level up through the
# real compiled binary instead of virtinst-core's library API directly.
# --disk's cases are also bounded by disk_from_optstr/apply_disk_optstr
# being only a partial slice of upstream's real (40+-field) ParserDisk —
# see that function's own doc comment for exactly which fields, and why
# a couple of real disk fixtures (edit-simple-disk and its sibling) are
# out of this slice's reach even though they exist.
#
# --memballoon has three real upstream fixtures, all covered, and no
# add-device case: the real driver rejects a second <memballoon>
# outright, matching upstream's own compare corpus having no
# add-memballoon fixture either.
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
#     principle — harmless no-op for every case below (none of
#     watchdog/vsock/disk touch generate_uuid/generate_mac; those are
#     domain- and NIC-level, not per-device), real once a case builds a
#     fresh domain or NIC instead of editing/adding an existing device.
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

echo "Building virt-xml, virt-clone and virt-install..."
cargo build -q --manifest-path "$REPO_ROOT/Cargo.toml" -p virtinst-cli --bin virt-xml --bin virt-clone --bin virt-install

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

# run_virt_xml_select SELECTOR ARGS... — like run_virt_xml, but with an
# explicit --edit selector instead of the bare (index-1-default) form,
# for the field-match cases below (select_elements_by_selector).
run_virt_xml_select() {
    local selector="$1"
    shift
    VIRTINST_TEST_PREDICTABLE=1 "$BIN" --connect "test://$TESTSUITE_XML" \
        test-for-virtxml --edit "$selector" --print-xml "$@"
}

# check_fails_with CMD... EXPECTED_STDERR LABEL — runs a command
# expected to *fail*, and checks its stderr against an exact expected
# string (upstream's own add_invalid fixtures pin exact error text).
check_fails_with() {
    local label="${@: -1}"
    local expected="${@: -2:1}"
    local args=("${@:1:$#-2}")
    local actual
    if actual="$("${args[@]}" 2>&1 >/dev/null)"; then
        echo "  FAIL: $label (expected failure, command succeeded)"
        fail=$((fail + 1))
        return
    fi
    if [[ "$actual" == *"$expected"* ]]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label"
        echo "        expected substring: $expected"
        echo "        actual stderr:      $actual"
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

# --disk cases. disk_from_optstr/apply_disk_optstr (virtinst-cli/src/
# lib.rs) are only a partial slice of upstream's real ParserDisk — see
# that function's own doc comment for exactly which fields. The two
# upstream fixtures below are within that slice's reach exactly;
# edit-simple-disk/edit-simple-disk-remove-path aren't (both need
# block-device source.dev handling this slice doesn't model), so the
# --edit case here is self-verified instead, like the four cases above.

echo
echo "=== add-disk-basic ==="
echo "  upstream: tests/data/cli/compare/virt-xml-add-disk-basic.xml"
echo "  cmd: --add-device --disk /pool-dir/testvol1.img,bus=virtio,target=vdf"
xml="$(run_virt_xml_action --add-device --disk /pool-dir/testvol1.img,bus=virtio,target=vdf)"
check_xpath "$xml" "string(//disk[last()]/@type)" "file" "type defaulted to file"
check_xpath "$xml" "string(//disk[last()]/@device)" "disk" "device defaulted to disk"
check_xpath "$xml" "string(//disk[last()]/source/@file)" "/pool-dir/testvol1.img" "source path"
check_xpath "$xml" "string(//disk[last()]/target/@dev)" "vdf" "target dev"
check_xpath "$xml" "string(//disk[last()]/target/@bus)" "virtio" "target bus"

echo
echo "=== remove-disk-index ==="
echo "  upstream: tests/data/cli/compare/virt-xml-remove-disk-index.xml"
echo "  cmd: --remove-device --disk 3"
xml="$(run_virt_xml_action --remove-device --disk 3)"
check_xpath "$xml" "count(//disk)" "2" "the third disk is gone, two remain"
check_xpath "$xml" "count(//disk/target[@dev='sda'])" "0" "the removed disk (target sda) is really gone"

echo
echo "=== edit-disk (self-verified, target real fields need block-device source.dev support) ==="
echo "  cmd: --edit 1 --disk target.bus=sata"
xml="$(run_virt_xml --disk target.bus=sata)"
check_xpath "$xml" "string(//disk[1]/target/@bus)" "sata" "bus changed to sata"
check_xpath "$xml" "string(//disk[1]/target/@dev)" "fda" "target dev untouched"
check_xpath "$xml" "string(//disk[1]/@type)" "block" "disk type untouched"

echo
echo "=== edit-select-disk-target ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-select-disk-target.xml"
echo "  cmd: --edit target=hda --disk /dev/null (field-match selector, disk's target/target.dev alias)"
xml="$(run_virt_xml_select target=hda --disk /dev/null)"
check_xpath "$xml" "string(//disk[target/@dev='hda']/source/@file)" "/dev/null" "the hda disk's path changed"

echo
echo "=== edit-select-disk-path ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-select-disk-path.xml"
echo "  cmd: --edit /tmp/foobar2 --disk shareable=off,readonly=on (field-match, disk's path/remove_first)"
xml="$(run_virt_xml_select /tmp/foobar2 --disk shareable=off,readonly=on)"
check_xpath "$xml" "count(//disk[source/@file='/tmp/foobar2']/readonly)" "1" "readonly added to the matched disk"
check_xpath "$xml" "count(//disk[source/@file='/tmp/foobar2']/shareable)" "0" "shareable removed from the matched disk"

echo
echo "=== add_invalid: no matching objects (exact upstream error text) ==="
echo "  upstream: tests/test_cli.py's add_invalid for '--edit target=vvv --disk /dev/null'"
check_fails_with "$BIN" --connect "test://$TESTSUITE_XML" test-for-virtxml --edit target=vvv --print-xml --disk /dev/null \
    "No matching objects found for --edit target=vvv" "exact upstream error text"

# --memballoon cases. Sixth device, small field set (model/autodeflate/
# stats.period/freePageReporting), three real upstream fixtures — see
# virtinst-cli/src/lib.rs's memballoon_from_optstr/apply_memballoon_optstr
# doc comments. No add-device case here: the real driver rejects a
# second <memballoon> outright ("only a single memory balloon device is
# supported"), matching upstream's own compare corpus having no
# add-memballoon fixture at all.

echo
echo "=== edit-simple-memballoon ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-memballoon.xml"
echo "  cmd: --memballoon model=none"
xml="$(run_virt_xml --memballoon model=none)"
check_xpath "$xml" "string(//memballoon/@model)" "none" "model changed to none"

echo
echo "=== edit-disable-memballoon ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-disable-memballoon.xml"
echo "  cmd: --memballoon none (positional shorthand, same result as above)"
xml="$(run_virt_xml --memballoon none)"
check_xpath "$xml" "string(//memballoon/@model)" "none" "model changed to none via positional form"

echo
echo "=== remove-memballoon ==="
echo "  upstream: tests/data/cli/compare/virt-xml-remove-memballoon.xml"
echo "  cmd: --remove-device --memballoon all"
xml="$(run_virt_xml_action --remove-device --memballoon all)"
check_xpath "$xml" "count(//memballoon)" "0" "the only memballoon is gone"

# --sound / --video cases. Both structs already existed in
# virtinst-core; this is CLI optstring wiring only — see
# sound_from_optstr/video_from_optstr's own doc comments in
# virtinst-cli/src/lib.rs.

echo
echo "=== edit-simple-soundhw ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-soundhw.xml"
echo "  cmd: --sound pcspk"
xml="$(run_virt_xml --sound pcspk)"
check_xpath "$xml" "string(//sound[1]/@model)" "pcspk" "first sound's model changed to pcspk"

echo
echo "=== add-sound ==="
echo "  upstream: tests/data/cli/compare/virt-xml-add-sound.xml"
echo "  cmd: --add-device --sound pcspk"
xml="$(run_virt_xml_action --add-device --sound pcspk)"
check_xpath "$xml" "string(//sound[last()]/@model)" "pcspk" "new sound has the given model"

echo
echo "=== edit-select-sound-model ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-select-sound-model.xml"
echo "  cmd: --edit ich6 --sound pcspk (field-match selector, not an index)"
xml="$(run_virt_xml_select ich6 --sound pcspk)"
check_xpath "$xml" "count(//sound[@model='ich6'])" "0" "the ich6 sound is gone (renamed)"
check_xpath "$xml" "count(//sound[@model='pcspk'])" "1" "...to pcspk"
check_xpath "$xml" "count(//sound[@model='sb16'])" "1" "sb16 untouched"
check_xpath "$xml" "count(//sound[@model='es1370'])" "1" "es1370 untouched"

echo
echo "=== remove-sound-model ==="
echo "  upstream: tests/data/cli/compare/virt-xml-remove-sound-model.xml"
echo "  cmd: --remove-device --sound ich6 (field-match selector, not an index)"
xml="$(run_virt_xml_action --remove-device --sound ich6)"
check_xpath "$xml" "count(//sound[@model='ich6'])" "0" "the ich6 sound is gone"
check_xpath "$xml" "count(//sound)" "2" "the other two remain"

echo
echo "=== edit-simple-video ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-video.xml"
echo "  cmd: --edit 1 --video cirrus"
xml="$(run_virt_xml --video cirrus)"
check_xpath "$xml" "string(//video[1]/model/@type)" "cirrus" "first video's model type changed to cirrus"
check_xpath "$xml" "string(//video[1]/model/@vram)" "16384" "sibling attribute untouched"
check_xpath "$xml" "string(//video[1]/model/@heads)" "1" "sibling attribute untouched"

echo
echo "=== remove-video-all ==="
echo "  upstream: tests/data/cli/compare/virt-xml-remove-video-all.xml"
echo "  cmd: --remove-device --video all"
xml="$(run_virt_xml_action --remove-device --video all)"
check_xpath "$xml" "count(//video)" "0" "both videos are gone"

# --controller / --tpm cases. Both structs already existed in
# virtinst-core; this is CLI optstring wiring only — see
# controller_from_optstr/tpm_from_optstr's own doc comments in
# virtinst-cli/src/lib.rs.

echo
echo "=== edit-simple-controller ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-controller.xml"
echo "  cmd: --controller index=15,model=lsilogic"
xml="$(run_virt_xml --controller index=15,model=lsilogic)"
check_xpath "$xml" "string(//controller[1]/@type)" "scsi" "type untouched"
check_xpath "$xml" "string(//controller[1]/@index)" "15" "index changed to 15"
check_xpath "$xml" "string(//controller[1]/@model)" "lsilogic" "model newly added"

echo
echo "=== edit-simple-tpm ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-tpm.xml"
echo "  cmd: --tpm backend.device.path=,backend.type=emulator,backend.version=2.0"
echo "  (attribute-level facts only — see apply_tpm_optstr's doc comment on the"
echo "   known no-pruning divergence for the now-attributeless <device/>)"
xml="$(run_virt_xml --tpm backend.device.path=,backend.type=emulator,backend.version=2.0)"
check_xpath "$xml" "string(//tpm/backend/@type)" "emulator" "backend type changed"
check_xpath "$xml" "string(//tpm/backend/@version)" "2.0" "backend version newly added"
check_xpath "$xml" "boolean(//tpm/backend/device/@path)" "false" "device path attribute is gone"

# --smartcard / --redirdev cases. Both structs already existed in
# virtinst-core; this is CLI optstring wiring only — see
# smartcard_from_optstr/redirdev_from_optstr's own doc comments in
# virtinst-cli/src/lib.rs.

echo
echo "=== edit-simple-smartcard ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-smartcard.xml"
echo "  cmd: --smartcard type=spicevmc"
xml="$(run_virt_xml --smartcard type=spicevmc)"
check_xpath "$xml" "string(//smartcard/@mode)" "host" "mode untouched"
check_xpath "$xml" "string(//smartcard/@type)" "spicevmc" "type newly added"

echo
echo "=== edit-simple-redirdev ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-redirdev.xml"
echo "  cmd: --redirdev type=spicevmc,server=example.com:12345"
xml="$(run_virt_xml --redirdev type=spicevmc,server=example.com:12345)"
check_xpath "$xml" "string(//redirdev/@bus)" "usb" "bus untouched"
check_xpath "$xml" "string(//redirdev/@type)" "spicevmc" "type changed"
check_xpath "$xml" "string(//redirdev/source/@mode)" "connect" "source mode untouched"
check_xpath "$xml" "string(//redirdev/source/@host)" "example.com" "source host changed"
check_xpath "$xml" "string(//redirdev/source/@service)" "12345" "source service changed"

# --filesystem case. Struct already existed in virtinst-core; this is
# CLI optstring wiring only — see filesystem_from_optstr's own doc
# comment in virtinst-cli/src/lib.rs (source/target's type-dependent
# mapping, hardcoded to source_dir).

echo
echo "=== edit-simple-filesystem ==="
echo "  upstream: tests/data/cli/compare/virt-xml-edit-simple-filesystem.xml"
echo "  cmd: --filesystem /1/2/3,/4/5/6,mode=mapped"
xml="$(run_virt_xml --filesystem /1/2/3,/4/5/6,mode=mapped)"
check_xpath "$xml" "string(//filesystem/@type)" "mount" "type untouched"
check_xpath "$xml" "string(//filesystem/@accessmode)" "mapped" "accessmode changed via mode alias"
check_xpath "$xml" "string(//filesystem/source/@dir)" "/1/2/3" "source dir changed"
check_xpath "$xml" "string(//filesystem/target/@dir)" "/4/5/6" "target dir changed"

# ---------------------------------------------------------------------
# virt-clone: upstream's whole "virt-clone tests" block in
# tests/test_cli.py. Compare cases check the full printed XML against
# tests/data/cli/compare/virt-clone-*.xml (structurally, xmlcmp.py,
# attribute order included); invalid cases grep the
# error the same way upstream's add_invalid does. Like upstream's
# harness, compare cases get --print-xml --__test-nodry appended, so the
# clone really runs against the test driver too. URI-KVM-X86 is
# testdriver.xml here: virt-clone reads nothing from its fake caps.
# The "remote" category needs MagicURI's fake remote URI: not covered.
# ---------------------------------------------------------------------

CLONE_BIN="$REPO_ROOT/target/debug/virt-clone"
XMLCMP="$REPO_ROOT/.scratch/rust-conversion/harness/xmlcmp.py"
CLONEXMLDIR="$UPSTREAM_REF/tests/data/cli/virtclone"
COMPAREDIR="$UPSTREAM_REF/tests/data/cli/compare"
URI_FULL="test://$UPSTREAM_REF/tests/data/testdriver/testdriver.xml"
URI_SUITE="test://$TESTSUITE_XML"
CLONE_UNMANAGED="--original-xml $CLONEXMLDIR/clone-disk.xml"
CLONE_MANAGED="--original-xml $CLONEXMLDIR/clone-disk-managed.xml"
CLONE_NOEXIST="--original-xml $CLONEXMLDIR/clone-disk-noexist.xml"
CLONE_NVRAM="--original-xml $CLONEXMLDIR/clone-nvram-auto.xml"
CLONE_NVRAM_NEWPOOL="--original-xml $CLONEXMLDIR/clone-nvram-newpool.xml"
CLONE_NVRAM_MISSING="--original-xml $CLONEXMLDIR/clone-nvram-missing.xml"
CLONE_EMPTY="--original-xml $CLONEXMLDIR/clone-empty.xml"
CLONE_NET_RBD="--original-xml $CLONEXMLDIR/clone-net-rbd.xml"
CLONE_NET_HTTP="--original-xml $CLONEXMLDIR/clone-net-http.xml"
CLONE_SERIAL="--original-xml $CLONEXMLDIR/clone-serial.xml"
# test_cli.py's EXIST_FILES / NEW_FILES.
EXISTIMG1=/pool-dir/testvol1.img
EXISTIMG3=/tmp/__virtinst_cli_exist3.img
EXISTIMG4=/tmp/__virtinst_cli_exist4.img
NEWCLONEIMG1=/tmp/__virtinst_cli_new1.img
NEWCLONEIMG2=/tmp/__virtinst_cli_new2.img
NEWCLONEIMG3=/tmp/__virtinst_cli_new3.img
NEWIMG1=/pool-dir/new1.img
CLONE_TMPFILES=(/tmp/__virtinst_cli_exist1.img /tmp/__virtinst_cli_exist2.img "$EXISTIMG3" "$EXISTIMG4")

clone_cleanup() {
    rm -f /tmp/__virtinst_cli_new*.img /tmp/__virtinst_cli_exist*-clone.img
}

# Run from the upstream checkout: one case passes the relative path
# `virt-install`, an existing file there.
run_clone() {
    (cd "$UPSTREAM_REF" && VIRTINST_TEST_PREDICTABLE=1 VIRTINST_TEST_SUITE=1 "$CLONE_BIN" "$@")
}

# clone_compare NAME ARGS... — upstream's add_compare.
clone_compare() {
    local name="$1"
    shift
    local args=("$@")
    if [[ " ${args[*]} " != *" --print-xml "* ]]; then
        args+=(--print-xml --__test-nodry)
    fi
    clone_cleanup
    local actual expected out
    actual="$(mktemp)"
    expected="$(mktemp)"
    # Only the XML: --__test-nodry's run prints after it.
    run_clone "${args[@]}" 2>/dev/null | sed '/^$/,$d' >"$actual" || true
    sed "s|TESTSUITE_SCRUBBED|$UPSTREAM_REF|g" "$COMPAREDIR/virt-clone-$name.xml" >"$expected"
    if out="$(python3 "$XMLCMP" "$actual" "$expected" 2>&1)"; then
        echo "  PASS: compare $name"
        pass=$((pass + 1))
    else
        echo "  FAIL: compare $name"
        echo "        $out"
        fail=$((fail + 1))
    fi
    rm -f "$actual" "$expected"
}

# clone_valid ARGS... — upstream's add_valid.
clone_valid() {
    clone_cleanup
    local out
    if out="$(run_clone "$@" 2>&1)"; then
        echo "  PASS: valid $*"
        pass=$((pass + 1))
    else
        echo "  FAIL: valid $*"
        echo "        $out"
        fail=$((fail + 1))
    fi
}

# clone_invalid GREP ARGS... — upstream's add_invalid.
clone_invalid() {
    local grep="$1"
    shift
    clone_cleanup
    local out
    if out="$(run_clone "$@" 2>&1)"; then
        echo "  FAIL: invalid (succeeded) $*"
        fail=$((fail + 1))
    elif [[ "$out" == *"$grep"* ]]; then
        echo "  PASS: invalid [$grep]"
        pass=$((pass + 1))
    else
        echo "  FAIL: invalid [$grep]"
        echo "        $out"
        fail=$((fail + 1))
    fi
}

touch "${CLONE_TMPFILES[@]}"

echo
echo "=== virt-clone: misc ==="
clone_compare clone-auto1 --connect "$URI_FULL" -o test-clone --auto-clone
clone_compare clone-auto2 --connect "$URI_FULL" -o test-clone-simple --name newvm --auto-clone --reflink
clone_compare clone-nvram --connect "$URI_FULL" $CLONE_NVRAM --auto-clone
clone_compare clone-nvram-path --connect "$URI_FULL" $CLONE_NVRAM --auto-clone --nvram /nvram/my-custom-path
clone_compare nvram-newpool --connect "$URI_FULL" $CLONE_NVRAM_NEWPOOL --auto-clone
clone_compare nvram-missing --connect "$URI_FULL" $CLONE_NVRAM_MISSING --auto-clone
clone_compare nvram-missing-preserve --connect "$URI_FULL" $CLONE_NVRAM_MISSING --auto-clone --preserve
clone_compare clone-manual --connect "$URI_FULL" -o test-clone -n test-newclone --mac 12:34:56:1A:B2:C3 --mac 12:34:56:1A:B7:C3 --uuid 12345678-12F4-1234-1234-123456789AFA --file /dev/pool-logical/newclone1.img --file /pool-dir/newclone2.img --skip-copy=hdb --force-copy=sdb --file /pool-dir/newclone3.img
clone_compare empty --connect "$URI_SUITE" $CLONE_EMPTY --auto-clone --print-xml
clone_compare pool-test-cross-pool --connect "$URI_FULL" -o test-clone-simple --auto -f /foo.img --print-xml
clone_compare auto-managed --connect "$URI_SUITE" $CLONE_MANAGED --auto-clone
clone_compare auto-unmanaged --connect "$URI_SUITE" $CLONE_UNMANAGED --auto-clone
clone_compare serial --connect "$URI_SUITE" $CLONE_SERIAL --auto-clone
clone_valid --connect "$URI_FULL" -o test-clone --auto-clone --nonsparse
clone_valid --connect "$URI_FULL" -o test-clone-simple -n newvm --preserve-data --file "$EXISTIMG1"
clone_valid --connect "$URI_SUITE" -n clonetest $CLONE_UNMANAGED --file "$EXISTIMG3" --file "$EXISTIMG4" --check path_exists=off
clone_valid --connect "$URI_SUITE" -n clonetest $CLONE_UNMANAGED --auto-clone --mac 22:11:11:11:11:11 --check all=off
clone_invalid "--check mac_in_use=off" --connect "$URI_SUITE" -n clonetest $CLONE_UNMANAGED --auto-clone --mac 22:11:11:11:11:11
clone_invalid "An original machine name is required" --connect "$URI_SUITE" --auto-clone
clone_invalid "use '--name NEW_VM_NAME'" --connect "$URI_SUITE" $CLONE_EMPTY --file foo
clone_invalid "Invalid name for new guest" --connect "$URI_SUITE" $CLONE_EMPTY --auto-clone -n test
clone_invalid "shutoff" --connect "$URI_SUITE" -o test --auto-clone
clone_invalid "Clone onto existing storage volume is not currently supported" --connect "$URI_FULL" -o test-clone-simple -n newvm --file "$EXISTIMG1"
clone_invalid "not enough free space" --connect "$URI_FULL" -o test-clone-full --auto-clone
clone_invalid "'http' is not cloneable" --connect "$URI_SUITE" $CLONE_NET_HTTP --auto-clone
clone_invalid "Cloning rbd volumes is not yet supported" --connect "$URI_FULL" $CLONE_NET_RBD --auto-clone

echo
echo "=== virt-clone: general ==="
G=(--connect "$URI_SUITE" -n clonetest)
clone_valid "${G[@]}" $CLONE_EMPTY --auto-clone --replace
clone_valid "${G[@]}" $CLONE_EMPTY --file "$NEWCLONEIMG1" --file "$NEWCLONEIMG2"
clone_valid "${G[@]}" $CLONE_EMPTY --file "$NEWCLONEIMG1" --file "$NEWCLONEIMG2" --prompt
clone_valid "${G[@]}" $CLONE_UNMANAGED --file "$NEWCLONEIMG1" --file "$NEWCLONEIMG2"
clone_valid "${G[@]}" $CLONE_UNMANAGED --file "$NEWCLONEIMG1" --skip-copy=hda
clone_compare unmanaged-preserve "${G[@]}" $CLONE_UNMANAGED --file virt-install --file "$EXISTIMG1" --preserve
clone_valid "${G[@]}" $CLONE_UNMANAGED --file "$NEWCLONEIMG1" --file "$NEWCLONEIMG2" --file "$NEWCLONEIMG3" --force-copy=hdc
clone_valid "${G[@]}" $CLONE_UNMANAGED --file "$NEWCLONEIMG1" --file "$NEWCLONEIMG2" --force-copy=fda
clone_valid "${G[@]}" $CLONE_MANAGED --file "$NEWIMG1"
clone_valid "${G[@]}" $CLONE_MANAGED --file "$NEWIMG1" --reflink
clone_compare replace --connect "$URI_FULL" -n clonetest -o test-clone -n test --auto-clone --replace
clone_valid "${G[@]}" $CLONE_MANAGED --auto-clone --force-copy fda
clone_invalid "Domain 'idontexist' was not found" "${G[@]}" -o idontexist --auto-clone
clone_invalid "Either --auto-clone or --file" "${G[@]}" $CLONE_UNMANAGED
clone_invalid "overwrite the existing path" "${G[@]}" $CLONE_UNMANAGED --file virt-install
clone_invalid "matching name 'default-vol'" "${G[@]}" $CLONE_MANAGED --file /tmp/clonevol
clone_valid "${G[@]}" $CLONE_NOEXIST --file "$EXISTIMG1" --preserve
# Self-verified (no upstream fixture): a missing --original-xml fails
# with Python's str(OSError), as upstream's open() does.
clone_invalid "ERROR    [Errno 2] No such file or directory: '/idontexist.xml'" "${G[@]}" --original-xml /idontexist.xml --auto-clone
# Upstream's next case ("StoragePool.install testsuite mocked failure")
# needs MagicURI's mocked pool failure: not covered.

clone_cleanup
rm -f "${CLONE_TMPFILES[@]}"

# Upstream's _add_argcomplete_cmd for virt-clone: bash completion through
# argcomplete's protocol (env vars in, completions out).
echo
echo "=== virt-clone: argcomplete ==="
clone_argcomplete() {
    local line="$1" grep="$2" out
    out="$(VIRTINST_TEST_SUITE=1 _ARGCOMPLETE=1 COMP_TYPE=9 COMP_LINE="$line" \
        COMP_POINT="${#line}" _ARGCOMPLETE_COMP_WORDBREAKS="\"'><;|&(:" "$CLONE_BIN" 2>&1)"
    if [[ "$out" == *"$grep"* ]]; then
        echo "  PASS: argcomplete [$line] -> $grep"
        pass=$((pass + 1))
    else
        echo "  FAIL: argcomplete [$line] -> $grep"
        echo "        $out"
        fail=$((fail + 1))
    fi
}
clone_argcomplete "virt-clone --preserve" "--preserve-data"
clone_argcomplete "virt-clone --check d" "disk_size"

# --version: argparse's version action prints BuildConfig.version alone
# (meson.build's project version) and exits, whatever else is given.
echo
echo "=== --version ==="
UPSTREAM_VERSION="$(sed -n "s/^  version: '\(.*\)',$/\1/p" "$UPSTREAM_REF/meson.build" | head -1)"
for tool in virt-install virt-clone virt-xml; do
    actual="$("$REPO_ROOT/target/debug/$tool" --connect test:///default --version 2>&1)"
    if [[ "$actual" == "$UPSTREAM_VERSION" ]]; then
        echo "  PASS: $tool --version"
        pass=$((pass + 1))
    else
        echo "  FAIL: $tool --version: expected '$UPSTREAM_VERSION', got '$actual'"
        fail=$((fail + 1))
    fi
done

# ---------------------------------------------------------------------
# Oracle: upstream's own virt-xml, run from $UPSTREAM_REF with the
# system libvirt/libxml2 Python bindings, against ours with the same
# arguments (oracle-virt-xml.txt). stdout, stderr and the exit code
# must match byte for byte. CI installs the bindings (ci.yml); locally,
# without them, this section is skipped with a note.
# ---------------------------------------------------------------------

ORACLE_PY=""
for py in python3 python3.12; do
    if command -v "$py" >/dev/null && "$py" -c "import libvirt, libxml2, requests" 2>/dev/null; then
        ORACLE_PY="$py"
        break
    fi
done

echo
echo "=== oracle: upstream virt-xml, byte for byte ==="
if [[ -z "$ORACLE_PY" ]]; then
    if [[ -n "${CI:-}" ]]; then
        echo "  FAIL: no Python with libvirt/libxml2/requests bindings for the oracle"
        fail=$((fail + 1))
    else
        echo "  SKIP: no Python with libvirt/libxml2/requests bindings (apt install python3-libvirt python3-libxml2 python3-requests)"
    fi
else
    ORACLE_TMP="$(mktemp -d)"
    while IFS= read -r line; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        line="${line//@SUITE@/test://$TESTSUITE_XML}"
        line="${line//@FULL@/test://$UPSTREAM_REF/tests/data/testdriver/testdriver.xml}"
        read -ra oargs <<<"$line"
        up_rc=0
        (cd "$UPSTREAM_REF" && "$ORACLE_PY" ./virt-xml "${oargs[@]}") \
            >"$ORACLE_TMP/up.out" 2>"$ORACLE_TMP/up.err" </dev/null || up_rc=$?
        me_rc=0
        VIRTINST_TEST_SUITE=1 "$REPO_ROOT/target/debug/virt-xml" "${oargs[@]}" \
            >"$ORACLE_TMP/me.out" 2>"$ORACLE_TMP/me.err" </dev/null || me_rc=$?
        if [[ "$up_rc" == "$me_rc" ]] && cmp -s "$ORACLE_TMP/up.out" "$ORACLE_TMP/me.out" \
            && cmp -s "$ORACLE_TMP/up.err" "$ORACLE_TMP/me.err"; then
            echo "  PASS: virt-xml $line"
            pass=$((pass + 1))
        else
            echo "  FAIL: virt-xml $line"
            echo "        exit code: upstream $up_rc, ours $me_rc"
            diff "$ORACLE_TMP/up.out" "$ORACLE_TMP/me.out" | head -10 | sed 's/^/        stdout /' || true
            diff "$ORACLE_TMP/up.err" "$ORACLE_TMP/me.err" | head -10 | sed 's/^/        stderr /' || true
            fail=$((fail + 1))
        fi
    done <"$REPO_ROOT/.scratch/rust-conversion/harness/oracle-virt-xml.txt"
    rm -rf "$ORACLE_TMP"
fi

echo
echo "$pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
