//! CLAUDE.md, checked against the crate it describes.
//!
//! An integration test on purpose: anything under `src/` would be counted by
//! its own scans — a `#[cfg(test)]` block added here to measure which modules
//! carry tests promptly made this module one of them, and the line-count
//! figures would drift by however long this file happens to be.
//!
//! Deliberately MIRRORED per crate rather than shared from a helper: each is
//! its own git repository that must build standalone, and this needs nothing
//! but std.

/// CLAUDE.md's claims about this crate's OWN tests, checked against the tree.
///
/// A test count is the most inviting kind of stale fact: it is concrete, it
/// reads as verified, and it is wrong the moment anyone adds a test. The
/// 2026-09-19 sweep found cce-window-manager documented at "~143 unit tests"
/// with 175, and its list of test-free modules naming `state.rs`, which had
/// grown tests — the list was right when written and had quietly inverted.
///
/// Four claim shapes are understood, and each is checked only if the doc
/// actually makes it. A count in one of them that this scan cannot read
/// fails the test rather than leaving the claim unchecked:
///
/// - `<N> modules carry unit tests` / `<N> modules have them` — how many
///   files under src/ contain a `#[test]` function.
/// - `<N> tests` / `~<N> unit tests` — total `#[test]` count. A `~` figure
///   gets a 10% tolerance; a bare one must be exact, because that is what
///   writing a bare number claims.
/// - ``  `x.rs` (the most  `` / `` `x.rs` has the most `` — that file has the
///   most `#[test]`s of any.
/// - `every module has one except `a.rs` … and `b.rs`` — exactly those lack
///   tests. Where the doc instead LISTS the modules that have them, the list
///   must match the real set exactly.
///
/// Deliberately MIRRORED per crate rather than shared: each is its own git
/// repository and must build standalone, and this needs nothing but std.
pub mod test_claims {
    use std::path::{Path, PathBuf};

    /// Every `.rs` under src/ that holds a `#[test]` function, with how many.
    ///
    /// Not every file with a `#[cfg(test)]`. The two agree here today, since
    /// each test module is an inline `#[cfg(test)] mod tests { … }` in the
    /// file it tests, the tray bridge's `src/bin/cce-xembed-tray/` modules
    /// included. But one split out as `#[cfg(test)] mod tests;` puts the gate
    /// in a parent with no tests of its own and the tests in a file with no
    /// gate, and that rule counted the one and missed the other
    /// (cce-compositor's `layout_probe.rs`).
    fn test_modules(root: &Path) -> Vec<(PathBuf, usize)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, usize)>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        let n = s.matches("#[test]").count();
                        if n > 0 {
                            out.push((p, n));
                        }
                    }
                }
            }
        }
        let mut v = Vec::new();
        walk(&root.join("src"), &mut v);
        v.sort();
        v
    }

    /// Every `.rs` under src/, tests or not — the denominator for "every
    /// module has one except …".
    fn all_sources(root: &Path) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        walk(&root.join("src"), &mut v);
        v.sort();
        v
    }

    const ONES: [&str; 20] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
        "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen",
        "seventeen", "eighteen", "nineteen",
    ];
    const TENS: [&str; 8] =
        ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

    /// A count as the docs write it: digits, or English words up to
    /// ninety-nine, hyphenated the usual way ("twenty-two").
    fn word_to_num(w: &str) -> Option<usize> {
        // Docs wrap these in punctuation — "(nine", "(~175", "48 tests:".
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if let Ok(n) = w.parse::<usize>() {
            return Some(n);
        }
        let lower = w.to_ascii_lowercase();
        let ones = |s: &str| ONES.iter().position(|x| *x == s);
        let tens = |s: &str| TENS.iter().position(|x| *x == s).map(|i| 20 + 10 * i);
        match lower.split_once('-') {
            None => ones(&lower).or_else(|| tens(&lower)),
            Some((t, o)) => Some(tens(t)? + ones(o).filter(|n| (1..=9).contains(n))?),
        }
    }

    /// Whether a token is trying to be a count, read or not: it carries a
    /// `~`, is digits with separators ("1,200"), or starts with a tens word
    /// ("twenty-too", "twentytwo"). "unit" before "tests" is prose and says
    /// nothing; one of these that `word_to_num` cannot read is a claim that
    /// would otherwise go unchecked in silence, which is the failure this
    /// whole file exists to prevent. Not any hyphenated number word: that
    /// would read "one-shot tests" as a count.
    fn looks_like_count(w: &str) -> bool {
        let t = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let lower = t.to_ascii_lowercase();
        w.contains('~')
            || (t.starts_with(|c: char| c.is_ascii_digit())
                && t.chars().all(|c| c.is_ascii_digit() || ",._".contains(c)))
            || TENS.iter().any(|x| lower.starts_with(x))
    }

    /// The token right before `phrase`'s first occurrence: `Ok` with the
    /// count and whether it was written with `~`, or `Err` with the token
    /// when it does not read as one. `None` when the phrase is absent.
    fn number_before(doc: &str, phrase: &str) -> Option<Result<(usize, bool), String>> {
        let at = doc.find(phrase)?;
        let word = doc[..at].split_whitespace().next_back()?;
        Some(match word_to_num(word) {
            Some(n) => Ok((n, word.contains('~'))),
            None => Err(word.to_string()),
        })
    }

    fn unreadable(word: &str, phrase: &str) -> String {
        format!(
            "\"{word} {phrase}\" — `{word}` is not a count this scan can read; write \
             it in digits or as words (\"twenty-two\"), or teach `word_to_num` the spelling"
        )
    }

    /// Backticked `*.rs` names from `phrase` up to the end of its sentence.
    fn names_after(doc: &str, phrase: &str) -> Vec<String> {
        let Some(at) = doc.find(phrase) else { return Vec::new() };
        let seg = &doc[at..];
        let end = seg.find(". ").unwrap_or(seg.len());
        seg[..end]
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|t| t.ends_with(".rs"))
            .map(|t| t.rsplit('/').next().unwrap_or(t).to_string())
            .collect()
    }

    fn base(p: &Path) -> String {
        p.file_name().and_then(|x| x.to_str()).unwrap_or_default().to_string()
    }

    /// cce-compositor's "Twenty-two modules carry unit tests" read as no
    /// number at all until 2026-10-13, and the count and the module list went
    /// unchecked with it.
    #[test]
    fn test_counts_read_as_the_docs_write_them() {
        for (w, n) in [
            ("Twenty-two", 22), ("(thirty-one", 31), ("twenty", 20), ("Nineteen", 19),
            ("ninety-nine", 99), ("(~175", 175), ("48", 48), ("zero", 0),
        ] {
            assert_eq!(word_to_num(w), Some(n), "{w}");
        }
        for w in ["twenty-zero", "twenty-ten", "two-twenty", "one-shot", "twentytwo", "carry"] {
            assert_eq!(word_to_num(w), None, "{w}");
        }
        for w in ["twenty-too", "twentytwo", "~many", "1,200"] {
            assert!(looks_like_count(w), "{w} should read as a count");
        }
        for w in ["unit", "one-shot", "e2e", "3D", "carry"] {
            assert!(!looks_like_count(w), "{w} should read as prose");
        }
        assert_eq!(
            number_before("Twenty-two modules carry unit tests", "modules carry unit tests"),
            Some(Ok((22, false)))
        );
        assert_eq!(
            number_before("Many modules carry unit tests", "modules carry unit tests"),
            Some(Err("Many".to_string()))
        );
    }

    #[test]
    fn test_claude_md_test_claims_match_the_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let doc = std::fs::read_to_string(root.join("CLAUDE.md"))
            .expect("CLAUDE.md is missing next to Cargo.toml");
        let mods = test_modules(root);
        let total: usize = mods.iter().map(|(_, n)| n).sum();
        let mut bad: Vec<String> = Vec::new();
        let mut checked = 0usize;

        // --- how many modules carry tests
        //
        // These phrases ARE the claim, so whatever stands before one is its
        // count: a token that does not read as a number fails outright.
        for phrase in ["modules carry unit tests", "modules have them", "modules have unit tests"] {
            let Some(read) = number_before(&doc, phrase) else { continue };
            checked += 1;
            match read {
                Err(word) => bad.push(unreadable(&word, phrase)),
                Ok((claimed, _)) => {
                    if claimed != mods.len() {
                        bad.push(format!(
                            "\"{claimed} {phrase}\" — {} modules actually do: {}",
                            mods.len(),
                            mods.iter().map(|(p, _)| base(p)).collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            }
            // Where the doc ENUMERATES them, the list must be the real
            // set. Two or more names means a list; a single one is the
            // "`browse.rs` has the most" shape, checked separately.
            let listed = names_after(&doc, phrase);
            if listed.len() >= 2 {
                let mut want: Vec<String> = mods.iter().map(|(p, _)| base(p)).collect();
                let mut got = listed.clone();
                want.sort();
                got.sort();
                got.dedup();
                if want != got {
                    bad.push(format!(
                        "the listed modules {got:?} are not the ones that have tests {want:?}"
                    ));
                }
            }
        }

        // --- total test count
        //
        // "tests" is everywhere in prose ("carry unit tests"), so here only a
        // token shaped like a count is a claim — but one that is and does
        // not read fails, rather than falling through to the next phrase.
        for phrase in ["unit tests", "tests"] {
            match number_before(&doc, phrase) {
                Some(Ok((claimed, approx))) => {
                    checked += 1;
                    let off = (total as f64 - claimed as f64) / claimed.max(1) as f64;
                    let stale = if approx { off.abs() > 0.10 } else { total != claimed };
                    if stale {
                        bad.push(format!(
                            "\"{}{claimed} {phrase}\" — the crate has {total}",
                            if approx { "~" } else { "" }
                        ));
                    }
                }
                Some(Err(word)) if looks_like_count(&word) => {
                    checked += 1;
                    bad.push(unreadable(&word, phrase));
                }
                _ => continue,
            }
            break; // "unit tests" wins; don't double-count its "tests" tail
        }

        // --- which file has the most
        for (name, phrase) in [("(the most", "(the most"), ("has the most", "has the most")] {
            let _ = name;
            if let Some(at) = doc.find(phrase) {
                let before = &doc[..at];
                let claimed = before.rfind('`').and_then(|e| {
                    before[..e].rfind('`').map(|s| before[s + 1..e].to_string())
                });
                if let Some(claimed) = claimed.filter(|c| c.ends_with(".rs")) {
                    checked += 1;
                    let claimed = claimed.rsplit('/').next().unwrap_or(&claimed).to_string();
                    if let Some((top, n)) = mods.iter().max_by_key(|(_, n)| *n) {
                        if base(top) != claimed {
                            bad.push(format!(
                                "`{claimed}` is called the one with the most tests, but {} has {n}",
                                base(top)
                            ));
                        }
                    }
                }
            }
        }

        // --- "every module has one except a.rs and b.rs"
        if doc.contains("every module has one except") {
            checked += 1;
            let claimed = names_after(&doc, "every module has one except");
            let with: Vec<String> = mods.iter().map(|(p, _)| base(p)).collect();
            let mut without: Vec<String> = all_sources(root)
                .iter()
                .map(|p| base(p))
                .filter(|b| !with.contains(b))
                .collect();
            let mut claimed = claimed;
            without.sort();
            without.dedup();
            claimed.sort();
            claimed.dedup();
            if claimed != without {
                bad.push(format!(
                    "the modules without tests are {without:?}, not {claimed:?}"
                ));
            }
        }

        assert!(
            checked > 0,
            "CLAUDE.md makes no test-count claim this scan recognizes — either the \
             wording changed and the scan needs updating, or the claims were removed \
             and so should this test"
        );
        assert!(bad.is_empty(), "CLAUDE.md test claims are stale:\n  {}", bad.join("\n  "));
    }
}
