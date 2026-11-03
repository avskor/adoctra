//! Table geometry: where each cell goes on Word's grid, column widths and
//! the borders of Ruby's `frame`/`grid`.

use adoctra_core::ast::{Table, TableCell};

/// One grid position of a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot<'a> {
    /// A cell starting in this row, spanning `span` columns from `column`.
    Cell {
        cell: &'a TableCell,
        column: usize,
        span: usize,
        /// The cell starts a vertical merge (rowspan > 1).
        merge: bool,
    },
    /// The continuation of a cell spanning rows from above.
    Continue { column: usize, span: usize },
    /// A position no cell covers (a short row).
    Empty { column: usize },
}

impl Slot<'_> {
    pub fn column(&self) -> usize {
        match self {
            Slot::Cell { column, .. } | Slot::Continue { column, .. } | Slot::Empty { column } => {
                *column
            }
        }
    }

    pub fn span(&self) -> usize {
        match self {
            Slot::Cell { span, .. } | Slot::Continue { span, .. } => *span,
            Slot::Empty { .. } => 1,
        }
    }
}

/// The slots of `rows` on a grid of `columns`: column spans are clipped to
/// the grid, row spans continue in the following rows.
pub fn place<'a>(rows: &[&'a [TableCell]], columns: usize) -> Vec<Vec<Slot<'a>>> {
    let columns = columns.max(1);
    // Per column: (rows still covered, span) of a cell from above.
    let mut covered: Vec<Option<(u32, usize)>> = vec![None; columns];
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut slots = Vec::new();
        let mut column = 0;
        let mut cells = row.iter();
        while column < columns {
            if let Some((_, span)) = covered[column] {
                slots.push(Slot::Continue { column, span });
                column += span;
                continue;
            }
            match cells.next() {
                Some(cell) => {
                    let span = (cell.colspan.unwrap_or(1).max(1) as usize).min(columns - column);
                    let rows_down = cell.rowspan.unwrap_or(1).max(1);
                    if rows_down > 1 {
                        covered[column] = Some((rows_down, span));
                    }
                    slots.push(Slot::Cell {
                        cell,
                        column,
                        span,
                        merge: rows_down > 1,
                    });
                    column += span;
                }
                None => {
                    slots.push(Slot::Empty { column });
                    column += 1;
                }
            }
        }
        // One more row of each row span is used up.
        for entry in covered.iter_mut() {
            if let Some((remaining, _)) = entry {
                *remaining -= 1;
                if *remaining == 0 {
                    *entry = None;
                }
            }
        }
        out.push(slots);
    }
    out
}

/// Column widths (twips) for a table `width` twips wide, from Ruby's
/// `colpcwidth` percentages (equal widths when they are missing).
pub fn column_widths(table: &Table, width: u32) -> Vec<u32> {
    let percents: Vec<f64> = table
        .columns
        .iter()
        .map(|c| c.width.trim().parse::<f64>().unwrap_or(0.0))
        .collect();
    let total: f64 = percents.iter().sum();
    let n = table.columns.len().max(1);
    if total <= 0.0 {
        return vec![width / n as u32; n];
    }
    percents
        .iter()
        .map(|p| (f64::from(width) * p / total).round() as u32)
        .collect()
}

/// Direct `w:tblBorders` for Ruby's `frame` (`all`, `ends`, `sides`,
/// `none`) and `grid` (`all`, `rows`, `cols`, `none`); `None` when both are
/// `all`, which the table style draws.
pub fn borders(frame: &str, grid: &str) -> Option<String> {
    if frame == "all" && grid == "all" {
        return None;
    }
    let line = |on: bool| {
        if on {
            r#"w:val="single" w:sz="4" w:space="0" w:color="DEDEDE""#
        } else {
            r#"w:val="nil""#
        }
    };
    let ends = matches!(frame, "all" | "ends" | "topbot");
    let sides = matches!(frame, "all" | "sides");
    let rows = matches!(grid, "all" | "rows");
    let cols = matches!(grid, "all" | "cols");
    Some(format!(
        "<w:tblBorders><w:top {}/><w:left {}/><w:bottom {}/><w:right {}/><w:insideH {}/><w:insideV {}/></w:tblBorders>",
        line(ends),
        line(sides),
        line(ends),
        line(sides),
        line(rows),
        line(cols)
    ))
}

/// A `w:shd` fill from a CSS color (`#rgb`, `#rrggbb` or a basic name).
pub fn fill(color: &str) -> Option<String> {
    let color = color.trim();
    if let Some(hex) = color.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => Some(
                hex.chars()
                    .flat_map(|c| [c, c])
                    .collect::<String>()
                    .to_uppercase(),
            ),
            6 => Some(hex.to_uppercase()),
            _ => None,
        };
    }
    Some(
        match color.to_ascii_lowercase().as_str() {
            "black" => "000000",
            "white" => "FFFFFF",
            "red" => "FF0000",
            "green" => "008000",
            "lime" => "00FF00",
            "blue" => "0000FF",
            "yellow" => "FFFF00",
            "aqua" | "cyan" => "00FFFF",
            "fuchsia" | "magenta" => "FF00FF",
            "gray" | "grey" => "808080",
            "silver" => "C0C0C0",
            "maroon" => "800000",
            "olive" => "808000",
            "navy" => "000080",
            "purple" => "800080",
            "teal" => "008080",
            "orange" => "FFA500",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use adoctra_core::ast::BlockKind;
    use adoctra_core::parse;

    fn table(src: &str) -> Table {
        let doc = parse(src).document;
        match &doc.blocks[0].kind {
            BlockKind::Table(t) => (**t).clone(),
            other => panic!("{other:?}"),
        }
    }

    fn shape(slots: &[Vec<Slot<'_>>]) -> Vec<String> {
        slots
            .iter()
            .map(|row| {
                row.iter()
                    .map(|s| match s {
                        Slot::Cell { span, merge, .. } => {
                            format!("C{span}{}", if *merge { "m" } else { "" })
                        }
                        Slot::Continue { span, .. } => format!("c{span}"),
                        Slot::Empty { .. } => "e".into(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn places_row_and_column_spans() {
        let t = table("[cols=3]\n|===\n.2+|a 2+|b\n|c |d\n|e |f |g\n|===\n");
        let rows: Vec<&[TableCell]> = t.body.iter().map(Vec::as_slice).collect();
        assert_eq!(shape(&place(&rows, 3)), ["C1m C2", "c1 C1 C1", "C1 C1 C1"]);
    }

    #[test]
    fn clips_spans_and_pads_short_rows() {
        let mut t = table("[cols=2]\n|===\n|a |b\n|c |d\n|===\n");
        // Spans Ruby would reject and rows it would complete, built directly.
        t.body[0][0].colspan = Some(3);
        t.body[0].truncate(1);
        t.body[1].truncate(1);
        let rows: Vec<&[TableCell]> = t.body.iter().map(Vec::as_slice).collect();
        assert_eq!(shape(&place(&rows, 2)), ["C2", "C1 e"]);
    }

    #[test]
    fn widths_follow_column_percentages() {
        let t = table("[cols=\"1,3\"]\n|===\n|a |b\n|===\n");
        assert_eq!(column_widths(&t, 8000), [2000, 6000]);
    }

    #[test]
    fn frame_and_grid_become_borders() {
        assert_eq!(borders("all", "all"), None);
        let b = borders("ends", "rows").unwrap();
        assert!(b.contains(r#"<w:top w:val="single""#));
        assert!(b.contains(r#"<w:left w:val="nil"/>"#));
        assert!(b.contains(r#"<w:insideH w:val="single""#));
        assert!(b.contains(r#"<w:insideV w:val="nil"/>"#));
        assert!(borders("none", "none").unwrap().matches("nil").count() == 6);
    }

    #[test]
    fn css_colors_become_fills() {
        assert_eq!(fill("#abc").as_deref(), Some("AABBCC"));
        assert_eq!(fill("#00ff7f").as_deref(), Some("00FF7F"));
        assert_eq!(fill("Red").as_deref(), Some("FF0000"));
        assert_eq!(fill("#zz0000"), None);
        assert_eq!(fill("rebeccapurple"), None);
    }
}
