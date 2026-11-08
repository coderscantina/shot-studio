//! One-off effects a person places: a sound as an `sfx` override on a word, a callout or a pop
//! as an overlay in the take block, and a Lottie file added to the channel's brand. Pure
//! functions over `shot-core` types, except [`add_lottie`], which writes the channel repo.

use std::fs;
use std::path::Path;

use shot_core::{
    Brand, Cut, EditError, Effect, ElementKind, Elements, Id, Layer, LottieTemplate, Micros,
    Overlay, OverrideEffect, ParamValue, Point, Provenance, Slot, SlotType, TakeAt, TimeRange,
};
use shot_render::lottie::{self, Finding, SlotKind};
use shot_render::overlay::{self, callout};

/// How long a placed callout shows.
pub const CALLOUT_LENGTH: Micros = Micros(3_000_000);
/// How long a placed pop shows, as long as the brand's example rule.
pub const POP_LENGTH: Micros = Micros(1_200_000);
/// How far from the moment a sound is placed at its word may start.
const WORD_REACH: Micros = Micros(1_000_000);

/// Where a sound name resolves, in lookup order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundOrigin {
    /// The video's `sounds/`.
    Video,
    /// The channel's `brand/sounds/`.
    Brand,
    Bundled,
}

impl SoundOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "This video",
            Self::Brand => "Brand",
            Self::Bundled => "Bundled",
        }
    }
}

/// The sounds a video can place, in lookup order: its `sounds/`, the channel's
/// `brand/sounds/`, then the bundled effects, each folder sorted by name. A name found earlier
/// hides the same name later, as the lookup does.
pub fn sounds(video_dir: &Path) -> Vec<(Id, SoundOrigin)> {
    let mut found: Vec<(Id, SoundOrigin)> = Vec::new();
    let brand = video_dir
        .parent()
        .and_then(Path::parent)
        .map(|r| r.join("brand"));
    let folders = [Some(video_dir.to_owned()), brand];
    for (dir, origin) in folders.iter().zip([SoundOrigin::Video, SoundOrigin::Brand]) {
        let Some(Ok(entries)) = dir.as_ref().map(|d| fs::read_dir(d.join("sounds"))) else {
            continue;
        };
        let mut names: Vec<Id> = entries
            .filter_map(|e| {
                let path = e.ok()?.path();
                let ext = path.extension()?.to_str()?;
                shot_audio::sound::EXTENSIONS
                    .contains(&ext)
                    .then(|| Id::new(path.file_stem()?.to_str()?))
                    .flatten()
            })
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            if !found.iter().any(|(n, _)| *n == name) {
                found.push((name, origin));
            }
        }
    }
    for bundled in shot_audio::sound::BUNDLED {
        let name = Id::new(bundled.name).expect("bundled names are ids");
        if bundled.kind == shot_audio::sound::Kind::Sfx && !found.iter().any(|(n, _)| *n == name) {
            found.push((name, SoundOrigin::Bundled));
        }
    }
    found
}

/// `cut` with `sound` on the word of `take` that is spoken at `t`, or starts nearest it within
/// a second: `override <take> word at=<its t> { sfx <sound> }`, replacing that word's sound and
/// keeping its other changes. The sound plays from the word's start.
pub fn with_sound(
    cut: &Cut,
    take: &Id,
    elements: Option<&Elements>,
    t: Micros,
    sound: Id,
) -> Result<Cut, String> {
    let words = || {
        elements
            .into_iter()
            .flat_map(|e| e.of_kind(ElementKind::Word))
    };
    let word = words()
        .find(|w| w.range().from() <= t && t < w.range().to())
        .or_else(|| {
            words()
                .filter(|w| w.t.0.abs_diff(t.0) <= WORD_REACH.0)
                .min_by_key(|w| w.t.0.abs_diff(t.0))
        })
        .ok_or("No word is spoken here. Sounds sit on a word, so pick one in the transcript")?;
    Ok(crate::style::with_override(
        cut,
        take,
        ElementKind::Word,
        word.t,
        |o| {
            o.effects
                .retain(|e| !matches!(e, OverrideEffect::Set(Effect::Sfx { .. })));
            o.effects
                .push(OverrideEffect::Set(Effect::Sfx { sound, gain: 0 }));
            o.effects.sort_by_key(OverrideEffect::key);
        },
    ))
}

/// `cut` with `overlay` drawn on top of the overlays of the block at `at`.
pub fn with_overlay(cut: &Cut, at: TakeAt, overlay: Overlay) -> Result<Cut, EditError> {
    let mut cut = cut.clone();
    cut.chapters
        .get_mut(at.chapter)
        .and_then(|c| c.takes.get_mut(at.take))
        .ok_or(EditError::UnknownTake(at))?
        .overlays
        .push(overlay);
    Ok(cut)
}

fn param(name: &str, value: ParamValue) -> (Id, ParamValue) {
    (
        Id::new(name).expect("template parameter names are ids"),
        value,
    )
}

fn shown_from(t: Micros, length: Micros) -> TimeRange {
    TimeRange::new(t, Micros(t.0 + length.0)).expect("a length is positive")
}

/// A callout drawn at screen pixel `at` at take time `t`, snapped with [`callout::snap`] to the
/// OCR box under it or the nearest click, else on the point as drawn.
pub fn callout(elements: Option<&Elements>, t: Micros, at: Point) -> Overlay {
    let params = match elements.and_then(|e| callout::snap(e, t, at)) {
        Some(snap) => snap
            .params()
            .into_iter()
            .map(|(name, value)| param(name, value))
            .collect(),
        None => vec![
            param("x", ParamValue::Integer(at.x.into())),
            param("y", ParamValue::Integer(at.y.into())),
        ],
    };
    Overlay {
        template: Id::new(callout::TEMPLATE).expect("an id"),
        range: shown_from(t, CALLOUT_LENGTH),
        params,
    }
}

/// What a pop shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop {
    /// One of the bundled animated emoji, [`overlay::pop::EMOJI`].
    Emoji(&'static str),
    /// A Lottie template the brand declares.
    Sticker(Id),
}

/// A pop at take time `t` in its default place and size.
pub fn pop(t: Micros, pop: Pop) -> Overlay {
    let params = match pop {
        Pop::Emoji(emoji) => vec![param("text", ParamValue::Text(emoji.to_owned()))],
        Pop::Sticker(name) => vec![param("sticker", ParamValue::Text(name.to_string()))],
    };
    Overlay {
        template: Id::new(overlay::pop::TEMPLATE).expect("an id"),
        range: shown_from(t, POP_LENGTH),
        params,
    }
}

/// The brand's stickers: its Lottie templates under new names, which a pop can draw. A
/// re-skin of a built-in template is not a sticker.
pub fn stickers(brand: &Brand) -> impl Iterator<Item = &Id> {
    brand
        .templates
        .iter()
        .filter(|t| overlay::spec(t.name.as_str()).is_none())
        .map(|t| &t.name)
}

/// Where an instance of the lock came from, in a few words: the rule's selector and the file
/// it lives in, or the override.
pub fn provenance(p: &Provenance) -> String {
    let file = match p.layer {
        Layer::Import => "an imported style",
        Layer::Brand => "the brand",
        Layer::Format => "the channel's format",
        Layer::Video => "this video's format",
        Layer::Override => return "Override in cut.kdl".to_owned(),
    };
    match &p.rule {
        Some(selector) => format!("Rule on {selector} in {file}"),
        None => format!("From {file}"),
    }
}

/// What adding a Lottie file to the brand did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LottieImport {
    /// Copied to `file` in the channel repo and declared as template `name`.
    Added { name: Id, file: String },
    /// The import check found features velato cannot draw; nothing was written.
    Refused(Vec<Finding>),
}

/// Checks the Lottie at `source` with [`lottie::check`] and, when it renders, copies it to
/// `brand/<name>.json` of the channel repo at `repo` and declares `template <name>` in
/// `brand/brand.kdl`, named after the file, with the slots the file offers ([`lottie::slots`]),
/// none of them required. A slot id that is no valid name in `brand.kdl` is left out: no
/// overlay could fill it.
pub fn add_lottie(repo: &Path, source: &Path) -> Result<LottieImport, String> {
    let bytes = fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let findings = lottie::check(&bytes);
    if !findings.is_empty() {
        return Ok(LottieImport::Refused(findings));
    }
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let slug: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = Id::new(slug.trim_matches('-'))
        .ok_or_else(|| format!("name the file with a letter first, not `{stem}`"))?;
    if overlay::spec(name.as_str()).is_some() {
        return Err(format!(
            "`{name}` is a built-in template; rename the file to add it as a sticker"
        ));
    }
    let brand_path = repo.join("brand/brand.kdl");
    let mut brand = match fs::read_to_string(&brand_path) {
        Ok(text) => Brand::parse(&text).map_err(|e| format!("brand/brand.kdl:{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Brand::default(),
        Err(e) => return Err(format!("{}: {e}", brand_path.display())),
    };
    if brand.templates.iter().any(|t| t.name == name) {
        return Err(format!("the brand already declares `{name}`"));
    }
    let file = format!("brand/{name}.json");
    let target = repo.join(&file);
    if target.exists() {
        return Err(format!("{file} already exists"));
    }
    fs::create_dir_all(repo.join("brand")).map_err(|e| e.to_string())?;
    fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
    let slots = lottie::slots(&bytes)
        .map_err(|e| format!("{}: {e}", source.display()))?
        .into_iter()
        .filter_map(|(id, kind)| {
            Some(Slot {
                name: Id::new(id)?,
                kind: match kind {
                    SlotKind::Text => SlotType::Text,
                    SlotKind::Color => SlotType::Color,
                    SlotKind::Integer => SlotType::Integer,
                    SlotKind::Time => SlotType::Time,
                },
                required: false,
            })
        })
        .collect();
    brand.templates.push(LottieTemplate {
        name: name.clone(),
        file: file.clone(),
        slots,
    });
    brand.templates.sort_by(|a, b| a.name.cmp(&b.name));
    let tmp = brand_path.with_extension("tmp");
    fs::write(&tmp, brand.to_kdl())
        .and_then(|()| fs::rename(&tmp, &brand_path))
        .map_err(|e| format!("{}: {e}", brand_path.display()))?;
    Ok(LottieImport::Added { name, file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Element;

    fn id(text: &str) -> Id {
        Id::new(text).unwrap()
    }

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn elements() -> Elements {
        let mut e = Elements::new(id("t01"));
        e.push(
            Element::over(ElementKind::Word, range(1_000_000, 1_400_000), "test")
                .with_text(shot_core::Attr::Text, "deploy"),
        )
        .unwrap();
        e.push(
            Element::at(ElementKind::Click, Micros(2_000_000), "events")
                .with_integer(shot_core::Attr::X, 1200)
                .with_integer(shot_core::Attr::Y, 640),
        )
        .unwrap();
        e
    }

    #[test]
    fn a_sound_is_an_sfx_override_on_the_word_spoken_then() {
        let cut = Cut::parse(
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=9_000_000\n    }\n}\n",
        )
        .unwrap();
        let e = elements();
        let placed = with_sound(&cut, &id("t01"), Some(&e), Micros(1_200_000), id("pop")).unwrap();
        let again =
            with_sound(&placed, &id("t01"), Some(&e), Micros(1_300_000), id("tick")).unwrap();
        assert!(
            again
                .to_kdl()
                .ends_with("override t01 word at=1_000_000 {\n    sfx tick\n}\n"),
            "{}",
            again.to_kdl()
        );
        assert!(with_sound(&cut, &id("t01"), Some(&e), Micros(5_000_000), id("pop")).is_err());
    }

    #[test]
    fn a_callout_snaps_to_a_click_near_where_it_is_drawn() {
        let e = elements();
        let snapped = callout(Some(&e), Micros(2_400_000), Point { x: 1250, y: 700 });
        assert_eq!(snapped.range, range(2_400_000, 5_400_000));
        assert_eq!(
            snapped.params,
            [
                (id("x"), ParamValue::Integer(1200)),
                (id("y"), ParamValue::Integer(640))
            ]
        );
        let free = callout(Some(&e), Micros(2_400_000), Point { x: 3000, y: 100 });
        assert_eq!(free.params[0], (id("x"), ParamValue::Integer(3000)));
        assert!(
            overlay::check(&snapped).is_ok()
                && overlay::check(&pop(Micros(0), Pop::Emoji("🔥"))).is_ok()
        );
    }

    /// Adds a fill with the slot id `accent` to the first shape group of a Lottie.
    fn add_slotted_fill(value: &mut serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) if map.get("ty").is_some_and(|t| t == "gr") => {
                let fill = serde_json::json!({
                    "ty": "fl",
                    "c": { "a": 0, "k": [1, 0, 0], "sid": "accent" },
                    "o": { "a": 0, "k": 100 }
                });
                map["it"]
                    .as_array_mut()
                    .expect("a group lists its items")
                    .insert(1, fill);
                true
            }
            serde_json::Value::Object(map) => map.values_mut().any(add_slotted_fill),
            serde_json::Value::Array(items) => items.iter_mut().any(add_slotted_fill),
            _ => false,
        }
    }

    #[test]
    fn a_lottie_is_added_to_the_brand_only_when_it_renders() {
        let repo = tempfile::tempdir().unwrap();
        let file = |name: &str, json: &str| {
            let path = repo.path().join(name);
            fs::write(&path, json).unwrap();
            path
        };
        let shape = include_str!("../../shot-render/assets/emoji/1f525.json");
        let mut json: serde_json::Value = serde_json::from_str(shape).unwrap();
        json["layers"][0]["ks"]["p"] =
            serde_json::json!({ "a": 0, "k": [0, 0], "x": "wiggle(2, 30)" });
        let expression = json.to_string();
        let refused = add_lottie(repo.path(), &file("Wiggle.json", &expression)).unwrap();
        let LottieImport::Refused(findings) = refused else {
            panic!("{refused:?}");
        };
        assert!(
            findings[0].to_string().contains("wiggle(2, 30)"),
            "{findings:?}"
        );
        assert!(!repo.path().join("brand/brand.kdl").exists());

        let mut slotted: serde_json::Value = serde_json::from_str(shape).unwrap();
        assert!(add_slotted_fill(&mut slotted));
        let slotted = slotted.to_string();
        let added = add_lottie(repo.path(), &file("Party Hat.json", &slotted)).unwrap();
        assert_eq!(
            added,
            LottieImport::Added {
                name: id("party-hat"),
                file: "brand/party-hat.json".into()
            }
        );
        let brand = Brand::parse(&fs::read_to_string(repo.path().join("brand/brand.kdl")).unwrap())
            .unwrap();
        assert_eq!(stickers(&brand).collect::<Vec<_>>(), [&id("party-hat")]);
        assert_eq!(
            brand.templates[0].slots,
            [
                Slot {
                    name: id("accent"),
                    kind: SlotType::Color,
                    required: false
                },
                // The bundled emoji's `rest` marker.
                Slot {
                    name: id("rest"),
                    kind: SlotType::Time,
                    required: false
                }
            ]
        );
        assert!(add_lottie(repo.path(), &file("Party Hat.json", shape)).is_err());
    }
}
