//! Demonstrates the exact JSON an editor frontend receives (native run).
fn main() {
    let src = ":lang: ru\n\n= Title\n\ntext with *bold*\n\n* one\n* two\n\n----\nunterminated";
    println!("--- parse_json ---\n{}", adoctra_wasm::parse_json(src));
    println!("\n--- tokens_json ---\n{}", adoctra_wasm::tokens_json(src));
}
