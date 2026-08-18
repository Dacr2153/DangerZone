//! Safe PDF metadata.
//!
//! The reconstructed PDF is built from scratch, so its `/Info` dictionary is
//! authored by the sanitizer rather than inherited from the untrusted input.
//! Only a fixed producer/creator string is written: no timestamps are emitted,
//! so nothing about when (or by whom) the original document was created leaks
//! into the safe output.

/// The name reported as the producer and creator of the safe PDF.
pub const PRODUCER_NAME: &str = "Dangerzone-RS";

/// Returns the literal `/Info` dictionary for the safe PDF.
///
/// `version` is the application version string (see
/// `dz_core::util::get_version`). The returned value is a complete PDF
/// dictionary that can be embedded as an object:
///
/// ```text
/// << /Producer (Dangerzone-RS 0.1.0) /Creator (Dangerzone-RS 0.1.0) >>
/// ```
pub fn info_dict(version: &str) -> String {
    let producer = format!("{PRODUCER_NAME} {version}");
    format!(
        "<< /Producer ({producer}) /Creator ({producer}) >>",
        producer = escape_pdf_string(&producer)
    )
}

/// Escapes the characters that have a special meaning inside a PDF literal
/// string.
fn escape_pdf_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_dict_reports_fixed_producer_and_creator() {
        let dict = info_dict("0.1.5");
        assert!(dict.contains("Dangerzone-RS 0.1.5"));
        assert!(dict.starts_with("<< /Producer ("));
        assert!(dict.ends_with(" >>"));
    }

    #[test]
    fn info_dict_escapes_parentheses_and_backslashes() {
        let dict = info_dict(r"(unlikely)\version");
        assert!(dict.contains(r"\(unlikely\)\\version"));
    }

    #[test]
    fn info_dict_contains_no_timestamps() {
        let dict = info_dict("0.1.5");
        assert!(!dict.contains("CreationDate"));
        assert!(!dict.contains("ModDate"));
    }
}
