//! Line normalization and indentation never mutate a source snapshot.
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Fragment {
    pub start: usize,
    pub end: usize,
    pub original_start: usize,
    pub original_end: usize,
    pub exact: bool,
    pub generated: bool,
}

#[derive(Clone)]
pub(crate) struct Line {
    pub original: Option<usize>,
    pub text: Arc<str>,
    pub eol: String,
    pub fragments: Vec<Fragment>,
}

impl Line {
    pub(crate) fn from_bounds(source: &str, start: usize, end: usize, strip_bom: bool) -> Self {
        let start =
            start + usize::from(strip_bom && source[start..end].starts_with('\u{feff}')) * 3;
        let suffix = if source[start..end].ends_with("\r\n") {
            2
        } else if source[start..end].ends_with('\n') {
            1
        } else {
            0
        };
        let body_end = end - suffix;
        let eol = if suffix == 2 { "\r\n" } else { "\n" };
        Self {
            original: Some(start),
            text: Arc::from(&source[start..body_end]),
            eol: eol.into(),
            fragments: vec![Fragment {
                start: 0,
                end: body_end - start + eol.len(),
                original_start: start,
                original_end: end,
                exact: true,
                generated: false,
            }],
        }
    }

    pub fn from_source(source: &str) -> Vec<Self> {
        Self::from_source_with_bom(source, true)
    }

    pub(crate) fn from_source_with_bom(source: &str, strip_bom: bool) -> Vec<Self> {
        let bom = usize::from(strip_bom && source.starts_with('\u{feff}')) * 3;
        crate::scanner::lines_with_offsets(&source[bom..])
            .into_iter()
            .map(|(off, text)| {
                let off = off + bom;
                let eol = if source[off + text.len()..].starts_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                Self {
                    original: Some(off),
                    text: Arc::from(text),
                    eol: eol.into(),
                    fragments: vec![Fragment {
                        start: 0,
                        end: text.len() + eol.len(),
                        original_start: off,
                        original_end: (off + text.len() + eol.len()).min(source.len()),
                        exact: true,
                        generated: false,
                    }],
                }
            })
            .collect()
    }

    pub fn generated(text: String) -> Self {
        Self {
            original: None,
            text: text.into(),
            eol: "\n".into(),
            fragments: Vec::new(),
        }
    }

    /// Include normalization recognizes a BOM after line/tag selection too.
    /// Called before other normalization, while the physical line has one
    /// contiguous fragment. Keep the snapshot and shift only the line's view.
    pub(crate) fn strip_bom(&mut self) {
        if let Some(text) = self.text.strip_prefix('\u{feff}') {
            self.text = Arc::from(text);
            self.original = self.original.map(|offset| offset + 3);
            debug_assert_eq!(self.fragments.len(), 1);
            let fragment = &mut self.fragments[0];
            fragment.end -= 3;
            fragment.original_start += 3;
        }
    }

    /// Ruby's rstrip removes ASCII whitespace and NUL, but keeps Unicode spaces.
    /// Split the newline from the retained text so subsequent lines and inline
    /// ranges continue to point at their physical bytes after removing a suffix.
    pub fn normalize(&mut self) {
        let retained = self.text.trim_end_matches(whitespace);
        if retained.len() == self.text.len() {
            return;
        }
        let original = self.original.expect("physical source line");
        let original_eol = original + self.text.len();
        let original_end = self.fragments.last().unwrap().original_end;
        let len = retained.len();
        self.text = Arc::from(retained);
        self.fragments.clear();
        if len > 0 {
            self.fragments.push(Fragment {
                start: 0,
                end: len,
                original_start: original,
                original_end: original + len,
                exact: true,
                generated: false,
            });
        }
        self.fragments.push(Fragment {
            start: len,
            end: len + self.eol.len(),
            original_start: original_eol,
            original_end,
            exact: true,
            generated: false,
        });
    }
}

fn whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0b' | '\x0c' | '\0')
}

// AsciiDoc readers rstrip before adjusting; raw resources only chomp.
fn normalized(line: &Line, normalize: bool) -> &str {
    if normalize {
        line.text.trim_end_matches(whitespace)
    } else {
        &line.text
    }
}

// Count expanded bytes and leading whitespace without allocating. Ruby tab
// stops count Unicode characters, not UTF-8 bytes.
fn dimensions(text: &str, tabsize: usize) -> Option<(usize, usize)> {
    let (mut bytes, mut column, mut leading) = (0usize, 0usize, 0usize);
    let mut in_indent = true;
    for c in text.chars() {
        let width = if c == '\t' && tabsize > 0 {
            tabsize - column % tabsize
        } else {
            1
        };
        bytes = bytes.checked_add(if c == '\t' { width } else { c.len_utf8() })?;
        column = column.checked_add(width)?;
        if in_indent && whitespace(c) {
            leading = leading.checked_add(width)?;
        } else {
            in_indent = false;
        }
    }
    Some((bytes, leading))
}

/// Return additional bytes charged to the host's include budget. Preflight the
/// whole transformation before allocating user-controlled indent/tab widths.
pub(crate) fn adjust(
    lines: &mut [Line],
    indent: i128,
    tabsize: i128,
    budget: usize,
    normalize: bool,
) -> Result<usize, ()> {
    let tabsize = usize::try_from(tabsize.max(0)).unwrap_or(usize::MAX);
    let requested = usize::try_from(indent.max(0)).map_err(|_| ())?;
    let mut common = usize::MAX;
    let mut metrics = Vec::with_capacity(lines.len());
    for line in lines.iter() {
        let value = normalized(line, normalize);
        let (size, leading) = dimensions(value, tabsize).ok_or(())?;
        if !value.is_empty() {
            common = common.min(leading);
        }
        metrics.push((size, !value.is_empty()));
    }
    if common == usize::MAX || indent < 0 {
        common = 0;
    }
    let mut growth = 0usize;
    for (line, &(size, nonempty)) in lines.iter().zip(&metrics) {
        let final_size = if indent >= 0 && nonempty {
            size.checked_sub(common)
                .and_then(|n| n.checked_add(requested))
                .ok_or(())?
        } else {
            size
        };
        // Bound the intermediate expanded buffer too, even when dedented later.
        let extra = size.max(final_size).saturating_sub(line.text.len());
        growth = growth
            .checked_add(extra)
            .filter(|n| *n <= budget)
            .ok_or(())?;
    }
    for (line, &(_, nonempty)) in lines.iter_mut().zip(&metrics) {
        let original = line.original.expect("physical include line");
        let original_eol = original + line.text.len();
        let original_end = line.fragments.last().unwrap().original_end;
        let mut expanded = String::new();
        let mut fragments: Vec<Fragment> = Vec::new();
        let mut column = 0;
        let value = normalized(line, normalize);
        let mut cursor = 0;
        for (offset, c) in value.char_indices() {
            if c == '\t' && tabsize > 0 {
                if offset > cursor {
                    let start = expanded.len();
                    expanded.push_str(&value[cursor..offset]);
                    fragments.push(Fragment {
                        start,
                        end: expanded.len(),
                        original_start: original + cursor,
                        original_end: original + offset,
                        exact: true,
                        generated: false,
                    });
                }
                let width = tabsize - column % tabsize;
                let start = expanded.len();
                expanded.extend(std::iter::repeat(' ').take(width));
                fragments.push(Fragment {
                    start,
                    end: expanded.len(),
                    original_start: original + offset,
                    original_end: original + offset + 1,
                    exact: false,
                    generated: false,
                });
                cursor = offset + 1;
                column += width;
            } else {
                column += 1;
            }
        }
        if cursor < value.len() {
            let start = expanded.len();
            expanded.push_str(&value[cursor..]);
            fragments.push(Fragment {
                start,
                end: expanded.len(),
                original_start: original + cursor,
                original_end: original + value.len(),
                exact: true,
                generated: false,
            });
        }
        let remove = if nonempty { common } else { 0 };
        let add = if indent >= 0 && nonempty {
            requested
        } else {
            0
        };
        let mut transformed = " ".repeat(add);
        transformed.push_str(&expanded[remove..]);
        fragments.retain(|f| f.end > remove);
        for fragment in &mut fragments {
            if fragment.exact {
                fragment.original_start += remove.saturating_sub(fragment.start);
            }
            fragment.start = fragment.start.saturating_sub(remove) + add;
            fragment.end = fragment.end - remove + add;
        }
        if add > 0 {
            fragments.insert(
                0,
                Fragment {
                    start: 0,
                    end: add,
                    original_start: original,
                    original_end: original,
                    exact: false,
                    generated: true,
                },
            );
        }
        fragments.push(Fragment {
            start: transformed.len(),
            end: transformed.len() + line.eol.len(),
            original_start: original_eol,
            original_end,
            exact: true,
            generated: false,
        });
        line.text = transformed.into();
        line.fragments = fragments;
    }
    Ok(growth)
}
