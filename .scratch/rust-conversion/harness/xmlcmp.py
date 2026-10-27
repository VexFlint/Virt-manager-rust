#!/usr/bin/env python3
"""Compare two XML documents structurally: same elements in the same
order, same attributes (in any order: virtinst-xml's DOM doesn't keep
attribute order yet) and same non-whitespace text.

Usage: xmlcmp.py ACTUAL EXPECTED   (exit 0 when equal, else prints the
first difference and exits 1)
"""
import sys
import xml.etree.ElementTree as ET


def walk(a, b, path):
    if a.tag != b.tag:
        return "%s: tag %r != %r" % (path, a.tag, b.tag)
    here = "%s/%s" % (path, a.tag)
    if a.attrib != b.attrib:
        return "%s: attributes %r != %r" % (here, a.attrib, b.attrib)
    if (a.text or "").strip() != (b.text or "").strip():
        return "%s: text %r != %r" % (here, a.text, b.text)
    ca, cb = list(a), list(b)
    if len(ca) != len(cb):
        return "%s: %d children != %d: %r vs %r" % (
            here, len(ca), len(cb), [c.tag for c in ca], [c.tag for c in cb])
    for x, y in zip(ca, cb):
        err = walk(x, y, here)
        if err:
            return err
    return None


def main():
    actual = ET.parse(sys.argv[1]).getroot()
    expected = ET.parse(sys.argv[2]).getroot()
    err = walk(actual, expected, "")
    if err:
        print(err)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
