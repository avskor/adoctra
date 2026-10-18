//! The restricted comparison grammar of Asciidoctor 2.0.23's Reader.
//! Only value comparisons are evaluated. Operands containing `{` are
//! substituted by the caller (Ruby's `sub_attributes` with `drop`), which may
//! execute set/counter expressions, left operand first.
use std::cmp::Ordering;

pub(crate) fn evaluate(
    expression: &str,
    substitute: &mut dyn FnMut(&str) -> String,
) -> Option<bool> {
    let expression = expression.trim_matches(|c: char| c.is_ascii_whitespace());
    // EvalExpressionRx finds the first operator after at least one character.
    // It is deliberately not quote-aware; substitution happens after splitting.
    for (index, _) in expression.char_indices().skip(1) {
        let rest = &expression[index..];
        let Some(operator) = ["==", "!=", "<=", ">=", "<", ">"]
            .into_iter()
            .find(|op| rest.starts_with(op) && rest.len() > op.len())
        else {
            continue;
        };
        let right = &rest[operator.len()..];
        let left = Value::resolve(expression[..index].trim_end_matches(' '), substitute);
        let right = Value::resolve(right.trim_start_matches(' '), substitute);
        let order = left.compare(&right);
        let equal = match (&left, &right) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            _ => order == Some(Ordering::Equal),
        };
        return Some(match operator {
            "==" => equal,
            "!=" => !equal,
            "<" => order == Some(Ordering::Less),
            ">" => order == Some(Ordering::Greater),
            "<=" => matches!(order, Some(Ordering::Less | Ordering::Equal)),
            ">=" => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
            _ => unreachable!(),
        });
    }
    None
}

enum Value {
    Nil,
    Bool(bool),
    String(String),
    Integer(String),
    Float(f64),
}

impl Value {
    fn resolve(raw: &str, substitute: &mut dyn FnMut(&str) -> String) -> Self {
        let quoted = (raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\''));
        // The pinned Ruby version removes only the opening quote (slice 1,
        // length - 1). Keep this quirk, including comparisons of mixed quotes.
        let value = if quoted { &raw[1..] } else { raw };
        let value = if value.contains('{') {
            substitute(value)
        } else {
            value.to_owned()
        };
        if quoted {
            Self::String(value)
        } else {
            match value.as_str() {
                "" => Self::Nil,
                "true" => Self::Bool(true),
                "false" => Self::Bool(false),
                s if s
                    .trim_end_matches(|c: char| c.is_ascii_whitespace())
                    .is_empty() =>
                {
                    Self::String(" ".into())
                }
                s if s.contains('.') => Self::Float(float_prefix(s)),
                s => Self::Integer(integer_prefix(s)),
            }
        }
    }

    fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::String(a), Self::String(b)) => Some(a.cmp(b)),
            (Self::Integer(a), Self::Integer(b)) => Some(compare_integers(a, b)),
            (Self::Float(a), Self::Float(b)) => a.partial_cmp(b),
            (Self::Integer(a), Self::Float(b)) => compare_integer_float(a, *b),
            (Self::Float(a), Self::Integer(b)) => {
                compare_integer_float(b, *a).map(Ordering::reverse)
            }
            _ => None,
        }
    }
}

// Integers remain decimal strings: Ruby integers do not overflow, and rounding
// them to f64 would silently change comparisons above 2^53.
fn integer_prefix(raw: &str) -> String {
    let raw = raw.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let negative = raw.starts_with('-');
    let raw = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    let end = digits_end(raw.as_bytes(), 0);
    let digits = raw[..end].replace('_', "");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        "0".into()
    } else if negative {
        format!("-{digits}")
    } else {
        digits.into()
    }
}

fn compare_integers(a: &str, b: &str) -> Ordering {
    let (an, bn) = (a.starts_with('-'), b.starts_with('-'));
    if an != bn {
        return bn.cmp(&an);
    }
    let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
    if an {
        order.reverse()
    } else {
        order
    }
}

fn compare_integer_float(integer: &str, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float.is_sign_positive() {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let whole = if float.abs() < 1.0 {
        "0".into()
    } else {
        format!("{:.0}", float.trunc())
    };
    let order = compare_integers(integer, &whole);
    Some(if order == Ordering::Equal {
        0.0_f64.partial_cmp(&float.fract()).unwrap()
    } else {
        order
    })
}

fn digits_end(bytes: &[u8], mut index: usize) -> usize {
    let start = index;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            index += 1;
        } else if bytes[index] == b'_'
            && index > start
            && bytes.get(index + 1).is_some_and(u8::is_ascii_digit)
        {
            index += 2;
        } else {
            break;
        }
    }
    index
}

fn float_prefix(raw: &str) -> f64 {
    let raw = raw.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let bytes = raw.as_bytes();
    let start = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut end = digits_end(bytes, start);
    let mut has_digits = end > start;
    if bytes.get(end) == Some(&b'.') {
        let fraction = digits_end(bytes, end + 1);
        // Ruby stops before a decimal point without a following digit;
        // in particular, 1.e2 is 1.0, not 100.0.
        if fraction == end + 1 {
            return raw[..end].replace('_', "").parse().unwrap_or(0.0);
        }
        has_digits |= fraction > end + 1;
        end = fraction;
    }
    if !has_digits {
        return 0.0;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let exp_start = end + 1 + usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exp_end = digits_end(bytes, exp_start);
        if exp_end > exp_start {
            end = exp_end;
        }
    }
    raw[..end].replace('_', "").parse().unwrap_or(0.0)
}
