use std::fmt;

/// A project file that failed to parse, located by line and column (both 1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub file: &'static str,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub(crate) fn at(
        file: &'static str,
        text: &str,
        offset: usize,
        message: impl Into<String>,
    ) -> Self {
        let before = &text[..offset.min(text.len())];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Self {
            file,
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.file, self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}
