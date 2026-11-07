use kdl::KdlValue;

use crate::ParseError;
use crate::kdl_io::{FromKdl, Node, ToKdl};

/// A point in a take's media time, in microseconds from the start of the take.
///
/// Integer microseconds keep the files exact and diffable. Frames are a render concern: the
/// renderer snaps to the output frame grid, so no file depends on a frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Micros(pub u64);

impl FromKdl for Micros {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let n = value
            .as_integer()
            .ok_or_else(|| "expected microseconds as an integer".to_owned())?;
        u64::try_from(n)
            .map(Self)
            .map_err(|_| format!("{n} is not a valid media time"))
    }
}

impl ToKdl for Micros {
    /// Writes digits in groups of three (`12_500_000`) so seconds are readable in a diff.
    fn write_kdl(&self, out: &mut String) {
        write_grouped(self.0, out);
    }
}

/// Writes `n` with its digits in groups of three, the canonical form of every integer that is
/// not a count or a percent.
pub(crate) fn write_grouped(n: u64, out: &mut String) {
    let digits = n.to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(c);
    }
}

/// A non-empty half-open range `[from, to)` of media time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    from: Micros,
    to: Micros,
}

impl TimeRange {
    /// Returns `None` unless `from < to`.
    pub fn new(from: Micros, to: Micros) -> Option<Self> {
        (from < to).then_some(Self { from, to })
    }

    pub fn from(self) -> Micros {
        self.from
    }

    pub fn to(self) -> Micros {
        self.to
    }

    pub fn duration(self) -> Micros {
        Micros(self.to.0 - self.from.0)
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.from < other.to && other.from < self.to
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        Self::new(self.from.max(other.from), self.to.min(other.to))
    }

    pub(crate) fn read(node: Node<'_>) -> Result<Self, ParseError> {
        let (from, to) = (node.prop("from")?, node.prop("to")?);
        Self::new(from, to).ok_or_else(|| node.error("`from` must be before `to`"))
    }

    /// Checks that the ranges are in order and do not overlap.
    pub(crate) fn check_ordered<'a>(
        items: impl IntoIterator<Item = (Self, Node<'a>)>,
    ) -> Result<(), ParseError> {
        let mut end = None;
        for (range, node) in items {
            if end.is_some_and(|end| range.from < end) {
                return Err(node.error(format!(
                    "`{}` ranges must be in order and must not overlap",
                    node.name()
                )));
            }
            end = Some(range.to);
        }
        Ok(())
    }
}

/// An identifier: lowercase ASCII letters, digits and dashes, starting with a letter.
///
/// Used for take ids, template and parameter names, and derived cut names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

impl Id {
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        let mut chars = text.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        valid.then_some(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromKdl for Id {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = String::from_kdl(value)?;
        Self::new(text.as_str()).ok_or_else(|| {
            format!("`{text}` is not an id (lowercase letters, digits and dashes, starting with a letter)")
        })
    }
}

impl ToKdl for Id {
    /// Bare unless the id collides with a KDL keyword.
    fn write_kdl(&self, out: &mut String) {
        if matches!(self.as_str(), "true" | "false" | "null" | "inf" | "nan") {
            crate::kdl_io::quote(self.as_str(), out);
        } else {
            out.push_str(self.as_str());
        }
    }
}
