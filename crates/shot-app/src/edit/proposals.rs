//! Agent cut proposals in the Edit screen ([0004]).
//!
//! A proposal that changes `cut.kdl` shows inline in the Text workspace where it applies: the
//! words it removes struck, the words it brings back inserted, the time it saves, and Accept
//! (Tab), Reject (Esc) and Another take. Away from the text (the Timeline workspace) a card says
//! where it applies and offers Show at (G). A lime spark marks each one on the overview bar. The
//! branch behind a proposal stays out of sight.
//!
//! What a proposal changes is read word by word from its `cut.kdl` where it branched and at its
//! head, so it shows on the cut on disk even after other edits. A cut proposal only removes or
//! restores recorded words, so that is all this shows.
//!
//! Play on a card previews the proposal: the preview plays the plan of the proposal's files on
//! top of the video's, styled by a lock built in memory, from a little before its first change.
//! Esc returns to the branch. The branch's `cut.lock` and cache are never written.
//!
//! [0004]: ../../../../docs/decisions/0004-proposals-as-branches.md

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use gpui::{
    Context, Div, FontWeight, Hsla, MouseButton, SharedString, Stateful, div, prelude::*, px,
};
use shot_board::Board;
use shot_core::{
    Cut, Id, Lock, Micros, Project, RangeEdit, TakeAt, TimeRange, Transcript, WordState,
};
use shot_mcp::agents::ScriptCommand;
use shot_mcp::style::{self, Disk, Patched};

use super::{EditScreen, Workspace};
use crate::agent::{self, AgentRun};
use crate::engine::Plan;
use crate::model::{self, BlockDoc, DocWord, EditMap, FPS, Item, Paragraph, timecode};
use crate::script::proposal::{self as words, Piece};
use crate::theme::*;
use crate::ui::{ActionLook, icon, kbd, proposal_action};

/// A span of recorded words of one take block that a proposal removes or brings back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordChange {
    pub at: TakeAt,
    pub removes: bool,
    /// From the start of the first word to the end of the last, in the take's media time.
    pub range: TimeRange,
}

impl WordChange {
    /// Whether the change turns `word` around on the cut as it is now.
    fn turns(&self, at: TakeAt, word: &DocWord) -> bool {
        let middle = midpoint(word.range);
        at == self.at
            && self.range.from() <= middle
            && middle < self.range.to()
            && (word.state == WordState::Kept) == self.removes
    }
}

/// An open proposal that changes the cut.
pub struct CutProposal {
    pub name: String,
    pub subject: String,
    base: Option<Cut>,
    head: Cut,
    pub changes: Vec<WordChange>,
}

impl CutProposal {
    /// `current` with the proposal applied: its head when nothing changed on disk since it
    /// branched, else its word changes on top of what is there.
    pub fn applied_to(
        &self,
        current: &Cut,
        transcripts: &HashMap<Id, Transcript>,
    ) -> Result<Cut, String> {
        if self.base.as_ref() == Some(current) {
            return Ok(self.head.clone());
        }
        apply(current, &self.changes, transcripts)
    }

    /// The change in words for Claude: take, word numbers and the words, one span a line.
    fn describe(&self, transcripts: &HashMap<Id, Transcript>) -> String {
        let mut out = String::new();
        for change in &self.changes {
            let Some(edit) = self
                .head
                .chapters
                .get(change.at.chapter)
                .and_then(|c| c.takes.get(change.at.take))
            else {
                continue;
            };
            let Some(transcript) = transcripts.get(&edit.take) else {
                continue;
            };
            let all = transcript.words();
            let first = all.partition_point(|w| w.range.from() < change.range.from());
            let end = all.partition_point(|w| w.range.from() < change.range.to());
            let text: Vec<&str> = all[first..end].iter().map(|w| w.text.as_str()).collect();
            out.push_str(&format!(
                "{} {} w{first}-w{} \"{}\"\n",
                if change.removes {
                    "removes"
                } else {
                    "restores"
                },
                edit.take,
                end.saturating_sub(1),
                text.join(" ")
            ));
        }
        out
    }
}

/// Where a proposal shows: a run of paragraphs of one take block, as struck and inserted words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Index into the screen's proposals.
    pub proposal: usize,
    pub at: TakeAt,
    pub paragraphs: Range<usize>,
    pub pieces: Vec<Piece>,
    /// Kept media the card's changes take out, negative when they add.
    pub saved: i64,
    /// Output frame of the first word it changes, else of its first paragraph.
    pub frame: Option<u64>,
    /// Its place among the proposal's cards, and how many there are.
    pub index: usize,
    pub count: usize,
}

/// The open proposals of `slug` in the channel repo `repo` that change recorded words.
pub fn load(
    repo: &Path,
    slug: &str,
    transcripts: &HashMap<Id, Transcript>,
) -> Result<Vec<CutProposal>, String> {
    let board = Board::open(repo).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for proposal in board.proposals(Some(slug)).map_err(|e| e.to_string())? {
        let (base, head) = board
            .proposal_file(slug, &proposal.name, "cut.kdl")
            .map_err(|e| e.to_string())?;
        let Some(Ok(head)) = head.map(|h| Cut::parse(&h)) else {
            continue;
        };
        let Ok(base) = base.map(|b| Cut::parse(&b)).transpose() else {
            continue;
        };
        let changes = word_changes(base.as_ref().unwrap_or(&Cut::default()), &head, transcripts);
        if !changes.is_empty() {
            found.push(CutProposal {
                name: proposal.name,
                subject: proposal.subject,
                base,
                head,
                changes,
            });
        }
    }
    Ok(found)
}

/// The words `head` keeps or removes differently from `base`, block by block, as spans. A span
/// runs on over words both already treat the way it does, so it covers what the proposal's
/// edit covered.
pub fn word_changes(
    base: &Cut,
    head: &Cut,
    transcripts: &HashMap<Id, Transcript>,
) -> Vec<WordChange> {
    let mut out = Vec::new();
    for (at, edit) in model::blocks(base) {
        let same_block = head
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .is_some_and(|h| h.take == edit.take);
        let Some(transcript) = transcripts.get(&edit.take).filter(|_| same_block) else {
            continue;
        };
        let (Ok(before), Ok(after)) = (
            base.transcript_view(at, transcript),
            head.transcript_view(at, transcript),
        ) else {
            continue;
        };
        let after: HashMap<Micros, bool> = after
            .iter()
            .map(|w| (w.word.range.from(), w.state == WordState::Kept))
            .collect();
        // (removes, first word, last changed word)
        let mut run: Option<(bool, TimeRange, TimeRange)> = None;
        let mut close = |run: &mut Option<(bool, TimeRange, TimeRange)>| {
            if let Some((removes, first, last)) = run.take() {
                out.push(WordChange {
                    at,
                    removes,
                    range: TimeRange::new(first.from(), last.to()).expect("words are ordered"),
                });
            }
        };
        for view in &before {
            let kept = view.state == WordState::Kept;
            let Some(&kept_after) = after.get(&view.word.range.from()) else {
                close(&mut run);
                continue;
            };
            match &mut run {
                // The run goes on over words the head treats the way it does.
                Some((removes, _, last)) if kept_after != *removes => {
                    if kept != kept_after {
                        *last = view.word.range;
                    }
                }
                _ => {
                    close(&mut run);
                    if kept != kept_after {
                        run = Some((kept, view.word.range, view.word.range));
                    }
                }
            }
        }
        close(&mut run);
    }
    out
}

/// Applies word changes through the same edits the app, CLI and MCP make.
pub fn apply(
    cut: &Cut,
    changes: &[WordChange],
    transcripts: &HashMap<Id, Transcript>,
) -> Result<Cut, String> {
    let mut cut = cut.clone();
    for change in changes {
        let take = cut
            .chapters
            .get(change.at.chapter)
            .and_then(|c| c.takes.get(change.at.take))
            .map(|edit| edit.take.clone())
            .ok_or("the proposal is for a chapter the cut no longer has")?;
        cut = if change.removes {
            let transcript = transcripts
                .get(&take)
                .ok_or_else(|| format!("take {take} has no transcript"))?;
            cut.delete_words(change.at, transcript, change.range)
        } else {
            cut.restore(change.at, change.range)
        }
        .map_err(|e| e.to_string())?;
    }
    Ok(cut)
}

/// The cards of `proposals` on the document `doc` of `cut`: each run of paragraphs that
/// proposals change gets one card per proposal changing it.
pub fn cards(
    proposals: &[CutProposal],
    doc: &[BlockDoc],
    cut: &Cut,
    transcripts: &HashMap<Id, Transcript>,
) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    for block in doc {
        let touching = |paragraph: &Paragraph| -> Vec<usize> {
            proposals
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    paragraph_words(paragraph)
                        .any(|i| p.changes.iter().any(|c| c.turns(block.at, &block.words[i])))
                })
                .map(|(i, _)| i)
                .collect()
        };
        let mut index = 0;
        while index < block.paragraphs.len() {
            if touching(&block.paragraphs[index]).is_empty() {
                index += 1;
                continue;
            }
            let start = index;
            let mut ids: Vec<usize> = Vec::new();
            while let Some(found) = block
                .paragraphs
                .get(index)
                .map(touching)
                .filter(|f| !f.is_empty())
            {
                ids.extend(found);
                index += 1;
            }
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                out.push(card(
                    &proposals[id],
                    id,
                    block,
                    start..index,
                    cut,
                    transcripts,
                ));
            }
        }
    }
    for card in 0..out.len() {
        let proposal = out[card].proposal;
        let same: Vec<usize> = (0..out.len())
            .filter(|&c| out[c].proposal == proposal)
            .collect();
        out[card].index = same.iter().position(|&c| c == card).unwrap_or_default();
        out[card].count = same.len();
    }
    out
}

fn card(
    proposal: &CutProposal,
    id: usize,
    block: &BlockDoc,
    paragraphs: Range<usize>,
    cut: &Cut,
    transcripts: &HashMap<Id, Transcript>,
) -> Card {
    let turned = |i: usize| {
        proposal
            .changes
            .iter()
            .find(|c| c.turns(block.at, &block.words[i]))
    };
    let mut pieces = Vec::new();
    let mut frame = None;
    let mut used: Vec<WordChange> = Vec::new();
    for i in paragraphs
        .clone()
        .flat_map(|p| paragraph_words(&block.paragraphs[p]).collect::<Vec<_>>())
    {
        let word = &block.words[i];
        let text = word.text.clone();
        match turned(i) {
            Some(change) => {
                frame = frame.or(word.output);
                if !used.contains(change) {
                    used.push(*change);
                }
                words::push(
                    &mut pieces,
                    if change.removes {
                        Piece::Removed(text)
                    } else {
                        Piece::Added(text)
                    },
                );
            }
            None if word.state == WordState::Kept => words::push(&mut pieces, Piece::Same(text)),
            None => {}
        }
    }
    let saved = apply(cut, &used, transcripts)
        .map(|after| kept(cut) as i64 - kept(&after) as i64)
        .unwrap_or_default();
    Card {
        proposal: id,
        at: block.at,
        frame: frame.or(block.paragraphs[paragraphs.start].start),
        paragraphs,
        pieces,
        saved,
        index: 0,
        count: 1,
    }
}

/// Word indices of a paragraph in reading order, removed words of its cuts included.
fn paragraph_words(paragraph: &Paragraph) -> impl Iterator<Item = usize> + '_ {
    paragraph.items.iter().flat_map(|item| match item {
        Item::Word(i) => *i..*i + 1,
        Item::Cut(run) => run.words.clone(),
    })
}

/// Kept media of the main cut in microseconds.
fn kept(cut: &Cut) -> u64 {
    model::blocks(cut)
        .flat_map(|(_, edit)| &edit.ranges)
        .filter_map(|r| match r {
            RangeEdit::Keep(range) => Some(range.duration().0),
            RangeEdit::Remove { .. } => None,
        })
        .sum()
}

fn midpoint(range: TimeRange) -> Micros {
    Micros((range.from().0 + range.to().0) / 2)
}

/// "saves 1.9 s", or "adds 0.4 s" for a proposal that brings back more than it cuts.
fn saved_text(saved: i64) -> String {
    let seconds = saved.unsigned_abs() as f64 / 1e6;
    match saved.signum() {
        1 => format!("saves {seconds:.1} s"),
        -1 => format!("adds {seconds:.1} s"),
        _ => String::new(),
    }
}

impl EditScreen {
    /// Rereads the open proposals from git and places them on the document.
    pub fn reload_proposals(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder.read(cx);
        self.proposals = match &self.channel {
            Some((repo, slug)) => match load(repo, slug, &folder.transcripts) {
                Ok(found) => found,
                Err(e) => {
                    self.error = Some(e);
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        self.cards = cards(
            &self.proposals,
            &self.doc,
            &folder.project.cut,
            &folder.transcripts,
        );
        if self
            .proposal
            .as_ref()
            .is_some_and(|name| !self.proposals.iter().any(|p| &p.name == name))
        {
            self.proposal = None;
        }
    }

    /// The card Tab, Esc and G act on first: the selected proposal's, else the first.
    pub fn active_card(&self) -> Option<&Card> {
        let selected = self
            .proposal
            .as_ref()
            .and_then(|name| self.proposals.iter().position(|p| &p.name == name));
        match selected {
            Some(id) => self.cards.iter().find(|c| c.proposal == id),
            None => self.cards.first(),
        }
    }

    fn select_proposal(&mut self, id: usize, cx: &mut Context<Self>) {
        self.proposal = self.proposals.get(id).map(|p| p.name.clone());
        self.selection = None;
        cx.notify();
    }

    /// Tab: accepts the selected proposal.
    pub fn accept_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(name) = self.proposal.clone() {
            self.accept_proposal(&name, cx);
        }
    }

    /// Esc: rejects the selected proposal. Returns false when none is selected.
    pub fn reject_selected(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(name) = self.proposal.clone() else {
            return false;
        };
        self.reject_proposal(&name, cx);
        true
    }

    /// Merges the proposal into the video branch and writes its cut to `cut.kdl`.
    fn accept_proposal(&mut self, name: &str, cx: &mut Context<Self>) {
        let (Some((repo, slug)), Some(proposal)) = (
            self.channel.clone(),
            self.proposals.iter().find(|p| p.name == name),
        ) else {
            return;
        };
        let folder = self.folder.read(cx);
        let result = proposal
            .applied_to(&folder.project.cut, &folder.transcripts)
            .and_then(|cut| {
                Board::open(&repo)
                    .and_then(|b| b.accept_proposal(&slug, name))
                    .map_err(|e| e.to_string())?;
                Ok(cut)
            })
            .and_then(|cut| {
                self.folder.update(cx, |folder, cx| {
                    folder.write_cut(cut)?;
                    cx.notify();
                    Ok(())
                })
            });
        self.error = result.err();
        self.proposal = None;
        self.reload_proposals(cx);
        cx.notify();
    }

    fn reject_proposal(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some((repo, slug)) = self.channel.clone() else {
            return;
        };
        if let Err(e) = Board::open(&repo).and_then(|b| b.reject_proposal(&slug, name)) {
            self.error = Some(e.to_string());
        }
        self.proposal = None;
        self.reload_proposals(cx);
        cx.notify();
    }

    /// Rejects the proposal and asks Claude for a different take on the same change.
    fn another_take(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some((repo, slug)) = self.channel.clone() else {
            return;
        };
        let Some(claude) = self.claude.clone() else {
            self.error = Some(
                "Another take needs Claude Code: install it and sign in with `claude`".to_owned(),
            );
            cx.notify();
            return;
        };
        if self.run.as_ref().is_some_and(AgentRun::running) {
            return;
        }
        let Some(diff) = self
            .proposals
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.describe(&self.folder.read(cx).transcripts))
        else {
            return;
        };
        self.reject_proposal(name, cx);
        let command = ScriptCommand::AnotherTake {
            proposal: name.to_owned(),
            diff,
        };
        match AgentRun::start(
            claude,
            &repo,
            "Another take".to_owned(),
            command.prompt(&slug),
        ) {
            Ok(run) => {
                self.run = Some(run);
                agent::watch(
                    cx,
                    |this: &mut Self| this.run.as_mut(),
                    |this, cx| {
                        if let Some(Err(e)) = this.run.as_ref().and_then(|r| r.outcome.clone()) {
                            this.error = Some(format!("Another take failed: {e}"));
                        }
                        this.reload_proposals(cx);
                    },
                );
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    /// G: shows the next proposal in the Text workspace, at its first change.
    pub fn show_next_proposal(&mut self, cx: &mut Context<Self>) {
        let next = match self.active_card() {
            Some(card) if self.workspace != Workspace::Text => Some(card.clone()),
            Some(card) => {
                let after = self
                    .cards
                    .iter()
                    .position(|c| c == card)
                    .map_or(0, |i| (i + 1) % self.cards.len());
                self.cards.get(after).cloned()
            }
            None => None,
        };
        if let Some(card) = next {
            self.show_card(&card, cx);
        }
    }

    fn show_card(&mut self, card: &Card, cx: &mut Context<Self>) {
        self.workspace = Workspace::Text;
        self.chapter = card.at.chapter;
        if let Some(frame) = card.frame {
            self.seek(frame);
        }
        self.scroll_to_card = true;
        self.select_proposal(card.proposal, cx);
    }
}

/// Unchanged words a card away from the transcript keeps on each side of a change.
const AWAY_CONTEXT: usize = 8;

/// The changed pieces with at most `context` unchanged words around each, the rest elided.
fn excerpt(pieces: &[Piece], context: usize) -> Vec<Piece> {
    let last = pieces.len().saturating_sub(1);
    pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            let Piece::Same(text) = piece else {
                return piece.clone();
            };
            let words: Vec<&str> = text.split_whitespace().collect();
            let (keep_start, keep_end) = (i > 0, i < last);
            if words.len() <= context * (keep_start as usize + keep_end as usize) {
                return piece.clone();
            }
            let mut out: Vec<&str> = Vec::new();
            if keep_start {
                out.extend(&words[..context]);
            }
            out.push("…");
            if keep_end {
                out.extend(&words[words.len() - context..]);
            }
            Piece::Same(out.join(" "))
        })
        .collect()
}

/// The 64ch measure of a card's text away from the transcript, at 16 px Geist.
const MEASURE: f32 = 64. * 8.2;

/// A proposal card: inline in the transcript, or `away` from it with where it applies.
pub fn card_element(
    this: &EditScreen,
    card: &Card,
    away: bool,
    cx: &mut Context<EditScreen>,
) -> Stateful<Div> {
    let proposal = &this.proposals[card.proposal];
    let name = proposal.name.clone();
    let selected = this.proposal.as_deref() == Some(name.as_str());
    let id = card.proposal;
    let key = format!("{}-{}-{}", name, card.at.chapter, card.paragraphs.start);
    let where_ = this
        .map
        .chapters
        .get(card.at.chapter)
        .map(|c| c.title.clone())
        .zip(card.frame.map(timecode))
        .map(|(title, at)| format!("{title}, {at}"));
    let (size, line) = if away { (16., 28.) } else { (18., 32.) };
    let pieces = if away {
        excerpt(&card.pieces, AWAY_CONTEXT)
    } else {
        card.pieces.clone()
    };
    let words = pieces.into_iter().flat_map(|piece| {
        let (text, kind) = match piece {
            Piece::Same(t) => (t, 0),
            Piece::Removed(t) => (t, 1),
            Piece::Added(t) => (t, 2),
        };
        let words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
        words.into_iter().map(move |w| {
            div()
                .pr(px(5.))
                .rounded(px(3.))
                .when(kind == 0, |d| d.text_color(BODY))
                .when(kind == 1, |d| {
                    d.text_color(gpui::rgb(0xb98b87))
                        .line_through()
                        .text_decoration_color(CUT)
                        .bg(Hsla::from(CUT).opacity(0.08))
                })
                .when(kind == 2, |d| {
                    d.text_color(STRONG).bg(Hsla::from(ACCENT).opacity(0.12))
                })
                .child(w)
        })
    });
    let running = this.run.as_ref().is_some_and(AgentRun::running);
    let playing = this.previewing.as_deref() == Some(name.as_str());
    div()
        .id(SharedString::from(format!("proposal-{key}")))
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(10.))
        .px(px(16.))
        .py(px(14.))
        .rounded(px(12.))
        .bg(RAISED)
        .border_1()
        .border_color(if selected {
            Hsla::from(ACCENT).opacity(0.5)
        } else {
            Hsla::from(RAISED)
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.select_proposal(id, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(14.))
                .min_w_0()
                .child(icon("spark", ICON, ACCENT))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TEXT)
                        .child(proposal.subject.clone()),
                )
                .when(away, |row| {
                    row.children(
                        where_
                            .clone()
                            .map(|w| div().flex_none().text_color(MUTED).child(w)),
                    )
                })
                .when(card.count > 1, |row| {
                    row.child(div().text_color(FAINT).child(format!(
                        "{} of {}, accepted together",
                        card.index + 1,
                        card.count
                    )))
                })
                .when(away, |row| row.child(div().flex_1()))
                .child(
                    div()
                        .flex_none()
                        .text_color(MUTED)
                        .child(saved_text(card.saved)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .text_size(px(size))
                .line_height(px(line))
                .when(away, |d| d.max_w(px(MEASURE)))
                .children(words),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(
                    proposal_action(
                        format!("accept-{key}"),
                        "Accept",
                        Some("check"),
                        Some("Tab"),
                        ActionLook::Accent,
                    )
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.accept_proposal(&name, cx)
                    })),
                )
                .child(
                    proposal_action(
                        format!("reject-{key}"),
                        "Reject",
                        Some("close"),
                        (!playing).then_some("Esc"),
                        ActionLook::Plain,
                    )
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.reject_proposal(&name, cx)
                    })),
                )
                .child(if playing {
                    proposal_action(
                        format!("play-{key}"),
                        "Back to the cut",
                        Some("pause"),
                        Some("Esc"),
                        ActionLook::Plain,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stop_proposal(cx);
                    }))
                } else {
                    proposal_action(
                        format!("play-{key}"),
                        "Play",
                        Some("play"),
                        None,
                        ActionLook::Plain,
                    )
                    .on_click(cx.listener({
                        let (name, card) = (name.clone(), card.clone());
                        move |this, _, _, cx| this.play_proposal(&name, &card, cx)
                    }))
                })
                .child(
                    proposal_action(
                        format!("again-{key}"),
                        if running {
                            "Claude is busy"
                        } else {
                            "Another take"
                        },
                        Some("again"),
                        None,
                        ActionLook::Quiet,
                    )
                    .on_click(cx.listener({
                        let name = name.clone();
                        move |this, _, _, cx| this.another_take(&name, cx)
                    })),
                )
                .when(away, |row| {
                    let card = card.clone();
                    row.child(div().flex_1()).child(
                        div()
                            .id(SharedString::from(format!("show-{key}")))
                            .h(px(30.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(9.))
                            .text_size(px(14.))
                            .text_color(MUTED)
                            .cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .child(format!(
                                "Show at {}",
                                card.frame.map(timecode).unwrap_or_default()
                            ))
                            .child(kbd("G"))
                            .on_click(cx.listener(move |this, _, _, cx| this.show_card(&card, cx))),
                    )
                }),
        )
}

/// The cut and lock of the video in `dir` as proposal `name` has them: its files on top of the
/// video's. A migrated video's lock is built from them like its own; one without a lock plays
/// the proposal's `cut.kdl` alone.
fn proposal_version(
    repo: &Path,
    slug: &str,
    name: &str,
    dir: &Path,
    current: &Cut,
    migrated: bool,
) -> Result<(Cut, Option<Lock>), String> {
    let files = Board::open(repo)
        .and_then(|board| board.proposal_files(slug, name))
        .map_err(|e| e.to_string())?;
    if !migrated {
        let path = style::video_path(slug, "cut.kdl");
        let cut = match files.get(&path) {
            Some(text) => Cut::parse(text).map_err(|e| format!("{path}:{e}"))?,
            None => current.clone(),
        };
        return Ok((cut, None));
    }
    let (disk, slug) = Disk::locate(dir)?;
    let built = style::build_video(
        &Patched {
            inner: &disk,
            files: &files,
        },
        &slug,
    )?;
    Ok((built.cut, Some(built.lock)))
}

/// How far before its first change a proposal's preview starts.
const LEAD_IN: Micros = Micros(2_000_000);

impl EditScreen {
    /// Plays proposal `name` in the preview, from a little before the first change of `card`.
    fn play_proposal(&mut self, name: &str, card: &Card, cx: &mut Context<Self>) {
        let Some((repo, slug)) = self.channel.clone() else {
            return;
        };
        let folder = self.folder.read(cx);
        let dir = folder.dir.clone();
        let built = proposal_version(
            &repo,
            &slug,
            name,
            &dir,
            &folder.project.cut,
            folder.lock().is_some(),
        )
        .and_then(|(cut, lock)| {
            let project = Project {
                cut,
                ..folder.project.clone()
            };
            let shown = match &lock {
                Some(lock) => crate::style::shown(&project.cut, lock),
                None => project.cut.clone(),
            };
            let fail = |e: shot_render::export::ExportError| e.to_string();
            let plan = shot_render::export::preview_plan_with(&dir, &project, lock.as_ref())
                .map_err(fail)?;
            let theme = shot_render::export::theme(&dir, lock.as_ref(), &project)
                .map_err(|e| e.to_string())?;
            let map = EditMap::new(&shown, plan);
            if map.spans.len() != map.plan.segments.len() {
                return Err("the proposal's preview could not place its segments".to_owned());
            }
            Ok((project, lock, map, theme))
        });
        let (project, lock, map, theme) = match built {
            Ok(built) => built,
            Err(e) => {
                self.error = Some(format!("the proposal does not play: {e}"));
                cx.notify();
                return;
            }
        };
        // The same media a little before the change, found on the branch's map, then on the
        // proposal's.
        let from = card
            .frame
            .map(|f| f.saturating_sub(LEAD_IN.0 * u64::from(FPS) / 1_000_000))
            .and_then(|f| self.map.source_at(f))
            .and_then(|(at, time)| map.output_frame(at, time))
            .unwrap_or_default();
        let folder = self.folder.read(cx);
        self.doc = model::document(
            &folder.shown.cut,
            &folder.transcripts,
            &map,
            self.view,
            folder.shown.format.default_layout,
        );
        let effects = project.format.effects();
        self.sound
            .set_plan_with(&map, dir.clone(), project, lock, from);
        self.engine.set_plan(
            Plan {
                map: map.clone(),
                dir,
                effects,
                theme: Ok(Arc::new(theme)),
            },
            from,
        );
        self.map = map;
        self.previewing = Some(name.to_owned());
        self.proposal = Some(name.to_owned());
        self.seek(from);
        self.set_playing(true);
        cx.notify();
    }

    /// Esc while a proposal plays: back to the branch's plan. Returns false when none plays.
    pub fn stop_proposal(&mut self, cx: &mut Context<Self>) -> bool {
        if self.previewing.take().is_none() {
            return false;
        }
        self.set_playing(false);
        self.rebuild(cx);
        true
    }
}

/// While Claude writes another take: a quiet line with its latest step.
pub fn agent_note(this: &EditScreen) -> Option<Div> {
    let run = this.run.as_ref().filter(|r| r.running())?;
    let step = run
        .lines()
        .last()
        .map_or_else(|| "Starting".to_owned(), |(line, _)| line.clone());
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(RAISED)
            .text_size(px(14.))
            .child(icon("spark", ICON, ACCENT))
            .child(
                div()
                    .text_color(TEXT)
                    .child("Claude is writing another take"),
            )
            .child(div().text_color(MUTED).child(step)),
    )
}

/// The lime sparks over the overview bar, one per card, clicking one shows it.
pub fn sparks(this: &EditScreen, cx: &mut Context<EditScreen>) -> Vec<Stateful<Div>> {
    let total = this.map.frames;
    this.cards
        .iter()
        .enumerate()
        .filter_map(|(i, card)| {
            let frame = card.frame?;
            let card = card.clone();
            Some(
                div()
                    .id(("spark", i))
                    .absolute()
                    .top(px(-15.))
                    .left(gpui::relative(model::fraction(frame, total)))
                    .ml(px(-7.))
                    .size(px(14.))
                    .child(icon("spark", ICON_SM, ACCENT))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.show_card(&card, cx);
                        }),
                    ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EditMap, View, document};
    use shot_core::{Layout, Word};

    fn transcripts() -> HashMap<Id, Transcript> {
        let id = Id::new("t01").unwrap();
        let mut t = Transcript::new(id.clone());
        for (i, text) in ["let", "me", "let", "me", "scroll", "I", "read", "it"]
            .iter()
            .enumerate()
        {
            let from = i as u64 * 1_000_000;
            t.push(Word {
                text: (*text).to_owned(),
                range: TimeRange::new(Micros(from), Micros(from + 800_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
        }
        HashMap::from([(id, t)])
    }

    const BASE: &str = "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=2_000_000\n        remove from=2_000_000 to=4_000_000 by=filler\n        keep from=4_000_000 to=8_000_000\n    }\n}\n";

    fn at() -> TakeAt {
        TakeAt {
            chapter: 0,
            take: 0,
        }
    }

    fn span(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    /// The proposal: cut "scroll", bring back the second "let me".
    fn head(base: &Cut, t: &HashMap<Id, Transcript>) -> Cut {
        let t01 = &t[&Id::new("t01").unwrap()];
        base.delete_words(at(), t01, span(4_000_000, 4_800_000))
            .unwrap()
            .restore(at(), span(2_000_000, 3_800_000))
            .unwrap()
    }

    #[test]
    fn reads_removed_and_restored_word_spans_between_two_cuts() {
        let t = transcripts();
        let base = Cut::parse(BASE).unwrap();
        let changes = word_changes(&base, &head(&base, &t), &t);
        assert_eq!(
            changes,
            [
                WordChange {
                    at: at(),
                    removes: false,
                    range: span(2_000_000, 3_800_000),
                },
                WordChange {
                    at: at(),
                    removes: true,
                    range: span(4_000_000, 4_800_000),
                },
            ]
        );
    }

    #[test]
    fn applies_on_top_of_other_edits_and_takes_the_head_when_nothing_moved() {
        let t = transcripts();
        let base = Cut::parse(BASE).unwrap();
        let head = head(&base, &t);
        let proposal = CutProposal {
            name: "tighter".into(),
            subject: "Tighter".into(),
            changes: word_changes(&base, &head, &t),
            base: Some(base.clone()),
            head: head.clone(),
        };
        assert_eq!(proposal.applied_to(&base, &t).unwrap(), head);

        let t01 = &t[&Id::new("t01").unwrap()];
        let edited = base
            .delete_words(at(), t01, span(7_000_000, 7_800_000))
            .unwrap();
        let expected = head
            .delete_words(at(), t01, span(7_000_000, 7_800_000))
            .unwrap();
        assert_eq!(proposal.applied_to(&edited, &t).unwrap(), expected);
    }

    #[test]
    fn away_from_the_text_a_card_keeps_the_words_around_its_changes() {
        let pieces = [
            Piece::Same("one two three four".into()),
            Piece::Removed("cut".into()),
            Piece::Same("five six seven".into()),
        ];
        assert_eq!(
            excerpt(&pieces, 2),
            [
                Piece::Same("… three four".into()),
                Piece::Removed("cut".into()),
                Piece::Same("five six …".into()),
            ]
        );
    }

    #[test]
    fn a_card_strikes_and_inserts_words_with_the_time_saved() {
        let t = transcripts();
        let base = Cut::parse(BASE).unwrap();
        let head = head(&base, &t);
        let proposal = CutProposal {
            name: "tighter".into(),
            subject: "Tighter".into(),
            changes: word_changes(&base, &head, &t),
            base: Some(base.clone()),
            head,
        };
        let project = shot_core::Project {
            video: shot_core::Video::parse("title \"A\"\nstatus editing\nformat opinion\n")
                .unwrap(),
            script: shot_core::Script::parse("# A\n").unwrap(),
            format: shot_core::FormatKind::Opinion.preset(),
            cut: base.clone(),
        };
        let map = EditMap::of(&project, View::Edited);
        let doc = document(&base, &t, &map, View::Edited, Layout::Face);
        let found = cards(&[proposal], &doc, &base, &t);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].pieces,
            [
                Piece::Same("let me".into()),
                Piece::Added("let me".into()),
                Piece::Removed("scroll".into()),
                Piece::Same("I read it".into()),
            ]
        );
        // Brings back 1.8 s, cuts 0.8 s.
        assert_eq!(found[0].saved, -1_000_000);
        assert_eq!(saved_text(1_900_000), "saves 1.9 s");
    }
}
