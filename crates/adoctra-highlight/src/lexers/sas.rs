//! Hand-ported block of `Rouge::Lexers::SAS`: a word in a procedure is a
//! keyword of that procedure (`@proc_name`).
//!
//! Data: SAS.data_step_statements, SAS.sas_functions, SAS.proc_keywords.

use crate::generated::lexers::{
    DATA_SAS_DATA_STEP_STATEMENTS, DATA_SAS_PROC_KEYWORDS, DATA_SAS_SAS_FUNCTIONS,
};
use crate::lexer::{contains, LexError, Match, Scan};
use crate::token::{KEYWORD, NAME};

/// A word in a `proc` step.
pub(crate) fn rule_546(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let word = m.text().to_uppercase();
    let proc_keyword = || {
        let name = s.lexer.strings.get("proc_name")?;
        let (_, keywords) = DATA_SAS_PROC_KEYWORDS
            .iter()
            .find(|(proc_name, _)| proc_name == name)?;
        Some(contains(keywords, &word))
    };
    let keyword = contains(DATA_SAS_DATA_STEP_STATEMENTS, &word)
        || contains(DATA_SAS_SAS_FUNCTIONS, &word)
        || proc_keyword().unwrap_or(false);
    s.token(if keyword { KEYWORD } else { NAME }, m.text());
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::lexer::{find_def, Lexer, Options};

    #[test]
    fn entering_datalines4_fails_like_ruby() {
        let mut lexer = Lexer::new(find_def("sas").unwrap(), Options::new()).unwrap();
        let error = lexer.lex("data a;\ndatalines4;\n1;;;;\n").unwrap_err();
        assert_eq!(error.0, "state datalines4: Rouge::RegexLexer::InvalidRegex");
        // Other input never enters the state.
        assert!(lexer.lex("data a;\ndatalines;\n1\n;\n").is_ok());
    }
}
