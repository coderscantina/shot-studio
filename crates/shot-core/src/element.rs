//! Elements: the committed facts of a take in `elements/<take>.jsonl`.
//!
//! An element is identified by its kind, its take and its start time `t`. A re-cut never moves
//! it, and style rules select elements by kind and attributes ([0019]). The file is JSON Lines
//! sorted by `t`; this module parses it from text and writes it back canonical, with no IO.
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md

use std::fmt;

use serde_json::Value;

use crate::kdl_io::keyword_enum;
use crate::{Id, Micros, ParseError, Point, TimeRange};

keyword_enum! {
    /// The closed set of element kinds. Order is the canonical order of elements that share a `t`.
    pub enum ElementKind {
        Word = "word",
        Sentence = "sentence",
        Silence = "silence",
        Pause = "pause",
        Filler = "filler",
        Retake = "retake",
        Misspoken = "misspoken",
        Emphasis = "emphasis",
        Url = "url",
        Code = "code",
        Product = "product",
        Click = "click",
        Typing = "typing",
        Idle = "idle",
        Ocr = "ocr",
        Face = "face",
        Expression = "expression",
        Chapter = "chapter",
        Beat = "beat",
        Tag = "tag",
        Marker = "marker",
        Claim = "claim",
        Punchline = "punchline",
        Aside = "aside",
        TopicShift = "topic-shift",
        NeedsSource = "needs-source",
    }
}

impl ElementKind {
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == text)
    }

    /// Whether elements of this kind are a moment (`t` only) rather than a span (`t` and `to`).
    pub fn is_point(self) -> bool {
        matches!(
            self,
            Self::Click | Self::Chapter | Self::Beat | Self::Marker | Self::TopicShift
        )
    }

    /// The attributes an element of this kind must carry, then the ones it may carry.
    pub fn attrs(self) -> (&'static [Attr], &'static [Attr]) {
        use Attr::*;
        match self {
            Self::Word | Self::Code => (&[Text], &[]),
            Self::Filler | Self::Typing | Self::Chapter => (&[], &[Text]),
            Self::Url => (&[Url, Host], &[]),
            Self::Product => (&[Name], &[]),
            Self::Tag => (&[Name], &[]),
            Self::Click => (&[X, Y], &[]),
            Self::Ocr => (&[Text, X, Y, Width, Height], &[]),
            Self::Face => (&[X, Y, Width, Height], &[]),
            Self::Expression => (&[Label, Strength], &[]),
            Self::Beat => (&[Index], &[]),
            Self::Marker => (&[Label], &[]),
            _ => (&[], &[]),
        }
    }
}

keyword_enum! {
    /// The closed set of element attributes, in canonical order.
    pub enum Attr {
        Text = "text",
        Url = "url",
        Host = "host",
        Name = "name",
        Label = "label",
        Index = "index",
        X = "x",
        Y = "y",
        Width = "width",
        Height = "height",
        Strength = "strength",
    }
}

impl Attr {
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.as_str() == text)
    }

    /// Whether the attribute holds an integer rather than text.
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Index | Self::X | Self::Y | Self::Width | Self::Height | Self::Strength
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AttrValue {
    Text(String),
    Integer(i64),
}

impl fmt::Display for AttrValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(s) => f.write_str(s),
            Self::Integer(n) => write!(f, "{n}"),
        }
    }
}

/// One fact of a take. `t` and `to` are the take's media time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub kind: ElementKind,
    pub t: Micros,
    /// The end of a span, `None` for a point kind.
    pub to: Option<Micros>,
    /// How sure the finding is, 0 to 100. Facts without a confidence are certain.
    pub confidence: Option<u8>,
    /// The source and version that wrote it, for example `vad@silero-5`.
    pub by: String,
    /// Attributes in canonical order, each at most once.
    pub attrs: Vec<(Attr, AttrValue)>,
}

impl Element {
    /// A point element with no attributes.
    pub fn at(kind: ElementKind, t: Micros, by: impl Into<String>) -> Self {
        Self {
            kind,
            t,
            to: None,
            confidence: None,
            by: by.into(),
            attrs: Vec::new(),
        }
    }

    /// A span element with no attributes.
    pub fn over(kind: ElementKind, range: TimeRange, by: impl Into<String>) -> Self {
        Self {
            to: Some(range.to()),
            ..Self::at(kind, range.from(), by)
        }
    }

    pub fn with_confidence(mut self, confidence: u8) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn with_text(self, attr: Attr, text: impl Into<String>) -> Self {
        self.with_attr(attr, AttrValue::Text(text.into()))
    }

    pub fn with_integer(self, attr: Attr, n: i64) -> Self {
        self.with_attr(attr, AttrValue::Integer(n))
    }

    fn with_attr(mut self, attr: Attr, value: AttrValue) -> Self {
        self.attrs.retain(|(a, _)| *a != attr);
        self.attrs.push((attr, value));
        self.attrs.sort_by_key(|(a, _)| *a);
        self
    }

    /// The span, or `t..t+1` for a point so every element has a non-empty range.
    pub fn range(&self) -> TimeRange {
        let to = self.to.unwrap_or(Micros(self.t.0 + 1));
        TimeRange::new(self.t, to).expect("elements are checked on construction")
    }

    pub fn attr(&self, attr: Attr) -> Option<&AttrValue> {
        self.attrs.iter().find(|(a, _)| *a == attr).map(|(_, v)| v)
    }

    pub fn text(&self, attr: Attr) -> Option<&str> {
        match self.attr(attr)? {
            AttrValue::Text(s) => Some(s),
            AttrValue::Integer(_) => None,
        }
    }

    pub fn integer(&self, attr: Attr) -> Option<i64> {
        match self.attr(attr)? {
            AttrValue::Integer(n) => Some(*n),
            AttrValue::Text(_) => None,
        }
    }

    /// The `x`, `y` point of a click, face or OCR box, in screen pixels.
    pub fn point(&self) -> Option<Point> {
        let x = i32::try_from(self.integer(Attr::X)?).ok()?;
        let y = i32::try_from(self.integer(Attr::Y)?).ok()?;
        Some(Point { x, y })
    }

    /// Checks the shape rules: `to` matches the kind, confidence is a percent, attributes fit.
    fn check(&self) -> Result<(), String> {
        let kind = self.kind;
        match (kind.is_point(), self.to) {
            (true, Some(_)) => return Err(format!("`{kind}` is a moment and takes no `to`")),
            (false, None) => return Err(format!("`{kind}` spans time and needs `to`")),
            (false, Some(to)) if to <= self.t => return Err("`to` must be after `t`".to_owned()),
            _ => {}
        }
        if self.confidence.is_some_and(|c| c > 100) {
            return Err("`confidence` is a percent from 0 to 100".to_owned());
        }
        if self.by.is_empty() {
            return Err("`by` names the source".to_owned());
        }
        let (required, optional) = kind.attrs();
        for attr in required {
            if self.attr(*attr).is_none() {
                return Err(format!("`{kind}` needs `{attr}`"));
            }
        }
        for (attr, value) in &self.attrs {
            if !required.contains(attr) && !optional.contains(attr) {
                return Err(format!("`{kind}` has no attribute `{attr}`"));
            }
            match (attr.is_integer(), value) {
                (true, AttrValue::Text(_)) => return Err(format!("`{attr}` is an integer")),
                (false, AttrValue::Integer(_)) => return Err(format!("`{attr}` is text")),
                _ => {}
            }
        }
        Ok(())
    }

    fn from_json(line: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let Value::Object(map) = value else {
            return Err("expected a JSON object".to_owned());
        };
        let mut element = Self {
            kind: ElementKind::Word,
            t: Micros(0),
            to: None,
            confidence: None,
            by: String::new(),
            attrs: Vec::new(),
        };
        let mut kind = None;
        for (key, value) in &map {
            match key.as_str() {
                "kind" => {
                    let text = value.as_str().ok_or("`kind` is a string")?;
                    kind = Some(ElementKind::parse(text).ok_or_else(|| {
                        format!(
                            "unknown kind `{text}`, expected one of: {}",
                            ElementKind::ALL
                                .iter()
                                .map(|k| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?);
                }
                "t" => element.t = Micros(as_u64(value, "t")?),
                "to" => element.to = Some(Micros(as_u64(value, "to")?)),
                "confidence" => {
                    let n = as_u64(value, "confidence")?;
                    element.confidence = Some(
                        u8::try_from(n)
                            .map_err(|_| "`confidence` is a percent from 0 to 100".to_owned())?,
                    );
                }
                "by" => element.by = value.as_str().ok_or("`by` is a string")?.to_owned(),
                other => {
                    let attr =
                        Attr::parse(other).ok_or_else(|| format!("unknown field `{other}`"))?;
                    let value = if attr.is_integer() {
                        AttrValue::Integer(
                            value
                                .as_i64()
                                .ok_or_else(|| format!("`{attr}` is an integer"))?,
                        )
                    } else {
                        AttrValue::Text(
                            value
                                .as_str()
                                .ok_or_else(|| format!("`{attr}` is text"))?
                                .to_owned(),
                        )
                    };
                    element.attrs.push((attr, value));
                }
            }
        }
        element.kind = kind.ok_or("missing `kind`")?;
        if !map.contains_key("t") {
            return Err("missing `t`".to_owned());
        }
        if element.by.is_empty() {
            return Err("missing `by`".to_owned());
        }
        element.attrs.sort_by_key(|(a, _)| *a);
        element.check()?;
        Ok(element)
    }

    /// Keys in a fixed order (`kind`, `t`, `to`, `confidence`, `by`, then attributes), which
    /// `serde_json`'s sorted map would not keep.
    fn to_json(&self) -> String {
        let mut fields: Vec<(&str, Value)> =
            vec![("kind", self.kind.as_str().into()), ("t", self.t.0.into())];
        if let Some(to) = self.to {
            fields.push(("to", to.0.into()));
        }
        if let Some(confidence) = self.confidence {
            fields.push(("confidence", confidence.into()));
        }
        fields.push(("by", self.by.as_str().into()));
        for (attr, value) in &self.attrs {
            let value = match value {
                AttrValue::Text(s) => Value::from(s.as_str()),
                AttrValue::Integer(n) => Value::from(*n),
            };
            fields.push((attr.as_str(), value));
        }
        let mut out = String::from("{");
        for (i, (key, value)) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&Value::from(*key).to_string());
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push('}');
        out
    }
}

fn as_u64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| format!("`{key}` is a non-negative integer"))
}

/// The elements of one take, sorted by `t` then kind, each identity (kind, `t`) unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elements {
    take: Id,
    items: Vec<Element>,
}

/// Returned by [`Elements::push`] when an element of that kind already starts at that time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateElement(pub Element);

impl Elements {
    pub fn new(take: Id) -> Self {
        Self {
            take,
            items: Vec::new(),
        }
    }

    pub fn take(&self) -> &Id {
        &self.take
    }

    pub fn iter(&self) -> impl Iterator<Item = &Element> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Inserts `element` at its sorted position. Fails when its identity is taken; the element
    /// is handed back in the error.
    pub fn push(&mut self, element: Element) -> Result<(), DuplicateElement> {
        let key = |e: &Element| (e.t, e.kind);
        let index = self.items.partition_point(|e| key(e) < key(&element));
        if self
            .items
            .get(index)
            .is_some_and(|e| key(e) == key(&element))
        {
            return Err(DuplicateElement(element));
        }
        self.items.insert(index, element);
        Ok(())
    }

    /// Elements of `kind`, sorted by `t`.
    pub fn of_kind(&self, kind: ElementKind) -> impl Iterator<Item = &Element> {
        self.items.iter().filter(move |e| e.kind == kind)
    }

    /// The element of `kind` whose `t` is nearest to `at`, within `tolerance`. An exact match
    /// wins; otherwise the earlier of two equally near elements.
    pub fn nearest(&self, kind: ElementKind, at: Micros, tolerance: Micros) -> Option<&Element> {
        self.of_kind(kind)
            .map(|e| (e.t.0.abs_diff(at.0), e))
            .filter(|(distance, _)| *distance <= tolerance.0)
            .min_by_key(|(distance, e)| (*distance, e.t))
            .map(|(_, e)| e)
    }

    /// Parses `elements/<take>.jsonl`. Errors name the line.
    pub fn parse(take: Id, text: &str) -> Result<Self, ParseError> {
        let mut elements = Self::new(take);
        let mut last = None;
        for (index, line) in text.lines().enumerate() {
            let at = |message: String| ParseError {
                file: "elements.jsonl",
                line: index + 1,
                column: 1,
                message,
            };
            if line.trim().is_empty() {
                return Err(at("blank line".to_owned()));
            }
            let element = Element::from_json(line).map_err(at)?;
            if last.is_some_and(|t| element.t < t) {
                return Err(at("elements must be sorted by `t`".to_owned()));
            }
            last = Some(element.t);
            elements.push(element).map_err(|DuplicateElement(e)| {
                at(format!("duplicate `{}` at {}", e.kind, e.t.0))
            })?;
        }
        Ok(elements)
    }

    /// The canonical file: one element per line, keys in a fixed order, a trailing newline.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for element in &self.items {
            out.push_str(&element.to_json());
            out.push('\n');
        }
        out
    }
}
