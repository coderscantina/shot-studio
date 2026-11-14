//! A channel's brand files as one value ([0029]): `brand/brand.kdl` with the channel's parts and
//! `theme <id>`, and every look in `brand/themes/<id>.kdl`. Every operation is a pure method that
//! returns the next value; [`Themes::changes`] lists the files that differ from the value before,
//! for the caller to write. The app, `shot theme` and the MCP server all go through these.
//!
//! [0029]: ../../../docs/decisions/0029-several-themes-per-channel.md

use std::collections::BTreeMap;
use std::fmt;

use crate::{Brand, CustomLayout, FormatLayer, Id, LayoutName, OverlayPreset};

/// The channel's brand file, relative to the repo root.
pub const BRAND_FILE: &str = "brand/brand.kdl";

/// The theme migration names the look a brand kept inline.
pub const DEFAULT_THEME: &str = "default";

/// `brand/themes/<id>.kdl`.
pub fn theme_file(id: &Id) -> String {
    format!("brand/themes/{id}.kdl")
}

/// The theme a video reads: the nearest layer that names one, its own `format.kdl`, then the
/// channel's `formats/<format>.kdl`, then the brand. `None` reads the look the brand holds.
pub fn picked_theme<'a>(
    brand: &'a Brand,
    channel: &'a FormatLayer,
    video: &'a FormatLayer,
) -> Option<&'a Id> {
    video
        .theme
        .as_ref()
        .or(channel.theme.as_ref())
        .or(brand.theme.as_ref())
}

/// Why a theme, preset or layout operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// A theme of that name exists already.
    Exists(Id),
    /// No theme of that name.
    Unknown(Id),
    /// The active theme cannot be deleted.
    Active(Id),
    /// No overlay preset of that name.
    UnknownPreset(Id),
    /// No channel layout of that name.
    UnknownLayout(LayoutName),
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists(id) => write!(f, "there is a theme `{id}` already"),
            Self::Unknown(id) => write!(f, "there is no theme `{id}` in brand/themes/"),
            Self::Active(id) => write!(
                f,
                "`{id}` is the channel's theme; use another one before deleting it"
            ),
            Self::UnknownPreset(id) => write!(f, "the brand has no overlay preset `{id}`"),
            Self::UnknownLayout(name) => write!(f, "the brand has no layout `{name}`"),
        }
    }
}

impl std::error::Error for ThemeError {}

/// One file a change writes or removes, by path relative to the repo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Write { path: String, text: String },
    Remove { path: String },
}

/// The brand file and the themes of one channel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Themes {
    /// `brand/brand.kdl`: the channel's parts and the theme it uses.
    pub brand: Brand,
    /// `brand/themes/<id>.kdl` by id, the active one among them.
    pub looks: BTreeMap<Id, Brand>,
}

impl Themes {
    /// The theme the channel uses.
    pub fn active(&self) -> Option<&Id> {
        self.brand.theme.as_ref()
    }

    /// Every theme, sorted.
    pub fn names(&self) -> Vec<Id> {
        self.looks.keys().cloned().collect()
    }

    pub fn look(&self, name: &Id) -> Option<&Brand> {
        self.looks.get(name)
    }

    fn known(&self, name: &Id) -> Result<(), ThemeError> {
        match self.looks.contains_key(name) {
            true => Ok(()),
            false => Err(ThemeError::Unknown(name.clone())),
        }
    }

    /// With theme `name` used by the channel: the one `theme` line of the brand file changes.
    pub fn activate(&self, name: &Id) -> Result<Self, ThemeError> {
        self.known(name)?;
        let mut next = self.clone();
        next.brand.theme = Some(name.clone());
        Ok(next)
    }

    /// With the theme the channel uses in a file: `default`, empty, named in the brand when it
    /// names none yet. What an edit of the channel's look starts from.
    pub fn named(&self) -> Self {
        let name = self
            .active()
            .cloned()
            .unwrap_or_else(|| Id::new(DEFAULT_THEME).expect("an id"));
        let mut next = self.clone();
        next.looks.entry(name.clone()).or_default();
        next.brand.theme = Some(name);
        next
    }

    /// With the look of the theme the channel uses changed by `change`, see [`Self::named`].
    pub fn with_active_look(&self, change: impl FnOnce(&mut Brand)) -> Self {
        let mut next = self.named();
        let name = next.active().cloned().expect("named");
        let look = next.looks.get_mut(&name).expect("named");
        change(look);
        *look = look.look();
        next
    }

    /// With theme `name` showing the look fields of `look`.
    pub fn with_look(&self, name: &Id, look: &Brand) -> Result<Self, ThemeError> {
        self.known(name)?;
        let mut next = self.clone();
        next.looks.insert(name.clone(), look.look());
        Ok(next)
    }

    /// With a new theme `name` of the look fields of `look`, not used yet.
    pub fn create(&self, name: &Id, look: &Brand) -> Result<Self, ThemeError> {
        if self.looks.contains_key(name) {
            return Err(ThemeError::Exists(name.clone()));
        }
        let mut next = self.clone();
        next.looks.insert(name.clone(), look.look());
        Ok(next)
    }

    /// With theme `from` renamed to `to`, the brand's `theme` line too when it names it.
    pub fn rename(&self, from: &Id, to: &Id) -> Result<Self, ThemeError> {
        self.known(from)?;
        if from == to {
            return Ok(self.clone());
        }
        if self.looks.contains_key(to) {
            return Err(ThemeError::Exists(to.clone()));
        }
        let mut next = self.clone();
        let look = next.looks.remove(from).expect("known");
        next.looks.insert(to.clone(), look);
        if self.active() == Some(from) {
            next.brand.theme = Some(to.clone());
        }
        Ok(next)
    }

    /// Without theme `name`. The active theme cannot be deleted.
    pub fn delete(&self, name: &Id) -> Result<Self, ThemeError> {
        self.known(name)?;
        if self.active() == Some(name) {
            return Err(ThemeError::Active(name.clone()));
        }
        let mut next = self.clone();
        next.looks.remove(name);
        Ok(next)
    }

    /// `base`, or `base-2`, `base-3`, ... whichever no theme has.
    pub fn unique(&self, base: &Id) -> Id {
        (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => Id::new(format!("{base}-{n}")).expect("an id with a number is an id"),
            })
            .find(|id| !self.looks.contains_key(id))
            .expect("some number is free")
    }

    /// With the channel's parts of the brand changed by `change`. Look fields it sets are
    /// dropped: they belong to a theme.
    pub fn with_channel(&self, change: impl FnOnce(&mut Brand)) -> Self {
        let mut next = self.clone();
        change(&mut next.brand);
        next.brand = next.brand.channel();
        next
    }

    /// With `preset` added, or replacing the preset `old` (a rename when the names differ)
    /// and any preset of its name.
    pub fn set_preset(&self, old: Option<&Id>, preset: OverlayPreset) -> Self {
        self.with_channel(|brand| {
            let presets = &mut brand.overlay_presets;
            presets.retain(|p| Some(&p.name) != old && p.name != preset.name);
            presets.push(preset);
            presets.sort_by(|a, b| a.name.cmp(&b.name));
        })
    }

    pub fn remove_preset(&self, name: &Id) -> Result<Self, ThemeError> {
        self.preset(name)?;
        Ok(self.with_channel(|brand| brand.overlay_presets.retain(|p| p.name != *name)))
    }

    pub fn preset(&self, name: &Id) -> Result<&OverlayPreset, ThemeError> {
        self.brand
            .overlay_presets
            .iter()
            .find(|p| p.name == *name)
            .ok_or_else(|| ThemeError::UnknownPreset(name.clone()))
    }

    /// With `layout` added to the channel, replacing the one of its name.
    pub fn set_layout(&self, layout: CustomLayout) -> Self {
        self.with_channel(|brand| brand.set_layout(layout))
    }

    pub fn remove_layout(&self, name: LayoutName) -> Result<Self, ThemeError> {
        self.brand
            .layout(name)
            .ok_or(ThemeError::UnknownLayout(name))?;
        Ok(self.with_channel(|brand| brand.layouts.retain(|l| l.name != name)))
    }

    /// The files that differ from `before`: the brand file, changed theme files, and the files
    /// of themes that are gone.
    pub fn changes(&self, before: &Self) -> Vec<FileChange> {
        let mut changes = Vec::new();
        if self.brand != before.brand {
            changes.push(FileChange::Write {
                path: BRAND_FILE.to_owned(),
                text: self.brand.to_kdl(),
            });
        }
        for (id, look) in &self.looks {
            if before.looks.get(id) != Some(look) {
                changes.push(FileChange::Write {
                    path: theme_file(id),
                    text: look.to_kdl(),
                });
            }
        }
        for id in before
            .looks
            .keys()
            .filter(|id| !self.looks.contains_key(*id))
        {
            changes.push(FileChange::Remove {
                path: theme_file(id),
            });
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::new(s).unwrap()
    }

    fn channel() -> Themes {
        let brand = Brand::parse(
            "theme dark\nsounds zoom=whoosh\noverlay-preset speaker template=lower-third length=5_000_000 {\n    text \"Mike\"\n}\n",
        )
        .unwrap();
        let looks = [
            ("dark", "colors accent=\"#c8f25a\"\n"),
            ("light", "colors accent=\"#3d7a00\"\n"),
        ]
        .map(|(name, text)| (id(name), Brand::parse_theme(text).unwrap()))
        .into();
        Themes { brand, looks }
    }

    #[test]
    fn a_switch_changes_one_line_of_the_brand_file() {
        let before = channel();
        let after = before.activate(&id("light")).unwrap();
        let changes = after.changes(&before);
        let [FileChange::Write { path, text }] = changes.as_slice() else {
            panic!("one file changes");
        };
        assert_eq!(path, BRAND_FILE);
        let old = before.brand.to_kdl();
        let changed: Vec<_> = old
            .lines()
            .zip(text.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(changed, [("theme dark", "theme light")]);
        assert_eq!(old.lines().count(), text.lines().count());
        assert_eq!(
            before.activate(&id("sepia")),
            Err(ThemeError::Unknown(id("sepia")))
        );
    }

    #[test]
    fn renaming_the_active_theme_renames_its_line_and_it_cannot_be_deleted() {
        let before = channel();
        let after = before.rename(&id("dark"), &id("night")).unwrap();
        assert_eq!(after.active(), Some(&id("night")));
        assert_eq!(
            after.changes(&before),
            [
                FileChange::Write {
                    path: BRAND_FILE.into(),
                    text: after.brand.to_kdl()
                },
                FileChange::Write {
                    path: "brand/themes/night.kdl".into(),
                    text: "colors accent=\"#c8f25a\"\n".into()
                },
                FileChange::Remove {
                    path: "brand/themes/dark.kdl".into()
                },
            ]
        );
        assert_eq!(
            after.delete(&id("night")),
            Err(ThemeError::Active(id("night")))
        );
        assert_eq!(
            before.rename(&id("dark"), &id("light")),
            Err(ThemeError::Exists(id("light")))
        );
    }
}
