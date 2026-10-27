//! Upstream's `xmlutil.py` helpers the CLI prints with: `diff` (Python's
//! `difflib.unified_diff`, ported exactly so `virt-xml --print-diff`
//! matches upstream line for line) and `unindent_device_xml`.

use std::collections::{HashMap, HashSet};

/// `difflib.SequenceMatcher(None, a, b)`, with its default `autojunk`.
struct SequenceMatcher<'a> {
    a: &'a [&'a str],
    b: &'a [&'a str],
    b2j: HashMap<&'a str, Vec<usize>>,
}

/// `(i, j, size)`: `a[i:i+size] == b[j:j+size]`.
type Match = (usize, usize, usize);

/// `(tag, i1, i2, j1, j2)`.
type Opcode = (&'static str, usize, usize, usize, usize);

impl<'a> SequenceMatcher<'a> {
    fn new(a: &'a [&'a str], b: &'a [&'a str]) -> Self {
        // `__chain_b`
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            let popular: HashSet<&str> = b2j
                .iter()
                .filter(|(_, idxs)| idxs.len() > ntest)
                .map(|(elt, _)| *elt)
                .collect();
            for elt in popular {
                b2j.remove(elt);
            }
        }
        SequenceMatcher { a, b, b2j }
    }

    /// `find_longest_match` (no junk function, so `bjunk` is empty).
    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len = HashMap::new();
            if let Some(js) = self.b2j.get(ai) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn get_matching_blocks(&self) -> Vec<Match> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = vec![];
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut non_adjacent = vec![];
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    non_adjacent.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            non_adjacent.push((i1, j1, k1));
        }
        non_adjacent.push((la, lb, 0));
        non_adjacent
    }

    fn get_opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut answer = vec![];
        for (ai, bj, size) in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    fn get_grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.get_opcodes();
        if codes.is_empty() {
            codes = vec![("equal", 0, 1, 0, 1)];
        }
        if codes[0].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[0];
            codes[0] = (
                tag,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        let last = codes.len() - 1;
        if codes[last].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[last];
            codes[last] = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = vec![];
        let mut group = vec![];
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }
}

/// `_format_range_unified`.
fn format_range_unified(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// `str.splitlines(True)`: lines keeping their terminators.
fn splitlines_keepends(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&s[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if bytes.get(i + 1) == Some(&b'\n') {
                    i + 1
                } else {
                    i
                };
                out.push(&s[start..=end]);
                i = end;
                start = end + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `difflib.unified_diff(a, b, fromfile, tofile)` over lines that keep
/// their newlines, joined.
fn unified_diff(a: &[&str], b: &[&str], fromfile: &str, tofile: &str) -> String {
    let mut out = String::new();
    let sm = SequenceMatcher::new(a, b);
    for (n, group) in sm.get_grouped_opcodes(3).into_iter().enumerate() {
        if n == 0 {
            out += &format!("--- {fromfile}\n+++ {tofile}\n");
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out += &format!(
            "@@ -{} +{} @@\n",
            format_range_unified(first.1, last.2),
            format_range_unified(first.3, last.4)
        );
        for (tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                for line in &a[i1..i2] {
                    out += &format!(" {line}");
                }
                continue;
            }
            if tag == "replace" || tag == "delete" {
                for line in &a[i1..i2] {
                    out += &format!("-{line}");
                }
            }
            if tag == "replace" || tag == "insert" {
                for line in &b[j1..j2] {
                    out += &format!("+{line}");
                }
            }
        }
    }
    out
}

/// `xmlutil.diff`.
pub fn diff(origstr: &str, newstr: &str, fromfile: &str, tofile: &str) -> String {
    let a = splitlines_keepends(origstr);
    let b = splitlines_keepends(newstr);
    unified_diff(&a, &b, fromfile, tofile)
}

/// `xmlutil.unindent_device_xml`: drop the first line's indentation from
/// every line.
pub fn unindent_device_xml(xml: &str) -> String {
    let lines: Vec<&str> = xml.lines().collect();
    let Some(first) = lines.first() else {
        return xml.to_string();
    };
    let unindent = first.chars().take_while(|c| *c == ' ').count();
    let mut ret = String::new();
    for line in lines {
        let strip = line
            .chars()
            .take(unindent)
            .take_while(|c| *c == ' ')
            .count();
        ret += &line[strip..];
        ret.push('\n');
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected outputs computed with Python 3.12's difflib.
    #[test]
    fn diff_matches_python_difflib() {
        let a = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
        let b = "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nNINE\nten\n";
        assert_eq!(
            diff(a, b, "Original XML", "Altered XML"),
            "--- Original XML\n+++ Altered XML\n@@ -1,3 +1,4 @@\n+zero\n one\n two\n three\n\
             @@ -6,5 +7,5 @@\n six\n seven\n eight\n-nine\n+NINE\n ten\n"
        );
        assert_eq!(diff(a, a, "x", "y"), "");
        assert_eq!(
            diff("a\nb\n", "a\n", "x", "y"),
            "--- x\n+++ y\n@@ -1,2 +1 @@\n a\n-b\n"
        );
    }

    /// Long inputs with repeated lines trip autojunk (len(b) >= 200).
    #[test]
    fn diff_matches_python_with_autojunk() {
        let mut a = String::new();
        for i in 0..250 {
            a += if i % 3 == 0 { "    </x>\n" } else { "  <y/>\n" };
            a += &format!("  <z n=\"{i}\"/>\n");
        }
        let b = a.replacen("<z n=\"100\"/>", "<z n=\"100\" m=\"1\"/>", 1);
        assert_eq!(
            diff(&a, &b, "Original XML", "Altered XML"),
            "--- Original XML\n+++ Altered XML\n@@ -199,7 +199,7 @@\n     </x>\n   \
             <z n=\"99\"/>\n   <y/>\n-  <z n=\"100\"/>\n+  <z n=\"100\" m=\"1\"/>\n   <y/>\n   \
             <z n=\"101\"/>\n     </x>\n"
        );
    }

    #[test]
    fn unindent_drops_the_first_lines_indentation() {
        assert_eq!(
            unindent_device_xml("    <a>\n      <b/>\n    </a>\n"),
            "<a>\n  <b/>\n</a>\n"
        );
    }
}
