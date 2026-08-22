//! Diagnostic rendering: source carets.
//!
//! ```text
//! error: unresolved name 'y'
//!   --> 1:6
//!    |
//!  1 | x => y
//!    |      ^
//! ```

use crate::diag::Diag;

pub fn render(source: &str, diag: &Diag) -> String {
    let mut out = format!("error: {}\n", diag.message);
    if let Some((line, col)) = diag.span {
        out.push_str(&format!("  --> {line}:{col}\n"));
        out.push_str("   |\n");
        if let Some(text) = source.lines().nth((line as usize).saturating_sub(1)) {
            let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
            out.push_str(&format!(" {line} | {text}\n"));
            out.push_str(&format!("   | {caret}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Stage;

    #[test]
    fn renders_the_offending_line_with_a_caret() {
        // `y` is at line 1, column 6 — the caret lands under it.
        let diag = Diag::new(Stage::Resolve, (1, 6), "unresolved name 'y'".to_string());
        let out = render("x => y", &diag);
        assert_eq!(
            out,
            "error: unresolved name 'y'\n  --> 1:6\n   |\n 1 | x => y\n   |      ^\n"
        );
    }

    #[test]
    fn a_spanless_diagnostic_has_no_caret() {
        let diag = Diag {
            span: None,
            message: "internal".to_string(),
            stage: Stage::Check,
            check: None,
        };
        assert_eq!(render("x", &diag), "error: internal\n");
    }
}
