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
        crate::resource::snapshot_lines(&self.text, &self.omissions)
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
        self.add_snapshot(name.into(), text.into(), true, Vec::new())
    }

    fn add_snapshot(
        &mut self,
        name: String,
        text: String,
        complete: bool,
        omissions: Vec<crate::OmittedSourceLine>,
    ) -> SourceId {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        let id = SourceId(self.files.len());
        self.files.push(SourceFile {
            name,
            text,
            line_starts,
            complete,
            omissions,
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
    ) -> crate::ResourceResult<SourceId> {
        self.load_using(target, relative_to, |request| {
            match provider.prepare_include(request, encoding, selection.line_limit())? {
                crate::IncludeInput::Decoded(resource) => Ok(resource.into()),
                crate::IncludeInput::Deferred { resource, binary } => {
                    selection.recover_bytes(resource, binary)
                }
            }
        })
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
        } = load(crate::ResourceRequest { target, referrer })?;
        if let Some(index) = self.files.iter().position(|file| {
            file.name == resource.name
                && file.text == resource.text
                && file.complete == complete
                && file.omissions == omissions
        }) {
            return Ok(SourceId(index));
        }
        Ok(self.add_snapshot(resource.name, resource.text, complete, omissions))
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
