use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Source, ToKdl, Writer, keyword_enum, required};
use crate::{FormatKind, ParseError};

keyword_enum! {
    /// The board column a video sits in.
    pub enum Status {
        Ideas = "ideas",
        Scripting = "scripting",
        Ready = "ready",
        Recorded = "recorded",
        Editing = "editing",
        Shipped = "shipped",
    }
}

/// `video.kdl`: the card on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    pub title: String,
    pub status: Status,
    pub format: FormatKind,
    pub target_date: Option<Date>,
}

impl Video {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "video.kdl",
            text,
        };
        let doc = src.parse()?;
        let (mut title, mut status, mut format, mut target_date) = (None, None, None, None);
        for node in src.nodes(&doc) {
            node.expect(1, &[])?;
            match node.name() {
                "title" => node.set_once(&mut title, node.arg(0)?)?,
                "status" => node.set_once(&mut status, node.arg(0)?)?,
                "format" => node.set_once(&mut format, node.arg(0)?)?,
                "target-date" => node.set_once(&mut target_date, node.arg(0)?)?,
                _ => return Err(node.unknown()),
            }
        }
        let missing = |message| src.error(0, message);
        Ok(Self {
            title: required(title, "title", missing)?,
            status: required(status, "status", missing)?,
            format: required(format, "format", missing)?,
            target_date,
        })
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.node("title").arg(&self.title).end();
        w.node("status").arg(&self.status).end();
        w.node("format").arg(&self.format).end();
        if let Some(date) = &self.target_date {
            w.node("target-date").arg(date).end();
        }
        w.finish()
    }
}

/// A calendar date, written as `"YYYY-MM-DD"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    pub fn year(self) -> u16 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }
}

impl FromKdl for Date {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = String::from_kdl(value)?;
        let invalid = || format!("`{text}` is not a date like \"2026-11-20\"");
        let mut parts = text.split('-');
        let (Some(y), Some(m), Some(d), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        if y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return Err(invalid());
        }
        let (Ok(y), Ok(m), Ok(d)) = (y.parse(), m.parse(), d.parse()) else {
            return Err(invalid());
        };
        Self::new(y, m, d).ok_or_else(invalid)
    }
}

impl ToKdl for Date {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(&format!(
            "\"{:04}-{:02}-{:02}\"",
            self.year, self.month, self.day
        ));
    }
}
