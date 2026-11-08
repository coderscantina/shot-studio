//! A channel's themes on disk ([0029]): `brand/brand.kdl` holds the active theme with the
//! channel's imports, rules and overlay presets, `brand/themes/<id>.kdl` every other theme's look.
//! Every change is a new [`Themes`] value made by a pure method; [`Themes::write`] writes the
//! files that differ from the value before, through `shot-core`'s canonical writer.
//!
//! [0029]: ../../../../docs/decisions/0029-several-themes-per-channel.md

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shot_core::{Brand, Id};

/// The name of the one theme of a brand without a `theme` node.
pub const DEFAULT: &str = "default";

/// The brand file and the saved themes of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Themes {
    pub repo: PathBuf,
    /// `brand/brand.kdl`.
    pub brand: Brand,
    /// `brand/themes/<id>.kdl` by id: the themes that are not active.
    pub saved: BTreeMap<Id, Brand>,
}

/// `brand` without what belongs to the channel rather than the theme: imports, rules, the theme
/// name and the overlay presets.
pub fn look_of(brand: &Brand) -> Brand {
    Brand {
        imports: Vec::new(),
        rules: Default::default(),
        theme: None,
        overlay_presets: Vec::new(),
        layouts: Vec::new(),
        ..brand.clone()
    }
}

/// `look` with the channel's parts of `brand`.
fn with_look(brand: &Brand, look: &Brand) -> Brand {
    Brand {
        imports: brand.imports.clone(),
        rules: brand.rules.clone(),
        theme: brand.theme.clone(),
        overlay_presets: brand.overlay_presets.clone(),
        layouts: brand.layouts.clone(),
        ..look_of(look)
    }
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

fn brand_path(repo: &Path) -> PathBuf {
    repo.join("brand/brand.kdl")
}

fn theme_path(repo: &Path, id: &Id) -> PathBuf {
    repo.join("brand/themes").join(format!("{id}.kdl"))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    shot_fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

impl Themes {
    /// Reads the channel at `repo`. A missing brand file is an empty brand.
    pub fn open(repo: &Path) -> Result<Self, String> {
        let path = brand_path(repo);
        let brand = match fs::read_to_string(&path) {
            Ok(text) => Brand::parse(&text).map_err(|e| format!("brand/brand.kdl:{e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Brand::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut saved = BTreeMap::new();
        if let Ok(entries) = fs::read_dir(repo.join("brand/themes")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(id) = path
                    .extension()
                    .filter(|e| *e == "kdl")
                    .and(path.file_stem())
                    .and_then(|s| Id::new(s.to_str()?))
                else {
                    continue;
                };
                let text =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let look =
                    Brand::parse_theme(&text).map_err(|e| format!("brand/themes/{id}.kdl:{e}"))?;
                saved.insert(id, look);
            }
        }
        Ok(Self {
            repo: repo.to_owned(),
            brand,
            saved,
        })
    }

    /// The active theme's id.
    pub fn active(&self) -> Id {
        self.brand
            .theme
            .clone()
            .unwrap_or_else(|| Id::new(DEFAULT).expect("an id"))
    }

    /// Every theme, sorted, the active one among them.
    pub fn names(&self) -> Vec<Id> {
        let mut names: Vec<Id> = self.saved.keys().cloned().collect();
        names.push(self.active());
        names.sort();
        names.dedup();
        names
    }

    /// The look of theme `name`.
    pub fn look(&self, name: &Id) -> Option<Brand> {
        if *name == self.active() {
            return Some(look_of(&self.brand));
        }
        self.saved.get(name).cloned()
    }

    /// With theme `name` showing `look`.
    pub fn with_look(&self, name: &Id, look: &Brand) -> Self {
        let mut next = self.clone();
        if *name == self.active() {
            next.brand = with_look(&self.brand, look);
        } else {
            next.saved.insert(name.clone(), look_of(look));
        }
        next
    }

    /// With the channel's parts of the brand changed by `change`: overlay presets.
    pub fn with_channel(&self, change: impl FnOnce(&mut Brand)) -> Self {
        let mut next = self.clone();
        change(&mut next.brand);
        next
    }

    /// `base`, or `base-2`, `base-3`, ... whichever no theme has.
    pub fn unique(&self, base: &Id) -> Id {
        let names = self.names();
        (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => Id::new(format!("{base}-{n}")).expect("an id with a number is an id"),
            })
            .find(|id| !names.contains(id))
            .expect("some number is free")
    }

    /// With a new theme `name` showing `look`, not active.
    pub fn create(&self, name: &Id, look: &Brand) -> Result<Self, String> {
        if self.names().contains(name) {
            return Err(format!("There is a theme called {} already", display(name)));
        }
        let mut next = self.named();
        next.saved.insert(name.clone(), look_of(look));
        Ok(next)
    }

    /// With theme `from` renamed to `to`.
    pub fn rename(&self, from: &Id, to: &Id) -> Result<Self, String> {
        if from == to {
            return Ok(self.clone());
        }
        if self.names().contains(to) {
            return Err(format!("There is a theme called {} already", display(to)));
        }
        let mut next = self.clone();
        if *from == self.active() {
            next.brand.theme = Some(to.clone());
        } else {
            let look = next.saved.remove(from).ok_or("That theme is gone")?;
            next.saved.insert(to.clone(), look);
        }
        Ok(next)
    }

    /// Without theme `name`. The active theme cannot be deleted.
    pub fn delete(&self, name: &Id) -> Result<Self, String> {
        if *name == self.active() {
            return Err("Make another theme active before deleting this one".into());
        }
        let mut next = self.clone();
        next.saved.remove(name);
        Ok(next)
    }

    /// With theme `name` active: the active look moves to its own file and `name`'s look into
    /// the brand file.
    pub fn activate(&self, name: &Id) -> Result<Self, String> {
        let active = self.active();
        if *name == active {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        let look = next.saved.remove(name).ok_or("That theme is gone")?;
        next.saved.insert(active, look_of(&self.brand));
        next.brand = with_look(&self.brand, &look);
        next.brand.theme = Some(name.clone());
        Ok(next)
    }

    /// With the active theme's name written, as every theme besides the first needs.
    fn named(&self) -> Self {
        let mut next = self.clone();
        next.brand.theme = Some(self.active());
        next
    }

    /// Writes what differs from `before`: the brand file, changed theme files, and removes
    /// the files of themes that are gone.
    pub fn write(&self, before: &Self) -> Result<(), String> {
        if self.brand != before.brand {
            write_file(&brand_path(&self.repo), &self.brand.to_kdl())?;
        }
        for (id, look) in &self.saved {
            if before.saved.get(id) != Some(look) {
                write_file(&theme_path(&self.repo, id), &look.to_kdl())?;
            }
        }
        for id in before
            .saved
            .keys()
            .filter(|id| !self.saved.contains_key(*id))
        {
            let path = theme_path(&self.repo, id);
            match fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: {e}", path.display()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::new(s).unwrap()
    }

    const BRAND: &str = "import \"https://example.com/styles\" rev=\"abc\" style=base\ncolors accent=\"#c8f25a\"\nlayout big-face {\n    camera x=0 y=0 width=3840 height=2160\n}\noverlay-preset speaker template=lower-third length=5_000_000 {\n    text \"Mike\"\n}\non punchline off\n";

    fn channel() -> (tempfile::TempDir, Themes) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("brand")).unwrap();
        fs::write(dir.path().join("brand/brand.kdl"), BRAND).unwrap();
        let themes = Themes::open(dir.path()).unwrap();
        (dir, themes)
    }

    fn read(dir: &Path, path: &str) -> Option<String> {
        fs::read_to_string(dir.join(path)).ok()
    }

    #[test]
    fn a_brand_without_a_theme_is_the_default_theme() {
        let (_dir, themes) = channel();
        assert_eq!(themes.names(), vec![id(DEFAULT)]);
        let look = themes.look(&id(DEFAULT)).unwrap();
        assert!(look.imports.is_empty() && look.overlay_presets.is_empty());
        assert_eq!(look.colors.accent, themes.brand.colors.accent);
    }

    #[test]
    fn switching_moves_the_looks_between_the_files_and_keeps_the_channel() {
        let (dir, themes) = channel();
        let mut light = Brand::default();
        light.colors.accent = Some(shot_core::Color([0x3d, 0x7a, 0x00]));
        let created = themes.create(&id("light"), &light).unwrap();
        created.write(&themes).unwrap();
        assert_eq!(
            read(dir.path(), "brand/themes/light.kdl").unwrap(),
            "colors accent=\"#3d7a00\"\n"
        );
        assert!(
            read(dir.path(), "brand/brand.kdl")
                .unwrap()
                .contains("theme default\n")
        );

        let switched = created.activate(&id("light")).unwrap();
        switched.write(&created).unwrap();
        let brand = read(dir.path(), "brand/brand.kdl").unwrap();
        assert!(brand.contains("theme light\n") && brand.contains("accent=\"#3d7a00\""));
        assert!(brand.contains("import ") && brand.contains("overlay-preset speaker"));
        assert!(
            brand.contains("layout big-face"),
            "channel layouts stay with the channel"
        );
        assert!(brand.contains("on punchline off"));
        assert_eq!(read(dir.path(), "brand/themes/light.kdl"), None);
        assert_eq!(
            read(dir.path(), "brand/themes/default.kdl").unwrap(),
            "colors accent=\"#c8f25a\"\n"
        );
        assert_eq!(Themes::open(dir.path()).unwrap(), switched);
    }

    #[test]
    fn edits_go_to_the_theme_they_change() {
        let (dir, themes) = channel();
        let themes = themes.create(&id("light"), &Brand::default()).unwrap();
        themes.write(&channel().1).unwrap();
        let mut look = themes.look(&id("light")).unwrap();
        look.logo = Some("brand/logo.svg".into());
        let edited = themes.with_look(&id("light"), &look);
        assert_eq!(edited.brand, themes.brand, "the active theme stays");
        edited.write(&themes).unwrap();
        assert_eq!(
            read(dir.path(), "brand/themes/light.kdl").unwrap(),
            "logo \"brand/logo.svg\"\n"
        );

        let renamed = edited.rename(&id("light"), &id("paper")).unwrap();
        renamed.write(&edited).unwrap();
        assert_eq!(read(dir.path(), "brand/themes/light.kdl"), None);
        assert!(read(dir.path(), "brand/themes/paper.kdl").is_some());
        assert!(renamed.delete(&id("default")).is_err());
        let deleted = renamed.delete(&id("paper")).unwrap();
        deleted.write(&renamed).unwrap();
        assert_eq!(read(dir.path(), "brand/themes/paper.kdl"), None);
    }

    #[test]
    fn names_become_ids() {
        assert_eq!(slug("Cantina Light!"), Some(id("cantina-light")));
        assert_eq!(slug("  2024 launch "), Some(id("launch")));
        assert_eq!(slug("!!"), None);
        assert_eq!(display(&id("cantina-light")), "Cantina light");
        let (_dir, themes) = channel();
        assert_eq!(themes.unique(&id(DEFAULT)), id("default-2"));
    }
}
