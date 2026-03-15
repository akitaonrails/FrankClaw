#![forbid(unsafe_code)]

/// Terminal color and table rendering utilities.
///
/// Respects `NO_COLOR` env and `TERM=dumb` conventions.

/// ANSI 256-color codes used throughout the CLI.
pub mod colors {
    pub const ACCENT: u8 = 33;  // blue
    pub const SUCCESS: u8 = 32; // green
    pub const WARN: u8 = 33;    // yellow (SGR, not 256)
    pub const ERROR: u8 = 31;   // red
    pub const INFO: u8 = 36;    // cyan
    pub const MUTED: u8 = 90;   // dim gray
}

/// Returns true if color output should be used.
pub fn is_color_enabled() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if let Ok(term) = std::env::var("TERM") {
        if term == "dumb" {
            return false;
        }
    }
    true
}

/// Wrap `text` in SGR escape sequences for the given color code.
pub fn colored(text: &str, color_code: u8) -> String {
    if !is_color_enabled() {
        return text.to_string();
    }
    format!("\x1b[{color_code}m{text}\x1b[0m")
}

/// Bold variant of colored.
pub fn bold_colored(text: &str, color_code: u8) -> String {
    if !is_color_enabled() {
        return text.to_string();
    }
    format!("\x1b[1;{color_code}m{text}\x1b[0m")
}

/// Severity badge with color.
pub fn severity_badge(severity: &str) -> String {
    let (label, color) = match severity {
        "CRIT" => ("[CRIT]", colors::ERROR),
        "HIGH" => ("[HIGH]", colors::WARN),
        " MED" | "MED" => ("[ MED]", colors::INFO),
        " LOW" | "LOW" => ("[ LOW]", 37), // white
        "INFO" => ("[INFO]", colors::MUTED),
        other => return format!("[{other}]"),
    };
    bold_colored(label, color)
}

/// Doctor check badges with color.
pub fn check_badge(prefix: &str) -> String {
    match prefix {
        "[PASS]" => colored("[PASS]", colors::SUCCESS),
        "[WARN]" => colored("[WARN]", colors::WARN),
        "[FAIL]" => bold_colored("[FAIL]", colors::ERROR),
        "[INFO]" => colored("[INFO]", colors::MUTED),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_wraps_with_sgr() {
        let result = colored("hello", colors::ERROR);
        // Contains the text regardless of color support
        assert!(result.contains("hello"));
    }

    #[test]
    fn bold_colored_wraps_with_sgr() {
        let result = bold_colored("warn", colors::WARN);
        assert!(result.contains("warn"));
    }

    #[test]
    fn severity_badge_formats_correctly() {
        let crit = severity_badge("CRIT");
        assert!(crit.contains("CRIT"));

        let info = severity_badge("INFO");
        assert!(info.contains("INFO"));

        // Unknown severity passes through
        let unknown = severity_badge("CUSTOM");
        assert_eq!(unknown, "[CUSTOM]");
    }

    #[test]
    fn check_badge_formats_correctly() {
        let pass = check_badge("[PASS]");
        assert!(pass.contains("PASS"));

        let fail = check_badge("[FAIL]");
        assert!(fail.contains("FAIL"));

        // Unknown badge passes through
        let other = check_badge("[OTHER]");
        assert_eq!(other, "[OTHER]");
    }
}
