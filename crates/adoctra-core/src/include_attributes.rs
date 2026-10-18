//! Include parameters use the shared AttributeList grammar. Substitution
//! happens on the whole list, before tokenization. Unlike block lists, include
//! lists do not request `sub_result`: single quotes only delimit a value.
use std::collections::BTreeMap;

pub(crate) fn parse(input: &str) -> BTreeMap<String, String> {
    let mut named = BTreeMap::new();
    // Positional fields have no meaning to the built-in include reader.
    for field in crate::attribute_list::parse(input) {
        let Some(name) = field.name(input) else {
            continue;
        };
        let value = field.text(input).unwrap_or_default();
        if matches!(name, "opts" | "options") {
            // Only a comma-separated options value deletes spaces;
            // tabs and spaces in a single value remain significant.
            if value.contains(',') {
                for option in value.replace(' ', "").split(',').filter(|s| !s.is_empty()) {
                    named.insert(format!("{option}-option"), String::new());
                }
            } else if !value.is_empty() {
                named.insert(format!("{value}-option"), String::new());
            }
        } else {
            named.insert(name.to_owned(), value);
        }
    }
    named
}
