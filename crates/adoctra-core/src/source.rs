//! Source identity and checked byte ranges. A caller owns one map per document
//! set and passes the registered id through `ParseOptions` when parsing a file.
//! Registration performs no I/O; explicit loading delegates to a host provider.

use crate::Span;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(pub usize);

/// One-based line and Unicode scalar column (not a UTF-16 editor column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    name: String,
    text: String,
    line_starts: Vec<usize>,
    complete: bool,
    omissions: Vec<crate::OmittedSourceLine>,
    frames: Option<crate::resource::SourceLineFrames>,
}

impl SourceFile {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether this snapshot covers the entire decoded resource. An include
    /// may retain an intact prefix or omit skipped undecodable lines. Ranges
    /// address `text`; `omitted_lines` describes gaps and `position` maps them
    /// to original physical line numbers.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Undecodable physical lines omitted from `text`, retaining their exact
    /// bytes and insertion points. Spans address retained UTF-8 snapshot text.
    pub fn omitted_lines(&self) -> &[crate::OmittedSourceLine] {
        &self.omissions
    }

    /// Resolve a diagnostic's raw byte range in an omitted physical line.
    pub fn omitted_slice(&self, span: crate::OmittedSourceSpan) -> Option<&[u8]> {
        self.omissions
            .iter()
            .find(|line| line.line == span.line)?
            .bytes
            .get(span.start..span.end)
    }

    pub(crate) fn include_lines(&self) -> Vec<crate::include_indent::Line> {
        crate::resource::framed_snapshot_lines(&self.text, &self.omissions, self.frames.as_ref())
    }

    pub(crate) fn has_line_frames(&self) -> bool {
        self.frames.is_some()
    }

    /// Whether the include path already removed Ruby's BOM (see
    /// [`crate::resource::SourceLineFrames::normalized`]).
    pub(crate) fn bom_normalized(&self) -> bool {
        self.frames.as_ref().is_some_and(|frames| frames.normalized)
    }

    pub(crate) fn stored_bytes(&self) -> usize {
        self.omissions.iter().fold(self.text.len(), |size, line| {
            size.saturating_add(line.bytes.len())
        })
    }

    pub fn position(&self, offset: usize) -> Option<SourcePosition> {
        self.text.get(..offset)?;
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        Some(SourcePosition {
            line: line
                + 1
                + self
                    .omissions
                    .iter()
                    .filter(|omitted| omitted.offset <= offset && omitted.bytes.ends_with(b"\n"))
                    .count(),
            column: self.text[self.line_starts[line]..offset].chars().count() + 1,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// IDs are stable within this map; two files may have the same display name.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> SourceId {
        self.add_snapshot(name.into(), text.into(), true, Vec::new(), None)
    }

    fn add_snapshot(
        &mut self,
        name: String,
        text: String,
        complete: bool,
        omissions: Vec<crate::OmittedSourceLine>,
        frames: Option<crate::resource::SourceLineFrames>,
    ) -> SourceId {
        let line_starts = if let Some(frames) = &frames {
            let mut starts: Vec<_> = frames.ranges.iter().map(|&(start, _)| start).collect();
            if starts.is_empty() {
                starts.push(0);
            }
            if frames.trailing_lf && starts.last() != Some(&text.len()) {
                starts.push(text.len());
            }
            starts
        } else {
            let mut starts = vec![0];
            starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
            starts
        };
        let id = SourceId(self.files.len());
        self.files.push(SourceFile {
            name,
            text,
            line_starts,
            complete,
            omissions,
            frames,
        });
        id
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceFile> {
        self.files.get(id.0)
    }

    /// Load through an explicitly supplied provider, resolving relative to a
    /// registered source when given. Reuse an identical named snapshot; changed
    /// contents receive a new id, preserving all previously issued spans.
    /// A failed load never modifies the map. No implicit I/O fallback exists.
    pub fn load(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        provider: &dyn crate::ResourceProvider,
    ) -> crate::ResourceResult<SourceId> {
        self.load_using(target, relative_to, |request| {
            provider.load(request).map(Into::into)
        })
    }

    /// Load a primary document like Ruby's `Asciidoctor.load_file`: the bytes
    /// pass `prepare_source_string`, so a UTF-16LE/BE BOM decodes the whole
    /// source (spans address the decoded UTF-8 snapshot) and other input must
    /// be valid UTF-8 (`InvalidEncoding` otherwise). A UTF-8 BOM stays in the
    /// snapshot for the reader. Text-only providers keep using `load`.
    pub fn load_document(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        provider: &dyn crate::ResourceProvider,
    ) -> crate::ResourceResult<SourceId> {
        self.load_using(target, relative_to, |request| {
            crate::resource::load_source_text(provider, request).map(Into::into)
        })
    }

    /// Register an include after decoding its external encoding. Source spans
    /// address the decoded UTF-8 snapshot. Repeated loads reuse an id only when
    /// both the canonical name and decoded text match; different decoding of
    /// the same bytes cannot invalidate earlier source ranges.
    pub fn load_with_encoding(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        provider: &dyn crate::ResourceProvider,
        encoding: Option<&str>,
    ) -> crate::ResourceResult<SourceId> {
        self.load_using(target, relative_to, |request| {
            provider
                .load_with_encoding(request, encoding)
                .map(Into::into)
        })
    }

    /// Register a complete include snapshot, or an explicitly incomplete
    /// decoded prefix recovered for a finite line selection. Completeness is
    /// part of snapshot identity, along with canonical name and decoded text.
    pub fn load_include(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        provider: &dyn crate::ResourceProvider,
        encoding: Option<&str>,
        line_limit: Option<usize>,
    ) -> crate::ResourceResult<SourceId> {
        self.load_using(target, relative_to, |request| {
            provider
                .load_include(request, encoding, line_limit)
                .map(Into::into)
        })
    }

    pub(crate) fn load_selected_include(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        provider: &dyn crate::ResourceProvider,
        encoding: Option<&str>,
        selection: &crate::include_selection::Selection,
    ) -> crate::ResourceResult<(SourceId, Option<crate::include_selection::Selected>)> {
        let mut prepared = None;
        let id = self.load_using(target, relative_to, |request| {
            match provider.prepare_include(request, encoding, selection.line_limit())? {
                crate::IncludeInput::Decoded(resource) => Ok(resource.into()),
                crate::IncludeInput::Deferred { resource, binary } => selection
                    .recover_bytes(resource, binary)
                    .map(|(snapshot, selected)| {
                        prepared = selected;
                        snapshot
                    }),
                crate::IncludeInput::Unconverted { resource, encoding } => selection
                    .recover_unconverted(resource, encoding.as_deref())
                    .map(|(snapshot, selected)| {
                        prepared = selected;
                        snapshot
                    }),
            }
        })?;
        if let Some(selected) = &mut prepared {
            for diagnostic in &mut selected.diagnostics {
                diagnostic.span.source_id = id;
            }
        }
        Ok((id, prepared))
    }

    fn load_using(
        &mut self,
        target: &str,
        relative_to: Option<SourceId>,
        load: impl FnOnce(
            crate::ResourceRequest<'_>,
        ) -> crate::ResourceResult<crate::resource::IncludeSnapshot>,
    ) -> crate::ResourceResult<SourceId> {
        let referrer = relative_to
            .map(|id| {
                self.get(id).map(SourceFile::name).ok_or_else(|| {
                    crate::ResourceError::new(
                        crate::ResourceErrorKind::InvalidTarget,
                        target,
                        format!("unknown referring source id {}", id.0),
                    )
                })
            })
            .transpose()?;
        let crate::resource::IncludeSnapshot {
            decoded: crate::IncludeResource { resource, complete },
            omissions,
            frames,
        } = load(crate::ResourceRequest { target, referrer })?;
        if let Some(index) = self.files.iter().position(|file| {
            file.name == resource.name
                && file.text == resource.text
                && file.complete == complete
                && file.omissions == omissions
                && file.frames == frames
        }) {
            return Ok(SourceId(index));
        }
        Ok(self.add_snapshot(resource.name, resource.text, complete, omissions, frames))
    }

    /// Invalid ids, reversed ranges and non-UTF-8 boundaries return None.
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.get(span.source_id)?.text.get(span.start..span.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_reader_frames_participate_in_identity_without_inserting_text() {
        fn snapshot(
            ranges: Vec<(usize, usize)>,
            trailing_lf: bool,
        ) -> crate::resource::IncludeSnapshot {
            crate::resource::IncludeSnapshot {
                decoded: crate::IncludeResource {
                    resource: crate::Resource {
                        name: "/frames.adoc".into(),
                        text: "a上b".into(),
                    },
                    complete: false,
                },
                omissions: Vec::new(),
                frames: Some(crate::resource::SourceLineFrames {
                    ranges,
                    trailing_lf,
                    normalized: false,
                }),
            }
        }
        let mut sources = SourceMap::new();
        let first = sources
            .load_using("/frames.adoc", None, |_| {
                Ok(snapshot(vec![(0, 4), (4, 5)], false))
            })
            .unwrap();
        assert_eq!(
            sources
                .load_using("/frames.adoc", None, |_| Ok(snapshot(
                    vec![(0, 4), (4, 5)],
                    false
                )))
                .unwrap(),
            first
        );
        let second = sources
            .load_using("/frames.adoc", None, |_| {
                Ok(snapshot(vec![(0, 1), (1, 5)], false))
            })
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(sources.get(first).unwrap().text(), "a上b");
        assert_eq!(sources.get(second).unwrap().text(), "a上b");
        assert_eq!(
            sources.get(first).unwrap().position(4),
            Some(SourcePosition { line: 2, column: 1 })
        );
        assert_eq!(
            sources.get(second).unwrap().position(4),
            Some(SourcePosition { line: 2, column: 2 })
        );
        assert_eq!(sources.get(first).unwrap().position(2), None);
        let lines = sources.get(first).unwrap().include_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text.as_ref(), "a上");
        assert_eq!(lines[1].text.as_ref(), "b");
        assert_eq!(sources.slice(Span::in_source(first, 0, 4)), Some("a上"));
        let eof = sources
            .load_using("/frames.adoc", None, |_| {
                Ok(snapshot(vec![(0, 4), (4, 5)], true))
            })
            .unwrap();
        assert_ne!(eof, first);
        assert_eq!(
            sources.get(eof).unwrap().position(5),
            Some(SourcePosition { line: 3, column: 1 })
        );
    }

    #[test]
    fn unicode_crlf_and_end_of_file_positions_are_checked() {
        let mut sources = SourceMap::new();
        let id = sources.add("example.adoc", "Я🙂\r\nnext\n");
        let file = sources.get(id).unwrap();
        assert_eq!(
            file.position(6),
            Some(SourcePosition { line: 1, column: 3 })
        );
        assert_eq!(
            file.position(8),
            Some(SourcePosition { line: 2, column: 1 })
        );
        assert_eq!(
            file.position(13),
            Some(SourcePosition { line: 3, column: 1 })
        );
        assert_eq!(file.position(1), None);
        assert_eq!(file.position(14), None);
        assert_eq!(sources.slice(Span::in_source(id, 2, 6)), Some("🙂"));
        assert_eq!(sources.slice(Span::in_source(id, 6, 2)), None);
        assert_eq!(sources.slice(Span::in_source(SourceId(99), 0, 0)), None);
    }
}
