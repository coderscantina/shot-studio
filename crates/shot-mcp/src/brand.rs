//! A channel's brand files on disk: reads them into [`Themes`] and writes what an operation
//! changed, holding the channel's writer lock ([`shot_fs::WriteLock::channel`]). The app's theme
//! window, `shot theme` and the MCP server's theme tools all go through here, so they change
//! `brand/` the same way ([0029]).
//!
//! [0029]: ../../../docs/decisions/0029-several-themes-per-channel.md

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use shot_core::{
    BRAND_FILE, Brand, FileChange, FormatLayer, Id, ParseError, Themes, migrate_brand, theme_file,
};

/// `path:line:column: message`.
fn located(path: &str, e: &ParseError) -> String {
    format!("{path}:{}:{}: {}", e.line, e.column, e.message)
}

fn read_text(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The theme files of the channel at `repo` by id, as text.
fn theme_texts(repo: &Path) -> Result<Vec<(Id, String)>, String> {
    let dir = repo.join("brand/themes");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut texts = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let id = path
            .extension()
            .filter(|e| *e == "kdl")
            .and(path.file_stem())
            .and_then(|s| Id::new(s.to_str()?));
        if let Some(id) = id {
            let text = read_text(&path)?.unwrap_or_default();
            texts.push((id, text));
        }
    }
    Ok(texts)
}

/// Reads `brand/brand.kdl` and every `brand/themes/<id>.kdl` of the channel at `repo`. A
/// missing brand file is an empty brand.
pub fn read(repo: &Path) -> Result<Themes, String> {
    let brand = match read_text(&repo.join(BRAND_FILE))? {
        Some(text) => Brand::parse(&text).map_err(|e| located(BRAND_FILE, &e))?,
        None => Brand::default(),
    };
    let mut looks = std::collections::BTreeMap::new();
    for (id, text) in theme_texts(repo)? {
        let look = Brand::parse_theme(&text).map_err(|e| located(&theme_file(&id), &e))?;
        looks.insert(id, look);
    }
    Ok(Themes { brand, looks })
}

/// Writes the files `next` changed from `before`. The caller holds the channel's lock.
fn apply(repo: &Path, before: &Themes, next: &Themes) -> Result<(), String> {
    for change in next.changes(before) {
        match change {
            FileChange::Write { path, text } => {
                let path = repo.join(path);
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                shot_fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            FileChange::Remove { path } => match fs::remove_file(repo.join(&path)) {
                Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("{path}: {e}")),
                _ => {}
            },
        }
    }
    Ok(())
}

fn lock(repo: &Path) -> Result<shot_fs::WriteLock, String> {
    shot_fs::WriteLock::channel(repo).map_err(|e| format!("{}: {e}", repo.display()))
}

/// Writes `next` over the brand files `before` describes, holding the channel's lock: what the
/// app's theme window does with each change and each undo step.
pub fn write(repo: &Path, before: &Themes, next: &Themes) -> Result<(), String> {
    let _lock = lock(repo)?;
    apply(repo, before, next)
}

/// Reads the brand files, makes the next state with `change` and writes what differs, all
/// while holding the channel's lock, so two writers never lose each other's change.
pub fn change(
    repo: &Path,
    change: impl FnOnce(&Themes) -> Result<Themes, String>,
) -> Result<Themes, String> {
    let _lock = lock(repo)?;
    let before = read(repo)?;
    let next = change(&before)?;
    apply(repo, &before, &next)?;
    Ok(next)
}

/// Moves a look kept inline in `brand/brand.kdl` into its theme file and keeps only look fields
/// in theme files, once. Returns whether anything was written.
pub fn migrate(repo: &Path) -> Result<bool, String> {
    let _lock = lock(repo)?;
    let brand = read_text(&repo.join(BRAND_FILE))?;
    let texts = theme_texts(repo)?;
    let themes: Vec<(Id, &str)> = texts
        .iter()
        .map(|(id, t)| (id.clone(), t.as_str()))
        .collect();
    let Some(next) = migrate_brand(brand.as_deref(), &themes).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    // Every file is written, since the old ones do not parse in the new grammar.
    apply(repo, &Themes::default(), &next)?;
    Ok(true)
}

/// The format layers of the channel at `repo` that name theme `id`, by path.
fn users(repo: &Path, id: &Id) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for (dir, file) in [("formats", None), ("videos", Some("format.kdl"))] {
        let Ok(entries) = fs::read_dir(repo.join(dir)) else {
            continue;
        };
        let mut found: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                Some(match file {
                    Some(file) => format!("{dir}/{name}/{file}"),
                    None => name.ends_with(".kdl").then(|| format!("{dir}/{name}"))?,
                })
            })
            .collect();
        found.sort();
        for path in found {
            let Some(text) = read_text(&repo.join(&path))? else {
                continue;
            };
            // A layer that does not parse names no theme this check could see.
            if FormatLayer::parse(&text).is_ok_and(|l| l.theme.as_ref() == Some(id)) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

/// Refuses to `doing` (rename, delete) theme `id` while a format layer names it.
pub fn refuse_used(repo: &Path, id: &Id, doing: &str) -> Result<(), String> {
    match users(repo, id)?.as_slice() {
        [] => Ok(()),
        paths => Err(format!(
            "{} names theme `{id}`; change it before you {doing} the theme",
            paths.join(", ")
        )),
    }
}

/// Makes theme `id` the channel's: the one `theme` line of `brand/brand.kdl` changes.
pub fn activate(repo: &Path, id: &Id) -> Result<Themes, String> {
    change(repo, |t| t.activate(id).map_err(|e| e.to_string()))
}

/// Saves the look of theme `from`, the channel's theme by default, as a new theme `name`. A
/// channel without a theme saves an empty look.
pub fn save(repo: &Path, name: &Id, from: Option<&Id>) -> Result<Themes, String> {
    change(repo, |t| {
        let look = match from.or(t.active()) {
            Some(from) => t
                .look(from)
                .cloned()
                .ok_or_else(|| format!("there is no theme `{from}` in brand/themes/"))?,
            None => Brand::default(),
        };
        t.create(name, &look).map_err(|e| e.to_string())
    })
}

/// Renames theme `from` to `to`. A theme a format layer names is not renamed.
pub fn rename(repo: &Path, from: &Id, to: &Id) -> Result<Themes, String> {
    change(repo, |t| {
        refuse_used(repo, from, "rename")?;
        t.rename(from, to).map_err(|e| e.to_string())
    })
}

/// Deletes theme `id`. The channel's theme and a theme a format layer names are not deleted.
pub fn delete(repo: &Path, id: &Id) -> Result<Themes, String> {
    change(repo, |t| {
        refuse_used(repo, id, "delete")?;
        t.delete(id).map_err(|e| e.to_string())
    })
}

/// The channel's themes, one per line, the active one marked, then the overlay presets.
pub fn list_text(themes: &Themes) -> String {
    let mut out = String::new();
    for name in themes.names() {
        let mark = if themes.active() == Some(&name) {
            " (active)"
        } else {
            ""
        };
        out.push_str(&format!("{name}{mark}\n"));
    }
    if themes.looks.is_empty() {
        out.push_str("no themes: brand/themes/ is empty\n");
    }
    for preset in &themes.brand.overlay_presets {
        out.push_str(&format!(
            "overlay preset {}: {} for {:.1} s\n",
            preset.name,
            preset.template,
            preset.length.0 as f64 / 1e6
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn id(s: &str) -> Id {
        Id::new(s).unwrap()
    }

    fn channel() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("brand/themes")).unwrap();
        fs::write(dir.path().join(BRAND_FILE), "theme dark\n").unwrap();
        fs::write(
            dir.path().join("brand/themes/dark.kdl"),
            "colors accent=\"#c8f25a\"\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn concurrent_brand_writes_both_land() {
        let dir = channel();
        let writers: Vec<_> = ["light", "paper"]
            .into_iter()
            .map(|name| {
                let repo = dir.path().to_owned();
                thread::spawn(move || {
                    // Each save reads, waits a little inside the lock and writes.
                    change(&repo, |t| {
                        thread::sleep(std::time::Duration::from_millis(20));
                        let look = t.look(&id("dark")).unwrap().clone();
                        t.create(&id(name), &look).map_err(|e| e.to_string())
                    })
                    .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let themes = read(dir.path()).unwrap();
        assert_eq!(themes.names(), [id("dark"), id("light"), id("paper")]);
    }

    #[test]
    fn opening_an_old_brand_moves_its_look_and_a_used_theme_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("brand")).unwrap();
        fs::write(
            dir.path().join(BRAND_FILE),
            "colors accent=\"#c8f25a\"\nsounds zoom=whoosh\n",
        )
        .unwrap();
        assert!(migrate(dir.path()).unwrap());
        assert!(!migrate(dir.path()).unwrap(), "once");
        let read_file = |p: &str| fs::read_to_string(dir.path().join(p)).unwrap();
        assert_eq!(read_file(BRAND_FILE), "theme default\nsounds zoom=whoosh\n");
        assert_eq!(
            read_file("brand/themes/default.kdl"),
            "colors accent=\"#c8f25a\"\n"
        );

        save(dir.path(), &id("light"), None).unwrap();
        fs::create_dir_all(dir.path().join("formats")).unwrap();
        fs::write(dir.path().join("formats/tutorial.kdl"), "theme light\n").unwrap();
        let err = delete(dir.path(), &id("light")).unwrap_err();
        assert!(err.contains("formats/tutorial.kdl"), "{err}");
    }
}
