//! The theme window's view of a channel's brand files ([0029]): the core's [`shot_core::Themes`]
//! with the repo it was read from. The controls edit one theme's look together with the
//! channel's sounds and bookends, so [`Themes::look`] shows both and [`Themes::with_look`] puts
//! each field back in its file. Every operation is the core's; [`Themes::write`] writes what
//! differs through [`shot_mcp::brand`], holding the channel's lock, so a switch is one line of
//! `brand/brand.kdl` and an undo step is the change it undoes.
//!
//! [0029]: ../../../../docs/decisions/0029-several-themes-per-channel.md

use std::path::{Path, PathBuf};

use shot_core::{Brand, DEFAULT_THEME, Id};

/// The brand files of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Themes {
    pub repo: PathBuf,
    pub files: shot_core::Themes,
}

/// An id from a name a person typed: lowercase letters, digits and single dashes.
pub fn slug(name: &str) -> Option<Id> {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out
        .trim_end_matches('-')
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    Id::new(out)
}

/// How a theme's id reads in the UI: `cantina-light` as "Cantina light".
pub fn display(id: &Id) -> String {
    let text = id.as_str().replace('-', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Themes {
    /// Reads the channel at `repo`, migrating brand files written before themes were files.
    pub fn open(repo: &Path) -> Result<Self, String> {
        shot_mcp::brand::migrate(repo)?;
        Ok(Self {
            repo: repo.to_owned(),
            files: shot_mcp::brand::read(repo)?,
        })
    }

    /// `brand/brand.kdl`.
    pub fn brand(&self) -> &Brand {
        &self.files.brand
    }

    /// The theme the channel uses, `default` before it names one.
    pub fn active(&self) -> Id {
        self.files
            .active()
            .cloned()
            .unwrap_or_else(|| Id::new(DEFAULT_THEME).expect("an id"))
    }

    /// Every theme, sorted, the active one among them.
    pub fn names(&self) -> Vec<Id> {
        let mut names = self.files.names();
        names.push(self.active());
        names.sort();
        names.dedup();
        names
    }

    /// Theme `name`'s look with the channel's parts, as the controls show it.
    pub fn look(&self, name: &Id) -> Option<Brand> {
        let look = match self.files.look(name) {
            Some(look) => look.clone(),
            None if *name == self.active() => Brand::default(),
            None => return None,
        };
        Some(self.files.brand.with_look(&look))
    }

    fn with(
        &self,
        files: Result<shot_core::Themes, shot_core::ThemeError>,
    ) -> Result<Self, String> {
        Ok(Self {
            repo: self.repo.clone(),
            files: files.map_err(|e| e.to_string())?,
        })
    }

    /// With theme `name` showing the look fields of `view` and the channel showing its sounds,
    /// bookends and other channel parts.
    pub fn with_look(&self, name: &Id, view: &Brand) -> Result<Self, String> {
        let files = self.files.named().with_look(name, view).map(|files| {
            files.with_channel(|brand| {
                *brand = Brand {
                    theme: brand.theme.clone(),
                    ..view.channel()
                }
            })
        });
        self.with(files)
    }

    /// With the channel's parts of the brand changed by `change`: overlay presets.
    pub fn with_channel(&self, change: impl FnOnce(&mut Brand)) -> Self {
        Self {
            repo: self.repo.clone(),
            files: self.files.with_channel(change),
        }
    }

    /// `base`, or `base-2`, `base-3`, ... whichever no theme has.
    pub fn unique(&self, base: &Id) -> Id {
        self.files.named().unique(base)
    }

    /// With a new theme `name` showing the look fields of `look`, not active.
    pub fn create(&self, name: &Id, look: &Brand) -> Result<Self, String> {
        self.with(self.files.named().create(name, look))
    }

    /// With theme `from` renamed to `to`. A theme a format layer names is not renamed.
    pub fn rename(&self, from: &Id, to: &Id) -> Result<Self, String> {
        shot_mcp::brand::refuse_used(&self.repo, from, "rename")?;
        self.with(self.files.named().rename(from, to))
    }

    /// Without theme `name`. The active theme and one a format layer names cannot be deleted.
    pub fn delete(&self, name: &Id) -> Result<Self, String> {
        shot_mcp::brand::refuse_used(&self.repo, name, "delete")?;
        self.with(self.files.named().delete(name))
    }

    /// With theme `name` active: the one `theme` line of the brand file changes.
    pub fn activate(&self, name: &Id) -> Result<Self, String> {
        self.with(self.files.named().activate(name))
    }

    /// Writes what differs from `before`.
    pub fn write(&self, before: &Self) -> Result<(), String> {
        shot_mcp::brand::write(&self.repo, &before.files, &self.files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn id(s: &str) -> Id {
        Id::new(s).unwrap()
    }

    #[test]
    fn the_window_edits_a_look_and_the_channels_sounds_in_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("brand")).unwrap();
        fs::write(
            dir.path().join("brand/brand.kdl"),
            "colors accent=\"#c8f25a\"\non punchline off\n",
        )
        .unwrap();
        let themes = Themes::open(dir.path()).unwrap();
        assert_eq!(themes.names(), [id(DEFAULT_THEME)]);
        let mut view = themes.look(&id(DEFAULT_THEME)).unwrap();
        view.logo = Some("brand/logo.svg".into());
        view.sounds = vec![(
            shot_core::SoundKey {
                effect: shot_core::SoundEffect::Zoom,
                phase: None,
            },
            id("whoosh"),
        )];
        let edited = themes.with_look(&id(DEFAULT_THEME), &view).unwrap();
        edited.write(&themes).unwrap();
        let read = |p: &str| fs::read_to_string(dir.path().join(p)).unwrap();
        assert_eq!(
            read("brand/brand.kdl"),
            "theme default\nsounds zoom=whoosh\non punchline off\n"
        );
        assert_eq!(
            read("brand/themes/default.kdl"),
            "colors accent=\"#c8f25a\"\nlogo \"brand/logo.svg\"\n"
        );
        assert_eq!(Themes::open(dir.path()).unwrap(), edited);
    }

    #[test]
    fn names_become_ids() {
        assert_eq!(slug("Cantina Light!"), Some(id("cantina-light")));
        assert_eq!(slug("  2024 launch "), Some(id("launch")));
        assert_eq!(slug("!!"), None);
        assert_eq!(display(&id("cantina-light")), "Cantina light");
    }
}
