//! A unified diff between two contents.
//!
//! The drift gate compares the **committed** file with the file its generator
//! produces and shows the difference as a unified diff (SPEC §7).  The diff
//! is written here, in the standard library: a dependency for a hundred lines
//! of arithmetic is a supply-chain cost the project does not need (see the
//! dependency rule of `AGENTS.md`).
//!
//! The output is deterministic: the same two contents always produce the same
//! bytes, and every path in it is relative to the repository root.
//!
//! The diff is a **patch**: the newline at the end of a file is part of the
//! file, so a file that ends with a newline and one that does not differ — and
//! the difference shows up in the diff.  The `\ No newline at end of file`
//! marker stands behind the line it describes, so `patch` accepts the result.

/// The number of unchanged lines shown around a change.
const CONTEXT: usize = 3;

/// The largest table the line comparison may build.
///
/// The table holds one cell per pair of lines.  A pair of files that share no
/// line at all is the worst case; beyond this size the comparison gives up
/// and reports the whole block as replaced.  The result is still a correct
/// diff — a coarser one — and the gate never depends on its shape.
const MAX_CELLS: usize = 4_000_000;

/// The kind of one line of the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The line is in both contents.
    Equal,
    /// The line is only in the old content.
    Removed,
    /// The line is only in the new content.
    Added,
}

/// One line of the diff.
#[derive(Debug, Clone)]
struct Edit {
    kind: Kind,
    /// The line number in the old content (1-based), if it has one.
    old_no: Option<usize>,
    /// The line number in the new content (1-based), if it has one.
    new_no: Option<usize>,
    /// The text of the line, without its newline.
    text: Vec<u8>,
    /// True when the line ends with a newline.
    ///
    /// Only the last line of a content can lack it, and that difference is a
    /// difference: `"one\ntwo\n"` and `"one\ntwo"` are not the same file.
    newline: bool,
}

/// The diff of two contents: the header and the hunks.
///
/// The header names the file on both sides — the committed version and the
/// produced one — with the path relative to the repository root.
pub fn unified(path: &str, old: &[u8], new: &[u8]) -> String {
    if old == new {
        return String::new();
    }

    let old_lines = split(old);
    let new_lines = split(new);
    let edits = compare(&old_lines, &new_lines);

    let mut out = String::new();
    out.push_str(&format!("--- {path}\n"));
    out.push_str(&format!("+++ {path}\n"));

    for hunk in hunks(&edits) {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
        ));
        for edit in &hunk.edits {
            let sign = match edit.kind {
                Kind::Equal => ' ',
                Kind::Removed => '-',
                Kind::Added => '+',
            };
            out.push(sign);
            out.push_str(&String::from_utf8_lossy(&edit.text));
            out.push('\n');
            if !edit.newline {
                // The line does not end with a newline.  The marker belongs
                // right behind it: it describes that line, and `patch` reads
                // it there.
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }

    out
}

/// One hunk of the diff.
struct Hunk {
    old_start: usize,
    old_len: usize,
    new_start: usize,
    new_len: usize,
    edits: Vec<Edit>,
}

/// One line of a content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line<'a> {
    /// The bytes of the line, without its newline.
    text: &'a [u8],
    /// True when the line ends with a newline.
    newline: bool,
}

/// Splits a content into its lines.
///
/// The last line of a content that does not end with a newline is marked: it
/// is a different line than the same bytes with a newline.
fn split(content: &[u8]) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in content.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(Line {
                text: &content[start..index],
                newline: true,
            });
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(Line {
            text: &content[start..],
            newline: false,
        });
    }
    lines
}

/// Compares two contents and returns the edits that turn the old into the new.
fn compare(old: &[Line<'_>], new: &[Line<'_>]) -> Vec<Edit> {
    // The common prefix and the common suffix are equal by definition; only
    // the middle can differ.
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut edits = Vec::new();
    for (index, line) in old.iter().take(prefix).enumerate() {
        edits.push(Edit {
            kind: Kind::Equal,
            old_no: Some(index + 1),
            new_no: Some(index + 1),
            text: line.text.to_vec(),
            newline: line.newline,
        });
    }
    edits.extend(middle(
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
        prefix + 1,
        prefix + 1,
    ));
    for index in 0..suffix {
        let old_index = old.len() - suffix + index;
        let new_index = new.len() - suffix + index;
        edits.push(Edit {
            kind: Kind::Equal,
            old_no: Some(old_index + 1),
            new_no: Some(new_index + 1),
            text: old[old_index].text.to_vec(),
            newline: old[old_index].newline,
        });
    }
    edits
}

/// Compares the parts that differ and returns their edits.
///
/// The comparison is the longest common subsequence of the two parts: the
/// lines that both have are kept, the others are removed and added.
fn middle(old: &[Line<'_>], new: &[Line<'_>], old_base: usize, new_base: usize) -> Vec<Edit> {
    let mut edits = Vec::new();

    if old.is_empty() {
        for (index, line) in new.iter().enumerate() {
            edits.push(Edit {
                kind: Kind::Added,
                old_no: None,
                new_no: Some(new_base + index),
                text: line.text.to_vec(),
                newline: line.newline,
            });
        }
        return edits;
    }
    if new.is_empty() {
        for (index, line) in old.iter().enumerate() {
            edits.push(Edit {
                kind: Kind::Removed,
                old_no: Some(old_base + index),
                new_no: None,
                text: line.text.to_vec(),
                newline: line.newline,
            });
        }
        return edits;
    }

    if old.len().saturating_mul(new.len()) > MAX_CELLS {
        // The table would be too large: the whole block is replaced.  The diff
        // is coarser, not wrong — and the gate does not depend on its shape.
        for (index, line) in old.iter().enumerate() {
            edits.push(Edit {
                kind: Kind::Removed,
                old_no: Some(old_base + index),
                new_no: None,
                text: line.text.to_vec(),
                newline: line.newline,
            });
        }
        for (index, line) in new.iter().enumerate() {
            edits.push(Edit {
                kind: Kind::Added,
                old_no: None,
                new_no: Some(new_base + index),
                text: line.text.to_vec(),
                newline: line.newline,
            });
        }
        return edits;
    }

    // The table holds the length of the longest common subsequence of the two
    // suffixes; it is filled from the back, so the walk below can go forward.
    let width = new.len() + 1;
    let mut table = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }

    let mut i = 0;
    let mut j = 0;
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            edits.push(Edit {
                kind: Kind::Equal,
                old_no: Some(old_base + i),
                new_no: Some(new_base + j),
                text: old[i].text.to_vec(),
                newline: old[i].newline,
            });
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            edits.push(Edit {
                kind: Kind::Removed,
                old_no: Some(old_base + i),
                new_no: None,
                text: old[i].text.to_vec(),
                newline: old[i].newline,
            });
            i += 1;
        } else {
            edits.push(Edit {
                kind: Kind::Added,
                old_no: None,
                new_no: Some(new_base + j),
                text: new[j].text.to_vec(),
                newline: new[j].newline,
            });
            j += 1;
        }
    }
    while i < old.len() {
        edits.push(Edit {
            kind: Kind::Removed,
            old_no: Some(old_base + i),
            new_no: None,
            text: old[i].text.to_vec(),
            newline: old[i].newline,
        });
        i += 1;
    }
    while j < new.len() {
        edits.push(Edit {
            kind: Kind::Added,
            old_no: None,
            new_no: Some(new_base + j),
            text: new[j].text.to_vec(),
            newline: new[j].newline,
        });
        j += 1;
    }

    edits
}

/// Groups the edits into hunks with their context.
fn hunks(edits: &[Edit]) -> Vec<Hunk> {
    let changed: Vec<usize> = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| edit.kind != Kind::Equal)
        .map(|(index, _)| index)
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }

    // The changes that are closer to each other than twice the context belong
    // to the same hunk.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut start = changed[0];
    let mut end = changed[0];
    for index in changed.iter().skip(1) {
        if *index - end > 2 * CONTEXT {
            groups.push((start, end));
            start = *index;
        }
        end = *index;
    }
    groups.push((start, end));

    let mut hunks = Vec::with_capacity(groups.len());
    for (first, last) in groups {
        let from = first.saturating_sub(CONTEXT);
        let to = (last + CONTEXT + 1).min(edits.len());
        let slice = &edits[from..to];

        let old_start = slice
            .iter()
            .find_map(|edit| edit.old_no)
            .unwrap_or_default();
        let new_start = slice
            .iter()
            .find_map(|edit| edit.new_no)
            .unwrap_or_default();
        let old_len = slice.iter().filter(|edit| edit.old_no.is_some()).count();
        let new_len = slice.iter().filter(|edit| edit.new_no.is_some()).count();

        hunks.push(Hunk {
            old_start,
            old_len,
            new_start,
            new_len,
            edits: slice.to_vec(),
        });
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_content_has_no_diff() {
        assert_eq!(unified("a.md", b"one\ntwo\n", b"one\ntwo\n"), "");
    }

    #[test]
    fn a_changed_line_is_shown_with_its_context() {
        let diff = unified("a.md", b"one\ntwo\nthree\n", b"one\nTWO\nthree\n");
        assert_eq!(
            diff,
            "--- a.md\n+++ a.md\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"
        );
    }

    #[test]
    fn the_paths_are_repository_relative() {
        let diff = unified("docs/a.md", b"one\n", b"two\n");
        assert!(diff.starts_with("--- docs/a.md\n+++ docs/a.md\n"), "{diff}");
    }

    #[test]
    fn an_added_line_is_shown_as_an_addition() {
        let diff = unified("a.md", b"one\n", b"one\ntwo\n");
        assert_eq!(diff, "--- a.md\n+++ a.md\n@@ -1,1 +1,2 @@\n one\n+two\n");
    }

    #[test]
    fn a_removed_line_is_shown_as_a_removal() {
        let diff = unified("a.md", b"one\ntwo\n", b"one\n");
        assert_eq!(diff, "--- a.md\n+++ a.md\n@@ -1,2 +1,1 @@\n one\n-two\n");
    }

    #[test]
    fn far_apart_changes_become_two_hunks() {
        let old = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let new = b"1\nX\n3\n4\n5\n6\n7\n8\n9\nY\n";
        let diff = unified("a.md", old, new);
        assert_eq!(diff.matches("@@ -").count(), 2, "{diff}");
    }

    #[test]
    fn the_diff_is_deterministic() {
        let old = b"a\nb\nc\nd\n";
        let new = b"a\nx\nc\ny\n";
        assert_eq!(unified("a.md", old, new), unified("a.md", old, new));
    }

    #[test]
    fn a_missing_newline_is_marked() {
        let diff = unified("a.md", b"one\ntwo", b"one\nthree");
        assert!(diff.contains("\\ No newline at end of file"), "{diff}");
    }

    #[test]
    fn an_empty_old_file_starts_at_zero() {
        let diff = unified("a.md", b"", b"one\n");
        assert_eq!(diff, "--- a.md\n+++ a.md\n@@ -0,0 +1,1 @@\n+one\n");
    }

    #[test]
    fn a_missing_trailing_newline_is_a_difference() {
        // The two files differ in nothing but the newline at the end.
        let diff = unified("a.md", b"one\ntwo\n", b"one\ntwo");
        assert_eq!(
            diff,
            "--- a.md\n+++ a.md\n@@ -1,2 +1,2 @@\n one\n-two\n+two\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn the_marker_stands_behind_the_line_it_describes() {
        // The new side is the shorter one: its last line is the added line.
        let diff = unified("a.md", b"a\nb\nc\n", b"a\nb");
        assert_eq!(
            diff,
            "--- a.md\n+++ a.md\n@@ -1,3 +1,2 @@\n a\n-b\n-c\n+b\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn a_context_line_without_a_newline_carries_the_marker() {
        // Both sides lack the trailing newline: the last line is a context
        // line, and the marker stands behind it.
        let diff = unified("a.md", b"x\na\nb", b"y\na\nb");
        assert_eq!(
            diff,
            "--- a.md\n+++ a.md\n@@ -1,3 +1,3 @@\n-x\n+y\n a\n b\n\\ No newline at end of file\n"
        );
    }
}
