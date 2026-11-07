//! Reading and writing the KDL project files.
//!
//! Parsing uses the `kdl` crate for the KDL v2 grammar and its source spans. Writing is done here
//! from the typed model instead of through the crate's format-preserving document: the canonical
//! form (fixed node and property order, four-space indent) is what makes the round trip
//! byte-identical and keeps diffs down to real changes.

use std::collections::HashSet;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use crate::ParseError;

/// The text of one project file, used to turn spans into line-numbered errors.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    pub file: &'static str,
    pub text: &'a str,
}

impl<'a> Source<'a> {
    pub fn parse(self) -> Result<KdlDocument, ParseError> {
        self.text.parse::<KdlDocument>().map_err(|error| {
            let first = error.diagnostics.first();
            let offset = first.map_or(0, |d| d.span.offset());
            let message = first
                .and_then(|d| d.message.clone())
                .unwrap_or_else(|| "invalid KDL".to_owned());
            self.error(offset, message)
        })
    }

    pub fn error(self, offset: usize, message: impl Into<String>) -> ParseError {
        ParseError::at(self.file, self.text, offset, message)
    }

    pub fn nodes(self, doc: &'a KdlDocument) -> impl Iterator<Item = Node<'a>> {
        doc.nodes().iter().map(move |node| Node { src: self, node })
    }
}

/// One KDL node with typed, located accessors.
#[derive(Clone, Copy)]
pub(crate) struct Node<'a> {
    src: Source<'a>,
    node: &'a KdlNode,
}

impl<'a> Node<'a> {
    pub fn name(self) -> &'a str {
        self.node.name().value()
    }

    pub fn error(self, message: impl Into<String>) -> ParseError {
        self.src.error(self.node.span().offset(), message)
    }

    fn entry_error(self, entry: &KdlEntry, message: impl Into<String>) -> ParseError {
        self.src.error(entry.span().offset(), message)
    }

    /// A leaf node with exactly `args` arguments and properties from `props`.
    pub fn expect(self, args: usize, props: &[&str]) -> Result<(), ParseError> {
        self.check(ArgCount::Exactly(args), props, false)
    }

    /// Like [`Node::expect`], with a child block allowed.
    pub fn expect_block(self, args: usize, props: &[&str]) -> Result<(), ParseError> {
        self.check(ArgCount::Exactly(args), props, true)
    }

    /// A leaf node with one or more arguments.
    pub fn expect_list(self) -> Result<(), ParseError> {
        self.check(ArgCount::AtLeast(1), &[], false)
    }

    /// A leaf node with any number of arguments, including none, and properties from `props`.
    pub fn expect_any(self, props: &[&str]) -> Result<(), ParseError> {
        self.check(ArgCount::AtLeast(0), props, false)
    }

    fn check(self, args: ArgCount, props: &[&str], block: bool) -> Result<(), ParseError> {
        let name = self.name();
        if self.node.ty().is_some() {
            return Err(self.error(format!("`{name}` takes no type annotation")));
        }
        let mut seen = HashSet::new();
        let mut count = 0;
        for entry in self.node.entries() {
            if entry.ty().is_some() {
                return Err(self.entry_error(entry, "values take no type annotation"));
            }
            match entry.name() {
                None => count += 1,
                Some(key) => {
                    let key = key.value();
                    if !props.contains(&key) {
                        return Err(
                            self.entry_error(entry, format!("`{name}` has no property `{key}`"))
                        );
                    }
                    if !seen.insert(key) {
                        return Err(self.entry_error(entry, format!("duplicate property `{key}`")));
                    }
                }
            }
        }
        match args {
            ArgCount::Exactly(n) if count != n => {
                return Err(self.error(format!("`{name}` takes {n} argument(s), found {count}")));
            }
            ArgCount::AtLeast(n) if count < n => {
                return Err(self.error(format!("`{name}` takes at least {n} argument(s)")));
            }
            _ => {}
        }
        if !block && self.node.children().is_some() {
            return Err(self.error(format!("`{name}` takes no children")));
        }
        Ok(())
    }

    fn args_raw(self) -> impl Iterator<Item = &'a KdlEntry> {
        self.node.entries().iter().filter(|e| e.name().is_none())
    }

    fn convert<T: FromKdl>(self, entry: &KdlEntry) -> Result<T, ParseError> {
        T::from_kdl(entry.value()).map_err(|message| self.entry_error(entry, message))
    }

    pub fn arg<T: FromKdl>(self, index: usize) -> Result<T, ParseError> {
        let entry = self.args_raw().nth(index).ok_or_else(|| {
            self.error(format!(
                "`{}` is missing argument {}",
                self.name(),
                index + 1
            ))
        })?;
        self.convert(entry)
    }

    pub fn args<T: FromKdl>(self) -> Result<Vec<T>, ParseError> {
        self.args_raw().map(|e| self.convert(e)).collect()
    }

    pub fn prop<T: FromKdl>(self, key: &str) -> Result<T, ParseError> {
        self.opt_prop(key)?
            .ok_or_else(|| self.error(format!("`{}` is missing property `{key}`", self.name())))
    }

    pub fn opt_prop<T: FromKdl>(self, key: &str) -> Result<Option<T>, ParseError> {
        self.node
            .entries()
            .iter()
            .find(|e| e.name().is_some_and(|n| n.value() == key))
            .map(|e| self.convert(e))
            .transpose()
    }

    pub fn children(self) -> impl Iterator<Item = Node<'a>> {
        self.node.iter_children().map(move |node| Node {
            src: self.src,
            node,
        })
    }

    pub fn unknown(self) -> ParseError {
        self.error(format!("unknown node `{}`", self.name()))
    }

    /// Stores `value` in a slot that may be set once.
    pub fn set_once<T>(self, slot: &mut Option<T>, value: T) -> Result<(), ParseError> {
        if slot.replace(value).is_some() {
            return Err(self.error(format!("duplicate `{}`", self.name())));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ArgCount {
    Exactly(usize),
    AtLeast(usize),
}

/// Takes a required node out of its slot, or reports it missing at `at`.
pub(crate) fn required<T>(
    slot: Option<T>,
    name: &str,
    at: impl FnOnce(String) -> ParseError,
) -> Result<T, ParseError> {
    slot.ok_or_else(|| at(format!("missing `{name}`")))
}

pub(crate) trait FromKdl: Sized {
    fn from_kdl(value: &KdlValue) -> Result<Self, String>;
}

pub(crate) trait ToKdl {
    fn write_kdl(&self, out: &mut String);
}

impl FromKdl for String {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        value
            .as_string()
            .map(str::to_owned)
            .ok_or_else(|| "expected a string".to_owned())
    }
}

impl ToKdl for String {
    fn write_kdl(&self, out: &mut String) {
        quote(self, out);
    }
}

impl FromKdl for bool {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        value
            .as_bool()
            .ok_or_else(|| "expected #true or #false".to_owned())
    }
}

impl ToKdl for bool {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(if *self { "#true" } else { "#false" });
    }
}

macro_rules! integer_kdl {
    ($($ty:ty),+) => {$(
        impl FromKdl for $ty {
            fn from_kdl(value: &KdlValue) -> Result<Self, String> {
                let n = value.as_integer().ok_or_else(|| "expected an integer".to_owned())?;
                <$ty>::try_from(n).map_err(|_| format!("{n} is out of range"))
            }
        }

        impl ToKdl for $ty {
            fn write_kdl(&self, out: &mut String) {
                out.push_str(&self.to_string());
            }
        }
    )+};
}

integer_kdl!(u8, u16, i16, i32, i64);

/// Writes a quoted KDL v2 string.
pub(crate) fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Builds canonical KDL text line by line.
#[derive(Default)]
pub(crate) struct Writer {
    out: String,
    depth: usize,
}

impl Writer {
    pub fn node(&mut self, name: &str) -> &mut Self {
        for _ in 0..self.depth {
            self.out.push_str("    ");
        }
        self.out.push_str(name);
        self
    }

    pub fn arg(&mut self, value: &impl ToKdl) -> &mut Self {
        self.out.push(' ');
        value.write_kdl(&mut self.out);
        self
    }

    pub fn prop(&mut self, key: &str, value: &impl ToKdl) -> &mut Self {
        self.out.push(' ');
        self.out.push_str(key);
        self.out.push('=');
        value.write_kdl(&mut self.out);
        self
    }

    pub fn end(&mut self) {
        self.out.push('\n');
    }

    pub fn open(&mut self) {
        self.out.push_str(" {\n");
        self.depth += 1;
    }

    pub fn close(&mut self) {
        self.depth -= 1;
        self.node("}").end();
    }

    pub fn blank(&mut self) {
        self.out.push('\n');
    }

    pub fn finish(self) -> String {
        self.out
    }
}

/// A closed set of bare KDL keywords, such as a status or a layout.
macro_rules! keyword_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)+ }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($(#[$vmeta])* $variant,)+ }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The keyword as written in the project files.
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text,)+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl $crate::kdl_io::FromKdl for $name {
            fn from_kdl(value: &kdl::KdlValue) -> Result<Self, String> {
                let text = value.as_string().ok_or_else(|| "expected a keyword".to_owned())?;
                match text {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(format!(
                        "unknown value `{text}`, expected one of: {}",
                        [$($text),+].join(", ")
                    )),
                }
            }
        }

        impl $crate::kdl_io::ToKdl for $name {
            fn write_kdl(&self, out: &mut String) {
                out.push_str(self.as_str());
            }
        }
    };
}

pub(crate) use keyword_enum;
