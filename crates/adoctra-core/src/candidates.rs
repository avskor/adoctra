//! Skipping offsets that cannot start a match. A scanner that tries each
//! char boundary in turn passes its necessary first-byte test here and jumps
//! to the next byte that passes it; every skipped offset would have matched
//! nothing. Candidate bytes are ASCII, so the returned offset is a char
//! boundary. Tests can turn skipping off to compare with the full scan.

/// The first offset from `at` whose byte satisfies `candidate`, or
/// `bytes.len()` when none does.
pub(crate) fn next(bytes: &[u8], at: usize, candidate: impl Fn(u8) -> bool) -> usize {
    #[cfg(test)]
    if tests::EVERY_OFFSET.with(std::cell::Cell::get) {
        return at;
    }
    bytes[at..]
        .iter()
        .position(|&b| candidate(b))
        .map_or(bytes.len(), |found| at + found)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;

    thread_local! {
        pub(super) static EVERY_OFFSET: Cell<bool> = const { Cell::new(false) };
    }

    /// Run `f` with skipping disabled: scanners try every offset again.
    pub(crate) fn every_offset<R>(f: impl FnOnce() -> R) -> R {
        EVERY_OFFSET.with(|flag| flag.set(true));
        let result = f();
        EVERY_OFFSET.with(|flag| flag.set(false));
        result
    }

    type Fragments = Vec<(
        std::ops::Range<usize>,
        String,
        Option<String>,
        bool,
        usize,
        crate::Span,
        Option<crate::AttributePart>,
    )>;

    /// A comparable view of a substitution stream and its fragments.
    pub(crate) fn text_snapshot(text: &crate::substitution_text::Text) -> (String, Fragments) {
        let fragments = text
            .fragments
            .iter()
            .map(|(range, f)| {
                let part = f.part.clone();
                (
                    range.clone(),
                    f.text.clone(),
                    f.decoded.clone(),
                    f.raw,
                    f.owner,
                    f.span,
                    part,
                )
            })
            .collect();
        (text.text.clone(), fragments)
    }

    /// Every concatenation of up to `depth` tokens, the empty string included.
    pub(crate) fn inputs(tokens: &[&str], depth: usize) -> Vec<String> {
        let mut inputs = vec![String::new()];
        let mut last = inputs.clone();
        for _ in 0..depth {
            last = last
                .iter()
                .flat_map(|s| tokens.iter().map(move |t| format!("{s}{t}")))
                .collect();
            inputs.extend(last.iter().cloned());
        }
        inputs.sort();
        inputs.dedup();
        inputs
    }

    #[test]
    fn next_finds_the_first_candidate_or_the_end() {
        let colon = |b: u8| b == b':';
        assert_eq!(super::next(b"ab:c:", 0, colon), 2);
        assert_eq!(super::next(b"ab:c:", 3, colon), 4);
        assert_eq!(super::next(b"abc", 1, colon), 3);
        assert_eq!(every_offset(|| super::next(b"abc", 1, colon)), 1);
        assert_eq!(super::next(b"abc", 1, colon), 3);
    }
}
