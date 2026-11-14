//! The video's `format.kdl` layer the app writes for a settings change. Pure, over
//! `shot-core` types; [`crate::folder`] does the IO.

use shot_core::{Format, FormatLayer};

/// The video's `format.kdl` layer that gives `format` over `base`, the settings below it: the
/// settings that differ, with the layer's brand fields and rules kept. An optional setting
/// `format` leaves at its default is written out, since leaving it out inherits `base`'s.
pub fn layer(format: &Format, base: &Format, current: &FormatLayer) -> FormatLayer {
    fn differs<T: PartialEq>(a: T, b: T) -> Option<T> {
        (a != b).then_some(a)
    }
    let layouts = |f: &Format| {
        std::iter::once(f.default_layout)
            .chain(f.other_layouts.iter().copied())
            .collect::<Vec<_>>()
    };
    let effects = format.effects();
    let caption_look = format.caption_look.diff(&base.caption_look);
    FormatLayer {
        prompter: differs(format.prompter, base.prompter),
        layouts: differs(layouts(format), layouts(base)),
        // A look needs its node, which names the style.
        captions: (format.captions != base.captions || !caption_look.is_empty())
            .then_some(format.captions),
        caption_look,
        zoom_motion: (format.zoom_motion != base.zoom_motion).then_some(effects.zoom_motion),
        cursor: (format.cursor != base.cursor).then_some(effects.cursor),
        keystrokes: (format.keystrokes != base.keystrokes).then_some(effects.keystrokes),
        window: (format.window != base.window).then_some(effects.window),
        layout_switch: (format.layout_switch != base.layout_switch).then(|| format.layout_switch()),
        inputs: differs(format.inputs(), base.inputs()),
        brand: current.brand.clone(),
        rules: current.rules.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{FormatKind, PrompterMode};

    #[test]
    fn a_format_change_writes_only_what_differs_from_the_channel() {
        let base = FormatKind::Opinion.preset();
        let current = FormatLayer::parse("on filler off\n").unwrap();
        let mut format = base.clone();
        assert_eq!(layer(&format, &base, &current), current);
        format.prompter = PrompterMode::FullScript;
        assert_eq!(
            layer(&format, &base, &current).to_kdl(),
            "prompter full-script\non filler off\n"
        );
    }
}
