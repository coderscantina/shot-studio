//! The brand files written before themes were files ([0029]): `brand/brand.kdl` with its look
//! inline, and theme files that held channel parts. Migration moves the inline look to the
//! theme the brand names, `default` without one, and keeps only look fields in theme files.
//!
//! [0029]: ../../../../docs/decisions/0029-several-themes-per-channel.md

use std::fmt;

use crate::{Brand, DEFAULT_THEME, Id, ParseError, Themes, theme_file};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrandMigrateError {
    /// A file does not parse: its path in the repo and the error.
    Parse(String, ParseError),
    /// `brand.kdl` keeps a look inline and the theme it names has a file with another look.
    Clash(Id),
}

impl fmt::Display for BrandMigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(path, e) => write!(f, "{path}:{e}"),
            Self::Clash(id) => write!(
                f,
                "brand/brand.kdl keeps a look inline and {} holds another one; move the look \
                 you want into {} and remove it from brand/brand.kdl",
                theme_file(id),
                theme_file(id)
            ),
        }
    }
}

impl std::error::Error for BrandMigrateError {}

/// The channel's brand files after migration, from `brand/brand.kdl` (`None` without one) and
/// the theme files by id. `None` when they are written the way they are read now.
pub fn migrate_brand(
    brand: Option<&str>,
    themes: &[(Id, &str)],
) -> Result<Option<Themes>, BrandMigrateError> {
    let parse = |path: &str, text: &str, theme: bool| {
        Brand::parse_old(text, theme).map_err(|e| BrandMigrateError::Parse(path.to_owned(), e))
    };
    let brand = match brand {
        Some(text) => parse(crate::BRAND_FILE, text, false)?,
        None => Brand::default(),
    };
    let mut migrated = false;
    let mut looks = std::collections::BTreeMap::new();
    for (id, text) in themes {
        let read = parse(&theme_file(id), text, true)?;
        migrated |= read.look() != read;
        looks.insert(id.clone(), read.look());
    }
    let mut next = brand.channel();
    if brand.has_look() {
        let name = brand
            .theme
            .clone()
            .unwrap_or_else(|| Id::new(DEFAULT_THEME).expect("an id"));
        match looks.get(&name) {
            Some(look) if *look != brand.look() => return Err(BrandMigrateError::Clash(name)),
            _ => {}
        }
        looks.insert(name.clone(), brand.look());
        next.theme = Some(name);
        migrated = true;
    }
    Ok(migrated.then_some(Themes { brand: next, looks }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inline_look_moves_to_the_default_theme() {
        let old = "colors accent=\"#c8f25a\"\nsounds zoom=whoosh\non punchline off\n";
        let themes = migrate_brand(Some(old), &[]).unwrap().expect("migrated");
        assert_eq!(
            themes.brand.to_kdl(),
            "theme default\nsounds zoom=whoosh\non punchline off\n"
        );
        let default = Id::new("default").unwrap();
        assert_eq!(
            themes.looks[&default].to_kdl(),
            "colors accent=\"#c8f25a\"\n"
        );
        let new = themes.brand.to_kdl();
        let files = [(default.clone(), "colors accent=\"#c8f25a\"\n")];
        assert_eq!(migrate_brand(Some(&new), &files), Ok(None));
        let clash = [(default.clone(), "colors accent=\"#000000\"\n")];
        assert_eq!(
            migrate_brand(Some(old), &clash),
            Err(BrandMigrateError::Clash(default))
        );
    }
}
