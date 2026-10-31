//! Prints a lexer's tokens for an input file (debugging aid):
//! `cargo run -p adoctra-highlight --example lex_dump -- <tag> <file>`.

use adoctra_highlight::lexer::{find_def, Lexer, Options};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[2]).expect("input file");
    let def = find_def(&args[1]).expect("lexer");
    let mut lexer = Lexer::new(def, Options::new()).expect("lexer");
    match lexer.lex(&text) {
        Ok(tokens) => {
            for (token, value) in tokens {
                println!("{}\t{:?}", token.qualname(), value);
            }
        }
        Err(error) => println!("error: {error}"),
    }
}
