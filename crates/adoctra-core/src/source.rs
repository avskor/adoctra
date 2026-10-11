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
}

impl SourceFile {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn position(&self, offset: usize) -> Option<SourcePosition> {
        self.text.get(..offset)?;
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        Some(SourcePosition {
            line: line + 1,
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
        let text = text.into();
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        let id = SourceId(self.files.len());
        self.files.push(SourceFile {
            name: name.into(),
            text,
            line_starts,
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
        let resource = provider.load(crate::ResourceRequest { target, referrer })?;
        if let Some(index) = self
            .files
            .iter()
            .position(|file| file.name == resource.name && file.text == resource.text)
        {
            return Ok(SourceId(index));
        }
        Ok(self.add(resource.name, resource.text))
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
