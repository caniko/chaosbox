//! Centralized `TypeQL` literal encoding.
//!
//! All query values flow through here; user-controlled text is never
//! concatenated into `TypeQL` syntax elsewhere: queries are fixed string
//! constants, values are encoded by these functions (the driver has no
//! bound-parameter API for inline literals, so encoding correctness is
//! load-bearing).

use thiserror::Error;

/// Literal encoding failures: non-finite doubles have no `TypeQL` form.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EncodeError {
    /// NaN or infinite double.
    #[error("non-finite double")]
    NonFiniteDouble,
}

/// Encode a string as a `TypeQL` double-quoted literal.
///
/// Escapes `"`, `\` and C0 controls (`\n`, `\r`, `\t` short forms,
/// remaining controls as `\u00XX`). Printable Unicode passes through
/// unescaped. Output is always wrapped in `"..."`.
#[must_use]
pub fn str_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Fixed `\u00XX` escape without an intermediate allocation.
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let n = c as u32;
                out.push('\\');
                out.push('u');
                out.push(HEX[(n >> 12) as usize & 0xf] as char);
                out.push(HEX[(n >> 8) as usize & 0xf] as char);
                out.push(HEX[(n >> 4) as usize & 0xf] as char);
                out.push(HEX[n as usize & 0xf] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Encode an integer literal.
#[must_use]
pub fn int_lit(v: i64) -> String {
    v.to_string()
}

/// Encode a double literal. Rejects NaN/infinite (no `TypeQL` form).
/// Integral values render with `.0` so the server reads a double.
pub fn double_lit(v: f64) -> Result<String, EncodeError> {
    if !v.is_finite() {
        return Err(EncodeError::NonFiniteDouble);
    }
    let s = v.to_string();
    if s.contains(['.', 'e', 'E']) {
        Ok(s)
    } else {
        Ok(format!("{s}.0"))
    }
}

/// Encode a boolean literal.
#[must_use]
pub fn bool_lit(v: bool) -> String {
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(str_lit("plain"), "\"plain\"");
        assert_eq!(str_lit("a\"b"), "\"a\\\"b\"");
        assert_eq!(str_lit("a\\b"), "\"a\\\\b\"");
        assert_eq!(str_lit("a\nb\rc\td"), "\"a\\nb\\rc\\td\"");
        assert_eq!(str_lit("a\x07b"), "\"a\\u0007b\"");
    }

    #[test]
    fn strings_pass_unicode_and_query_like_text_through_safely() {
        assert_eq!(str_lit("héllo→世界"), "\"héllo→世界\"");
        // Adversarial: looks like a query terminator + new statement.
        let evil = "\"; delete $x isa code-entity; match $y";
        let lit = str_lit(evil);
        assert!(lit.starts_with('"') && lit.ends_with('"'));
        assert!(lit.contains("\\\";"));
    }

    #[test]
    fn doubles_reject_non_finite_and_keep_decimal_point() {
        assert_eq!(double_lit(0.5).unwrap(), "0.5");
        assert_eq!(double_lit(1.0).unwrap(), "1.0");
        assert_eq!(double_lit(-3.0).unwrap(), "-3.0");
        assert!(double_lit(f64::NAN).is_err());
        assert!(double_lit(f64::INFINITY).is_err());
        assert!(double_lit(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn ints_and_bools_render_plainly() {
        assert_eq!(int_lit(-42), "-42");
        assert_eq!(bool_lit(true), "true");
        assert_eq!(bool_lit(false), "false");
    }
}
