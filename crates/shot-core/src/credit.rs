//! `<asset>.credit.kdl`: where a downloaded sound or image comes from, its license and the
//! attribution the video description carries. The file sits beside the asset with the same
//! stem, so `brand/sounds/deep-whoosh.mp3` has `brand/sounds/deep-whoosh.credit.kdl`.

use crate::ParseError;
use crate::kdl_io::{Source, Writer, keyword_enum, required};

keyword_enum! {
    /// The licenses a downloaded asset may carry: those that allow commercial use and changes
    /// without binding the video to a license of its own.
    pub enum License {
        /// Creative Commons Zero, public domain dedication.
        Cc0 = "cc0",
        /// Public Domain Mark.
        Pdm = "pdm",
        /// Creative Commons Attribution.
        By = "by",
    }
}

impl License {
    /// The name people know it by, such as "CC BY".
    pub fn label(self) -> &'static str {
        match self {
            Self::Cc0 => "CC0",
            Self::Pdm => "Public domain",
            Self::By => "CC BY",
        }
    }

    /// Whether the video has to credit the creator.
    pub fn needs_attribution(self) -> bool {
        self == Self::By
    }
}

/// The credit of one downloaded asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    pub title: String,
    pub creator: Option<Creator>,
    pub license: License,
    /// The license version, such as "2.0".
    pub version: Option<String>,
    pub license_url: Option<String>,
    /// The page the asset was found on.
    pub source: String,
    /// Who hosts it, such as "freesound" or "flickr".
    pub provider: Option<String>,
    /// The URL the file was downloaded from.
    pub file: Option<String>,
    /// The Openverse id of the asset.
    pub openverse: Option<String>,
    pub tags: Vec<String>,
    /// The attribution line, as the provider words it.
    pub attribution: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Creator {
    pub name: String,
    pub url: Option<String>,
}

impl Credit {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "credit.kdl",
            text,
        };
        let doc = src.parse()?;
        let (mut title, mut creator, mut license, mut source, mut file) =
            (None, None, None, None, None);
        let (mut openverse, mut tags, mut attribution) = (None, None, None);
        for node in src.nodes(&doc) {
            match node.name() {
                "title" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut title, node.arg::<String>(0)?)?;
                }
                "creator" => {
                    node.expect(1, &["url"])?;
                    let value = Creator {
                        name: node.arg(0)?,
                        url: node.opt_prop("url")?,
                    };
                    node.set_once(&mut creator, value)?;
                }
                "license" => {
                    node.expect(1, &["version", "url"])?;
                    let value: (License, Option<String>, Option<String>) = (
                        node.arg(0)?,
                        node.opt_prop("version")?,
                        node.opt_prop("url")?,
                    );
                    node.set_once(&mut license, value)?;
                }
                "source" => {
                    node.expect(1, &["provider"])?;
                    let value: (String, Option<String>) =
                        (node.arg(0)?, node.opt_prop("provider")?);
                    node.set_once(&mut source, value)?;
                }
                "file" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut file, node.arg::<String>(0)?)?;
                }
                "openverse" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut openverse, node.arg::<String>(0)?)?;
                }
                "tags" => {
                    node.expect_list()?;
                    node.set_once(&mut tags, node.args::<String>()?)?;
                }
                "attribution" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut attribution, node.arg::<String>(0)?)?;
                }
                _ => return Err(node.unknown()),
            }
        }
        let missing = |message| src.error(0, message);
        let (license, version, license_url) = required(license, "license", missing)?;
        let (source, provider) = required(source, "source", missing)?;
        Ok(Self {
            title: required(title, "title", missing)?,
            creator,
            license,
            version,
            license_url,
            source,
            provider,
            file,
            openverse,
            tags: tags.unwrap_or_default(),
            attribution: required(attribution, "attribution", missing)?,
        })
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.node("title").arg(&self.title).end();
        if let Some(creator) = &self.creator {
            w.node("creator").arg(&creator.name);
            if let Some(url) = &creator.url {
                w.prop("url", url);
            }
            w.end();
        }
        w.node("license").arg(&self.license);
        if let Some(version) = &self.version {
            w.prop("version", version);
        }
        if let Some(url) = &self.license_url {
            w.prop("url", url);
        }
        w.end();
        w.node("source").arg(&self.source);
        if let Some(provider) = &self.provider {
            w.prop("provider", provider);
        }
        w.end();
        if let Some(file) = &self.file {
            w.node("file").arg(file).end();
        }
        if let Some(id) = &self.openverse {
            w.node("openverse").arg(id).end();
        }
        if !self.tags.is_empty() {
            w.node("tags");
            for tag in &self.tags {
                w.arg(tag);
            }
            w.end();
        }
        w.node("attribution").arg(&self.attribution).end();
        w.finish()
    }

    /// The license with its version, such as "CC BY 2.0".
    pub fn license_name(&self) -> String {
        match &self.version {
            Some(version) if self.license != License::Pdm => {
                format!("{} {version}", self.license.label())
            }
            _ => self.license.label().to_owned(),
        }
    }

    /// One line for the video description: the attribution and where the asset was found.
    pub fn line(&self) -> String {
        format!("{} Source: {}", self.attribution.trim(), self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str =
        include_str!("../../../fixtures/examples/channel/brand/sounds/deep-whoosh.credit.kdl");
    const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

    #[test]
    fn a_credit_parses_and_writes_back_byte_identical() {
        assert!(CONTRACT.contains(EXAMPLE), "the contract shows the example");
        let credit = Credit::parse(EXAMPLE).unwrap();
        assert_eq!(credit.license, License::Cc0);
        assert_eq!(credit.creator.as_ref().unwrap().name, "Kinoton");
        assert_eq!(credit.tags, ["sfx", "swoosh", "whoosh"]);
        assert_eq!(credit.license_name(), "CC0 1.0");
        assert_eq!(credit.to_kdl(), EXAMPLE);
    }

    #[test]
    fn a_minimal_credit_round_trips_and_strictness_holds() {
        let minimal = "title \"Facepalm\"\nlicense by\nsource \"https://flickr.com/p/1\"\nattribution \"Facepalm by someone\"\n";
        let credit = Credit::parse(minimal).unwrap();
        assert_eq!(credit.to_kdl(), minimal);
        assert!(credit.line().ends_with("Source: https://flickr.com/p/1"));
        let nc = minimal.replace("license by", "license by-nc");
        assert!(
            Credit::parse(&nc)
                .unwrap_err()
                .to_string()
                .contains("by-nc")
        );
        let unknown = format!("{minimal}price 3\n");
        assert_eq!(Credit::parse(&unknown).unwrap_err().line, 5);
        let missing = minimal.replace("attribution \"Facepalm by someone\"\n", "");
        assert!(
            Credit::parse(&missing)
                .unwrap_err()
                .message
                .contains("attribution")
        );
    }
}
