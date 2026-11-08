//! What the Library lists: every sound, music file and image on disk with its credit, the
//! overlay templates with the bundled emoji and the brand's stickers, the effects, and the
//! Openverse results, each with a license and a source. Pure: reading the folders is
//! [`shot_library::Folders`]' job.

use std::time::Duration;

use shot_core::{Brand, CaptionStyle, Id, TransitionKind};
use shot_library::{Asset, Hit, Origin, Section as AssetSection};
use shot_render::overlay::{TEMPLATES, TemplateSpec, pop::EMOJI};

use crate::edit::library::Placement;
use crate::place::{self, Pop};

/// A section of the Library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    All,
    Sounds,
    Music,
    Images,
    Overlays,
    Effects,
}

impl Section {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Sounds,
        Self::Music,
        Self::Images,
        Self::Overlays,
        Self::Effects,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Everything",
            Self::Sounds => "Sounds",
            Self::Music => "Music",
            Self::Images => "Memes and images",
            Self::Overlays => "Overlays",
            Self::Effects => "Effects",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::All => "library",
            Self::Sounds => "sound",
            Self::Music => "music",
            Self::Images => "image",
            Self::Overlays => "layout",
            Self::Effects => "sparkles",
        }
    }

    /// What Openverse searches in this section, if it searches online.
    pub fn online(self) -> Option<shot_library::Media> {
        match self {
            Self::Sounds => Some(shot_library::Media::Audio),
            Self::Images => Some(shot_library::Media::Image),
            _ => None,
        }
    }
}

/// An effect the format and the rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectItem {
    Zoom,
    Transition(TransitionKind),
    Captions(CaptionStyle),
}

/// One entry of the Library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A sound, music file or image on disk, or a bundled sound or bed.
    Asset(Asset),
    Template(&'static TemplateSpec),
    Emoji(&'static str),
    Sticker(Id),
    Effect(EffectItem),
    /// An Openverse result, not downloaded yet.
    Online(Hit),
}

impl Item {
    pub fn section(&self) -> Section {
        match self {
            Self::Asset(asset) => match asset.section {
                AssetSection::Sounds => Section::Sounds,
                AssetSection::Music => Section::Music,
                AssetSection::Images => Section::Images,
            },
            Self::Template(_) | Self::Emoji(_) | Self::Sticker(_) => Section::Overlays,
            Self::Effect(_) => Section::Effects,
            Self::Online(hit) => match hit.media {
                shot_library::Media::Audio => Section::Sounds,
                shot_library::Media::Image => Section::Images,
            },
        }
    }

    /// Unique within the Library, to keep the selection and the audition apart.
    pub fn key(&self) -> String {
        match self {
            Self::Asset(asset) => match &asset.path {
                Some(path) => path.display().to_string(),
                None => format!("bundled:{:?}:{}", asset.section, asset.name),
            },
            Self::Template(spec) => format!("template:{}", spec.name),
            Self::Emoji(emoji) => format!("emoji:{emoji}"),
            Self::Sticker(name) => format!("sticker:{name}"),
            Self::Effect(effect) => format!("effect:{effect:?}"),
            Self::Online(hit) => format!("openverse:{}", hit.id),
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::Asset(asset) => asset
                .credit
                .as_ref()
                .map_or_else(|| asset.name.clone(), |c| c.title.clone()),
            Self::Template(spec) => template_title(spec.name),
            Self::Emoji(emoji) => format!("{emoji} pop"),
            Self::Sticker(name) => format!("{name} sticker"),
            Self::Effect(EffectItem::Zoom) => "Punch-in zoom".into(),
            Self::Effect(EffectItem::Transition(kind)) => {
                format!("{} transition", sentence(kind.as_str()))
            }
            Self::Effect(EffectItem::Captions(style)) => {
                format!("{} captions", sentence(style.as_str()))
            }
            Self::Online(hit) => hit.title.clone(),
        }
    }

    /// One line under the title.
    pub fn about(&self) -> String {
        match self {
            Self::Asset(asset) => match (&asset.credit, asset.section) {
                (Some(credit), _) => credit
                    .creator
                    .as_ref()
                    .map_or_else(|| "Downloaded".into(), |c| format!("by {}", c.name)),
                (None, AssetSection::Sounds) => format!("Placed as sfx {}", asset.name),
                (None, AssetSection::Music) => "A bed under the whole cut".into(),
                (None, AssetSection::Images) => "A still for a pop or a cutaway".into(),
            },
            Self::Template(spec) => sentence(spec.about),
            Self::Emoji(_) => "An animated emoji popping in".into(),
            Self::Sticker(_) => "A brand Lottie popping in".into(),
            Self::Effect(EffectItem::Zoom) => "Pushes in on the selected words".into(),
            Self::Effect(EffectItem::Transition(kind)) => match kind {
                TransitionKind::Cut => "A hard cut between chapters".into(),
                TransitionKind::Dissolve => "Crossfades the chapters".into(),
                TransitionKind::Dip => "Dips through the brand background".into(),
                TransitionKind::Stinger => "The brand's Lottie covers the cut".into(),
                TransitionKind::MaskWipe => "Opens a shape from the middle".into(),
                TransitionKind::ZoomThrough => "Punches both sides in towards the cut".into(),
            },
            Self::Effect(EffectItem::Captions(style)) => match style {
                CaptionStyle::WordPop => "Words pop as they are spoken".into(),
                CaptionStyle::Clean => "Plain lines".into(),
                CaptionStyle::Off => "No burned-in captions".into(),
            },
            Self::Online(hit) => hit
                .creator
                .as_ref()
                .map_or_else(|| "Openverse".into(), |c| format!("by {}", c.name)),
        }
    }

    /// The license, such as "CC BY 2.0", or what stands in for one.
    pub fn license(&self) -> String {
        match self {
            Self::Asset(asset) => match (&asset.credit, asset.origin) {
                (Some(credit), _) => credit.license_name(),
                (None, Origin::Bundled) => "CC0".into(),
                (None, _) => "Your file".into(),
            },
            Self::Template(_) | Self::Effect(_) => "Built in".into(),
            Self::Emoji(_) => "CC BY 4.0".into(),
            Self::Sticker(_) => "Your file".into(),
            Self::Online(hit) => hit.credit().license_name(),
        }
    }

    /// Where it comes from: the provider of a download or a result, else where it lives.
    pub fn source(&self) -> String {
        match self {
            Self::Asset(asset) => match asset.credit.as_ref().and_then(|c| c.provider.clone()) {
                Some(provider) => sentence(&provider),
                None => asset.origin.label().into(),
            },
            Self::Template(_) | Self::Effect(_) => "Shot Studio".into(),
            Self::Emoji(_) => "Noto emoji".into(),
            Self::Sticker(_) => "Brand".into(),
            Self::Online(hit) => hit
                .provider
                .as_deref()
                .map_or_else(|| "Openverse".into(), sentence),
        }
    }

    pub fn tags(&self) -> Vec<String> {
        match self {
            Self::Asset(asset) => asset
                .credit
                .as_ref()
                .map(|c| c.tags.clone())
                .unwrap_or_default(),
            Self::Online(hit) => hit.tags.clone(),
            Self::Template(spec) => spec.params.iter().map(|p| p.name.to_owned()).collect(),
            _ => Vec::new(),
        }
    }

    /// How long a sound plays, when it is known.
    pub fn duration(&self) -> Option<Duration> {
        match self {
            Self::Online(hit) => hit.duration,
            _ => None,
        }
    }

    /// Whether every word of the lowercase query is in the title, name, tags or source.
    pub fn matches(&self, words: &[String]) -> bool {
        if words.is_empty() {
            return true;
        }
        if let Self::Asset(asset) = self
            && asset.matches(words)
        {
            return true;
        }
        let text = format!(
            "{} {} {} {}",
            self.title(),
            self.about(),
            self.tags().join(" "),
            self.source()
        )
        .to_lowercase();
        words.iter().all(|w| text.contains(w.as_str()))
    }

    /// Whether it plays on its own, with Space.
    pub fn audible(&self) -> bool {
        matches!(self, Self::Online(hit) if hit.media == shot_library::Media::Audio)
            || matches!(self, Self::Asset(a) if a.section != AssetSection::Images)
    }

    /// Whether it is a still that previews larger with Space.
    pub fn still(&self) -> bool {
        matches!(self, Self::Online(hit) if hit.media == shot_library::Media::Image)
            || matches!(self, Self::Asset(a) if a.section == AssetSection::Images)
    }

    /// What P places. An image pops in; [`Item::cutaway`] places it as b-roll.
    pub fn placement(
        &self,
        video_relative: impl Fn(&Asset) -> Option<String>,
    ) -> Option<Placement> {
        match self {
            Self::Asset(asset) => match asset.section {
                AssetSection::Sounds => Id::new(&asset.name).map(Placement::Sound),
                AssetSection::Music => match &asset.path {
                    Some(path) => Some(Placement::MusicFile(path.clone())),
                    None => shot_audio::sound::bundled(&asset.name).map(|b| Placement::Bed(b.name)),
                },
                AssetSection::Images => video_relative(asset).map(Placement::ImagePop),
            },
            Self::Emoji(emoji) => Some(Placement::Pop(Pop::Emoji(emoji))),
            Self::Sticker(name) => Some(Placement::Pop(Pop::Sticker(name.clone()))),
            Self::Effect(EffectItem::Zoom) => Some(Placement::Zoom),
            Self::Template(_) | Self::Effect(_) | Self::Online(_) => None,
        }
    }

    /// What B places: an image as a b-roll cutaway.
    pub fn cutaway(&self, video_relative: impl Fn(&Asset) -> Option<String>) -> Option<Placement> {
        match self {
            Self::Asset(asset) if asset.section == AssetSection::Images => {
                video_relative(asset).map(Placement::Cutaway)
            }
            _ => None,
        }
    }
}

/// The overlay templates, the bundled emoji pops and the brand's stickers.
pub fn overlays(brand: Option<&Brand>) -> Vec<Item> {
    let mut items: Vec<Item> = TEMPLATES.into_iter().map(Item::Template).collect();
    items.extend(EMOJI.iter().map(|(emoji, _)| Item::Emoji(emoji)));
    items.extend(
        brand
            .into_iter()
            .flat_map(place::stickers)
            .cloned()
            .map(Item::Sticker),
    );
    items
}

/// The zoom, the chapter transitions and the caption styles.
pub fn effects() -> Vec<Item> {
    let mut items = vec![Item::Effect(EffectItem::Zoom)];
    items.extend(
        TransitionKind::ALL
            .iter()
            .map(|k| Item::Effect(EffectItem::Transition(*k))),
    );
    items.extend(
        CaptionStyle::ALL
            .iter()
            .map(|s| Item::Effect(EffectItem::Captions(*s))),
    );
    items
}

/// The query as lowercase words.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn template_title(name: &str) -> String {
    match name {
        "b-roll" => "B-roll".into(),
        name => sentence(&name.replace('-', " ")),
    }
}

/// `text` with its first letter capitalized.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled(section: AssetSection, name: &str) -> Item {
        Item::Asset(Asset {
            section,
            name: name.into(),
            origin: Origin::Bundled,
            path: None,
            credit: None,
        })
    }

    #[test]
    fn every_item_names_a_license_and_a_source() {
        let mut items = overlays(None);
        items.extend(effects());
        items.push(bundled(AssetSection::Sounds, "whoosh"));
        for item in &items {
            assert!(
                !item.license().is_empty() && !item.source().is_empty(),
                "{item:?}"
            );
        }
        assert_eq!(bundled(AssetSection::Sounds, "whoosh").license(), "CC0");
        assert_eq!(
            items
                .iter()
                .filter(|i| i.section() == Section::Effects)
                .count(),
            10
        );
    }

    #[test]
    fn local_search_matches_every_word_across_titles_and_tags() {
        let whoosh = bundled(AssetSection::Sounds, "whoosh");
        assert!(whoosh.matches(&words("WHOO")));
        assert!(!whoosh.matches(&words("whoosh pop")));
        let lower = Item::Template(shot_render::overlay::spec("lower-third").unwrap());
        assert_eq!(lower.title(), "Lower third");
        assert!(lower.matches(&words("lower subtitle")));
        assert!(Item::Emoji("🔥").matches(&words("pop")));
    }

    #[test]
    fn placing_a_bundled_sound_or_bed_names_it() {
        let place = |item: &Item| item.placement(|_| None);
        assert_eq!(
            place(&bundled(AssetSection::Sounds, "pop")),
            Some(Placement::Sound(Id::new("pop").unwrap()))
        );
        assert_eq!(
            place(&bundled(AssetSection::Music, "bed-calm")),
            Some(Placement::Bed("bed-calm"))
        );
        assert_eq!(place(&Item::Template(TEMPLATES[0])), None);
    }
}
