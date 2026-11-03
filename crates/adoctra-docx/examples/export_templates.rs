//! Write the built-in templates as `book.docx` and `article.docx` into a
//! directory, as a starting point for a custom template:
//!
//! ```sh
//! cargo run -p adoctra-docx --example export_templates -- <dir>
//! ```

fn main() {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: export_templates <dir>");
        std::process::exit(2);
    };
    let dir = std::path::PathBuf::from(dir);
    for builtin in [adoctra_docx::Builtin::Book, adoctra_docx::Builtin::Article] {
        let path = dir.join(format!("{}.docx", builtin.name()));
        if let Err(e) = std::fs::write(&path, builtin.to_docx()) {
            eprintln!("{}: {e}", path.display());
            std::process::exit(1);
        }
        println!("{}", path.display());
    }
}
