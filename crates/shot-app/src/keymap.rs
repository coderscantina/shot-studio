//! Every key binding of the app, in one list grouped by key context.
//!
//! A context names the element that declares it with `.key_context(..)`. Bindings without one work
//! in every window. A deeper context wins over a shallower one, so a screen's Esc or ⌘Z beats the
//! app-wide one, and the text fields of gpui-component bind their own keys in their own context.

use gpui::{App, KeyBinding};

use crate::{
    about, board, brand, cut, edit, layouts::builder, library, prompter, record, script, ship,
    studio, takes, welcome,
};

/// Binds the list. Call once at startup, before the menus are set, so they show the shortcuts.
pub fn bind(cx: &mut App) {
    cx.bind_keys(bindings());
}

fn bindings() -> Vec<KeyBinding> {
    let prompter_output = Some(prompter::OUTPUT_CONTEXT);
    let edit = Some(edit::CONTEXT);
    let knob = Some(edit::zoom::KNOB_CONTEXT);
    let switch = Some(edit::switch::CONTEXT);
    let record = Some(record::CONTEXT);
    let cut = Some(cut::CONTEXT);
    let takes = Some(takes::CONTEXT);
    let script = Some(script::CONTEXT);
    let ship = Some(ship::CONTEXT);
    let thumbnail = Some(ship::thumbnail::CONTEXT);
    let board = Some(board::CONTEXT);
    let builder = Some(builder::CONTEXT);
    let library = Some(library::WINDOW);
    let library_list = Some(library::LIST);

    vec![
        // Every window
        KeyBinding::new("cmd-q", about::Quit, None),
        KeyBinding::new("cmd-shift-p", prompter::TogglePrompter, None),
        KeyBinding::new("cmd-o", welcome::OpenVideo, None),
        KeyBinding::new("cmd-z", studio::Undo, None),
        KeyBinding::new("cmd-shift-z", studio::Redo, None),
        KeyBinding::new("cmd-l", studio::OpenLibrary, None),
        KeyBinding::new("cmd-shift-b", brand::OpenTheme, None),
        KeyBinding::new("cmd-shift-o", brand::OpenOverlays, None),
        // Prompter output
        KeyBinding::new("escape", prompter::HidePrompter, prompter_output),
        // Edit screen
        KeyBinding::new("space", edit::TogglePlay, edit),
        KeyBinding::new("backspace", edit::DeleteSelection, edit),
        KeyBinding::new("delete", edit::DeleteSelection, edit),
        KeyBinding::new("r", edit::RestoreSelection, edit),
        KeyBinding::new("escape", edit::ClearSelection, edit),
        KeyBinding::new("1", edit::Layout1, edit),
        KeyBinding::new("2", edit::Layout2, edit),
        KeyBinding::new("3", edit::Layout3, edit),
        KeyBinding::new("4", edit::Layout4, edit),
        KeyBinding::new("alt-1", edit::ShowText, edit),
        KeyBinding::new("alt-2", edit::ShowPicture, edit),
        KeyBinding::new("alt-3", edit::ShowTimeline, edit),
        KeyBinding::new("left", edit::PreviousWord, edit),
        KeyBinding::new("right", edit::NextWord, edit),
        KeyBinding::new("shift-left", edit::ExtendPrevious, edit),
        KeyBinding::new("shift-right", edit::ExtendNext, edit),
        KeyBinding::new("tab", edit::AcceptProposal, edit),
        KeyBinding::new("shift-tab", edit::FocusPrevious, edit),
        KeyBinding::new("g", edit::ShowProposal, edit),
        KeyBinding::new("s", edit::PickSound, edit),
        KeyBinding::new("z", edit::AddZoom, edit),
        KeyBinding::new("=", edit::ZoomIn, edit),
        KeyBinding::new("-", edit::ZoomOut, edit),
        KeyBinding::new("shift-z", edit::ZoomToFit, edit),
        KeyBinding::new("alt-z", edit::ZoomToSelection, edit),
        KeyBinding::new("home", edit::GoToStart, edit),
        KeyBinding::new("end", edit::GoToEnd, edit),
        KeyBinding::new("cmd-enter", edit::RecordTake, edit),
        KeyBinding::new("enter", edit::EditCaption, edit),
        KeyBinding::new("h", edit::AddHighlight, edit),
        KeyBinding::new("p", edit::PickPop, edit),
        KeyBinding::new("l", edit::PickLayout, edit),
        KeyBinding::new("t", edit::EditSwitch, edit),
        KeyBinding::new("shift-l", edit::BuildLayout, edit),
        // Edit screen, a focused knob
        KeyBinding::new("left", edit::zoom::StepDown, knob),
        KeyBinding::new("right", edit::zoom::StepUp, knob),
        KeyBinding::new("down", edit::zoom::StepDown, knob),
        KeyBinding::new("up", edit::zoom::StepUp, knob),
        KeyBinding::new("shift-left", edit::zoom::StepDownMore, knob),
        KeyBinding::new("shift-right", edit::zoom::StepUpMore, knob),
        // Edit screen, scene switch
        KeyBinding::new("1", edit::switch::MorphSwitch, switch),
        KeyBinding::new("2", edit::switch::DissolveSwitch, switch),
        KeyBinding::new("3", edit::switch::ZoomThroughSwitch, switch),
        KeyBinding::new("4", edit::switch::CutSwitch, switch),
        KeyBinding::new("5", edit::switch::DipSwitch, switch),
        KeyBinding::new("6", edit::switch::StingerSwitch, switch),
        KeyBinding::new("7", edit::switch::MaskWipeSwitch, switch),
        KeyBinding::new("[", edit::switch::ShorterSwitch, switch),
        KeyBinding::new("]", edit::switch::LongerSwitch, switch),
        KeyBinding::new("space", edit::switch::PlaySwitch, switch),
        KeyBinding::new("backspace", edit::switch::FormatSwitch, switch),
        KeyBinding::new("delete", edit::switch::FormatSwitch, switch),
        KeyBinding::new("escape", edit::switch::CloseSwitch, switch),
        // Record screen
        KeyBinding::new("space", record::ToggleRecording, record),
        // Cut screen
        KeyBinding::new("k", cut::KeepItem, cut),
        KeyBinding::new("c", cut::CutItem, cut),
        KeyBinding::new("down", cut::NextItem, cut),
        KeyBinding::new("up", cut::PreviousItem, cut),
        KeyBinding::new("space", cut::PlayAround, cut),
        KeyBinding::new("shift-space", cut::PlayCut, cut),
        KeyBinding::new("cmd-enter", cut::Continue, cut),
        KeyBinding::new("r", cut::ReviewApplied, cut),
        KeyBinding::new("alt-1", cut::ShowReview, cut),
        KeyBinding::new("alt-2", cut::ShowTakes, cut),
        // Cut screen, takes view
        KeyBinding::new("alt-1", cut::ShowReview, takes),
        KeyBinding::new("alt-2", cut::ShowTakes, takes),
        KeyBinding::new("down", takes::NextBeat, takes),
        KeyBinding::new("up", takes::PreviousBeat, takes),
        KeyBinding::new("right", takes::NextAttempt, takes),
        KeyBinding::new("left", takes::PreviousAttempt, takes),
        KeyBinding::new("space", takes::Play, takes),
        KeyBinding::new("a", takes::Compare, takes),
        KeyBinding::new("1", takes::Rate1, takes),
        KeyBinding::new("2", takes::Rate2, takes),
        KeyBinding::new("3", takes::Rate3, takes),
        KeyBinding::new("4", takes::Rate4, takes),
        KeyBinding::new("5", takes::Rate5, takes),
        KeyBinding::new("p", takes::Pick, takes),
        KeyBinding::new("enter", takes::UseThis, takes),
        KeyBinding::new("shift-enter", takes::Assemble, takes),
        KeyBinding::new("shift-right", takes::MarkMore, takes),
        KeyBinding::new("shift-left", takes::MarkLess, takes),
        KeyBinding::new("escape", takes::ClearMark, takes),
        KeyBinding::new("cmd-enter", takes::Continue, takes),
        // Script screen
        KeyBinding::new("tab", script::AcceptProposal, script),
        KeyBinding::new("escape", script::RejectProposal, script),
        KeyBinding::new("cmd-enter", script::ContinueToRecord, script),
        // Ship screen
        KeyBinding::new("cmd-enter", ship::ShipVideo, ship),
        KeyBinding::new("e", ship::ExportCut, ship),
        KeyBinding::new("t", ship::FindThumbnails, ship),
        KeyBinding::new("d", ship::OpenStudio, ship),
        KeyBinding::new("escape", ship::Stop, ship),
        KeyBinding::new("down", ship::NextCut, ship),
        KeyBinding::new("up", ship::PreviousCut, ship),
        // Ship screen, thumbnail studio
        KeyBinding::new("escape", ship::thumbnail::CloseStudio, thumbnail),
        KeyBinding::new("cmd-z", ship::thumbnail::UndoDesign, thumbnail),
        KeyBinding::new("cmd-shift-z", ship::thumbnail::RedoDesign, thumbnail),
        KeyBinding::new("backspace", ship::thumbnail::RemoveSticker, thumbnail),
        KeyBinding::new("delete", ship::thumbnail::RemoveSticker, thumbnail),
        KeyBinding::new("alt-1", ship::thumbnail::PresetOne, thumbnail),
        KeyBinding::new("alt-2", ship::thumbnail::PresetTwo, thumbnail),
        KeyBinding::new("alt-3", ship::thumbnail::PresetThree, thumbnail),
        KeyBinding::new("alt-4", ship::thumbnail::PresetFour, thumbnail),
        // Board
        KeyBinding::new("left", board::SelectLeft, board),
        KeyBinding::new("right", board::SelectRight, board),
        KeyBinding::new("up", board::SelectUp, board),
        KeyBinding::new("down", board::SelectDown, board),
        KeyBinding::new("alt-left", board::MoveLeft, board),
        KeyBinding::new("alt-right", board::MoveRight, board),
        KeyBinding::new("enter", board::OpenSelected, board),
        KeyBinding::new("n", board::NewVideo, board),
        KeyBinding::new("escape", board::Deselect, board),
        // Layout builder
        KeyBinding::new("left", builder::NudgeLeft, builder),
        KeyBinding::new("right", builder::NudgeRight, builder),
        KeyBinding::new("up", builder::NudgeUp, builder),
        KeyBinding::new("down", builder::NudgeDown, builder),
        KeyBinding::new("shift-left", builder::JumpLeft, builder),
        KeyBinding::new("shift-right", builder::JumpRight, builder),
        KeyBinding::new("shift-up", builder::JumpUp, builder),
        KeyBinding::new("shift-down", builder::JumpDown, builder),
        KeyBinding::new("alt-left", builder::Narrower, builder),
        KeyBinding::new("alt-right", builder::Wider, builder),
        KeyBinding::new("alt-up", builder::Shorter, builder),
        KeyBinding::new("alt-down", builder::Taller, builder),
        KeyBinding::new("alt-shift-left", builder::MuchNarrower, builder),
        KeyBinding::new("alt-shift-right", builder::MuchWider, builder),
        KeyBinding::new("alt-shift-up", builder::MuchShorter, builder),
        KeyBinding::new("alt-shift-down", builder::MuchTaller, builder),
        KeyBinding::new("tab", builder::NextSlot, builder),
        KeyBinding::new("]", builder::BringForward, builder),
        KeyBinding::new("[", builder::SendBack, builder),
        KeyBinding::new("backspace", builder::RemoveSlot, builder),
        KeyBinding::new("delete", builder::RemoveSlot, builder),
        KeyBinding::new("cmd-s", builder::SaveLayout, builder),
        KeyBinding::new("cmd-enter", builder::SaveLayout, builder),
        KeyBinding::new("escape", builder::CloseBuilder, builder),
        KeyBinding::new("cmd-w", builder::CloseBuilder, builder),
        // Library window
        KeyBinding::new("cmd-f", library::FocusSearch, library),
        KeyBinding::new("tab", library::ToggleOnline, library),
        KeyBinding::new("cmd-0", library::ShowAll, library),
        KeyBinding::new("cmd-1", library::ShowSounds, library),
        KeyBinding::new("cmd-2", library::ShowMusic, library),
        KeyBinding::new("cmd-3", library::ShowImages, library),
        KeyBinding::new("cmd-4", library::ShowOverlays, library),
        KeyBinding::new("cmd-5", library::ShowEffects, library),
        KeyBinding::new("cmd-shift-c", library::CopyCredits, library),
        KeyBinding::new("cmd-w", library::CloseLibrary, library),
        // Library window, result list
        KeyBinding::new("down", library::SelectNext, library_list),
        KeyBinding::new("up", library::SelectPrevious, library_list),
        KeyBinding::new("left", library::SelectLeft, library_list),
        KeyBinding::new("right", library::SelectRight, library_list),
        KeyBinding::new("space", library::Listen, library_list),
        KeyBinding::new("enter", library::Primary, library_list),
        KeyBinding::new("d", library::Download, library_list),
        KeyBinding::new("p", library::Place, library_list),
        KeyBinding::new("b", library::Cutaway, library_list),
        KeyBinding::new("escape", library::FocusSearch, library_list),
        KeyBinding::new("/", library::FocusSearch, library_list),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use gpui::AsKeystroke;

    use super::*;

    #[test]
    fn no_two_bindings_share_a_key_in_one_context() {
        let mut seen = HashMap::new();
        for binding in bindings() {
            let context = binding.predicate().map(|predicate| predicate.to_string());
            let keys: Vec<_> = binding
                .keystrokes()
                .iter()
                .map(|key| key.as_keystroke().clone())
                .collect();
            let action = binding.action().name();
            if let Some(earlier) = seen.insert((context.clone(), keys.clone()), action) {
                panic!("{keys:?} in {context:?} binds both {earlier} and {action}");
            }
        }
    }

    #[test]
    fn every_binding_names_a_registered_action() {
        let registered: HashSet<_> = gpui::generate_list_of_all_registered_actions()
            .map(|action| action.name)
            .collect();
        for binding in bindings() {
            let name = binding.action().name();
            assert!(
                registered.contains(name),
                "{name} is not a registered action"
            );
        }
    }
}
