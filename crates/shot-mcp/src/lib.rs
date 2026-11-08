//! The `shot` MCP server: the channel board for outside agents and the in-app `claude` bridge,
//! over stdio. Its tools are the AI contract ([0003]).
//!
//! Every tool returns text ([0005]): video files, transcripts, the review queue, caption fixes,
//! script checks and the upload description. Changes are proposals, branches a person accepts
//! or rejects; accepting is the human's review action, so it is deliberately not a tool. A cut
//! proposal only removes or restores recorded words. The one tool that returns images,
//! `thumbnail_frames`, is a frame task: it runs only in a server started with
//! [`ServeOptions::frame_task`], which the app sets when a person starts a thumbnail pick.
//!
//! [0003]: ../../docs/decisions/0003-claude-through-local-cli.md
//! [0005]: ../../docs/decisions/0005-privacy-text-only-to-claude.md

pub mod agents;
pub mod bridge;
pub mod style;
mod tee;
pub mod text;
pub mod transcript;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::path::PathBuf;

use base64::Engine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use shot_board::Board;
use shot_core::{
    Cut, Decision, ElementKind, Format, Id, Micros, Project, Script, Selector, Transcript, Video,
};

use crate::transcript::WordSpan;

/// The files of a video folder that `read_video` returns.
const VIDEO_FILES: [&str; 4] = ["video.kdl", "script.md", "cut.kdl", "format.kdl"];
const DEFAULT_ACTIVITY_LIMIT: usize = 20;
/// The most elements or instances one style tool prints.
const STYLE_LIMIT: usize = 300;

/// How a server session runs.
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// Allows the frame task tools. Only set for a session a person started on purpose.
    pub frame_task: bool,
    /// Appends every byte the server writes to the client to this file, so a session's data
    /// flow can be audited.
    pub log: Option<PathBuf>,
}

/// Serves the channel repo at `repo` on stdin and stdout until the client disconnects.
pub async fn serve_stdio(
    repo: PathBuf,
    options: ServeOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = ShotServer::new(repo).with_frame_task(options.frame_task);
    let stdout = tee::Tee::new(tokio::io::stdout(), options.log.as_deref())?;
    server
        .serve((tokio::io::stdin(), stdout))
        .await?
        .waiting()
        .await?;
    Ok(())
}

/// One MCP session over one channel repo. The repo is opened per call, so nothing is cached and
/// changes made by the app or by git in between are always seen. Transcripts are read from the
/// cache in the repo's working tree, `videos/<slug>/.shot/cache`.
#[derive(Clone)]
pub struct ShotServer {
    repo: PathBuf,
    frame_task: bool,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListVideosArgs {
    /// Only videos in this column: ideas, scripting, ready, recorded, editing or shipped.
    status: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct VideoArgs {
    /// The video slug, for example `why-we-deploy-on-fridays`.
    slug: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct MoveCardArgs {
    /// The video slug.
    slug: String,
    /// The target column: ideas, scripting, ready, recorded, editing or shipped.
    status: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ActivityArgs {
    /// Maximum number of commits, newest first. Defaults to 20.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListProposalsArgs {
    /// Only proposals of this video.
    slug: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CreateProposalArgs {
    /// The video the proposal changes.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes, for example `tighter-intro`.
    name: String,
    /// The commit subject: what the proposal does, in one line.
    subject: String,
    /// New contents by path relative to the video folder, for example `{"script.md": "..."}`.
    files: BTreeMap<String, String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposalArgs {
    /// The video slug.
    slug: String,
    /// The proposal name.
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeCutArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes, for example `tighter-intro`.
    name: String,
    /// The commit subject: what the proposal does, in one line.
    subject: String,
    /// Spans of recorded words to cut, numbered as `read_transcript` shows them.
    #[serde(default)]
    remove: Vec<WordSpan>,
    /// Spans of removed words to bring back.
    #[serde(default)]
    restore: Vec<WordSpan>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ReviewDecision {
    /// The item number from `review_queue`.
    item: usize,
    /// `keep` or `cut`.
    decision: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeReviewArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes.
    name: String,
    /// The commit subject.
    subject: String,
    /// One decision per review item you are sure about.
    decisions: Vec<ReviewDecision>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeScriptArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes, for example `hook-a`.
    name: String,
    /// The commit subject.
    subject: String,
    /// The whole new script.md: `# title`, `## chapter`, `- beat` with `[label](source)` links.
    script: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ReadElementsArgs {
    /// The video slug.
    slug: String,
    /// The take id, for example `t01`.
    take: String,
    /// Only these element kinds, for example `["click", "filler"]`. Default: all but words.
    #[serde(default)]
    kinds: Vec<String>,
    /// Window start in the take's media time, microseconds.
    from_us: Option<u64>,
    /// Window end in microseconds.
    to_us: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ReadLockArgs {
    /// The video slug.
    slug: String,
    /// Only this take.
    take: Option<String>,
    /// Window start in the take's media time, microseconds.
    from_us: Option<u64>,
    /// Window end in microseconds.
    to_us: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ExplainArgs {
    /// The video slug.
    slug: String,
    /// The take id.
    take: String,
    /// The time to explain, in the take's media time, microseconds.
    at_us: u64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RuleScopeArgs {
    /// The rule's selector as the lock writes it, for example `silence min=500_000`.
    selector: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeRuleArgs {
    /// The video the proposal branches off. A `format` or `brand` rule applies to every
    /// unshipped video of the channel once accepted.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes.
    name: String,
    /// The commit subject: what the proposal does, in one line.
    subject: String,
    /// Where the rule lives: `video` (this video's format.kdl), `format` (the channel's format
    /// of this video) or `brand` (brand/brand.kdl).
    layer: String,
    /// One rule as KDL, for example `on silence min=500_000 { cut breath=250_000 }`. A rule
    /// with the same selector in that layer is replaced; `on <selector> off` switches the
    /// selector off for the layers below.
    rule: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeOverrideArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes.
    name: String,
    /// The commit subject.
    subject: String,
    /// The take id.
    take: String,
    /// The element kind, for example `click`.
    kind: String,
    /// The element's start `t` in microseconds, as read_elements shows it.
    at_us: u64,
    /// The override's children as KDL, one per line: `keep`, `cut`, an effect such as
    /// `zoom 140 hold=1_000_000`, or `zoom off`.
    effects: String,
}

#[tool_router]
impl ShotServer {
    pub fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            frame_task: false,
            tool_router: Self::tool_router(),
        }
    }

    /// Allows the frame task tools in this session.
    pub fn with_frame_task(mut self, allowed: bool) -> Self {
        self.frame_task = allowed;
        self
    }

    #[tool(
        description = "List the videos of the channel by board column with title, format and target date. Videos whose video.kdl cannot be parsed are listed separately with file:line:column."
    )]
    fn list_videos(&self, Parameters(args): Parameters<ListVideosArgs>) -> Result<String, String> {
        let only = args.status.as_deref().map(text::parse_status).transpose()?;
        let mut cards = self.board()?.cards().map_err(|e| e.to_string())?;
        if let Some(status) = only {
            cards.retain(|c| c.video.as_ref().is_ok_and(|v| v.status == status));
        }
        Ok(text::board(&cards))
    }

    #[tool(
        description = "Read the text of one video from its branch tip: video.kdl, script.md, cut.kdl and format.kdl. Files that do not exist yet are marked as missing."
    )]
    fn read_video(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        self.read_video_files(&args.slug)
    }

    #[tool(
        description = "Move a video to another board column. This is one commit on the video's branch."
    )]
    fn move_card(&self, Parameters(args): Parameters<MoveCardArgs>) -> Result<String, String> {
        let status = text::parse_status(&args.status)?;
        let moved = self
            .board()?
            .move_card(&args.slug, status)
            .map_err(|e| e.to_string())?;
        Ok(match moved {
            Some(sha) => format!("moved {} to {status} in {}\n", args.slug, short(&sha)),
            None => format!("{} is already in {status}\n", args.slug),
        })
    }

    #[tool(
        description = "The newest commits across all video branches, newest first, with time, author and subject."
    )]
    fn activity(&self, Parameters(args): Parameters<ActivityArgs>) -> Result<String, String> {
        let limit = args.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT);
        let entries = self.board()?.activity(limit).map_err(|e| e.to_string())?;
        Ok(text::activity(&entries))
    }

    #[tool(description = "List open proposals, optionally of one video.")]
    fn list_proposals(
        &self,
        Parameters(args): Parameters<ListProposalsArgs>,
    ) -> Result<String, String> {
        let found = self
            .board()?
            .proposals(args.slug.as_deref())
            .map_err(|e| e.to_string())?;
        Ok(text::proposals(&found))
    }

    #[tool(
        description = "Propose changes to a video's text files as a branch with one commit. Every known project file must parse. Nothing changes on the video until the human accepts the proposal in the app or with `shot proposal accept`. Prefer propose_cut for cuts and propose_script for the script."
    )]
    fn create_proposal(
        &self,
        Parameters(args): Parameters<CreateProposalArgs>,
    ) -> Result<String, String> {
        for (file, text) in &args.files {
            validate(file, text)?;
        }
        let proposal = self
            .board()?
            .create_proposal(&args.slug, &args.name, &args.subject, &args.files)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "created proposal {}/{} at {}\n",
            proposal.slug,
            proposal.name,
            short(&proposal.head)
        ))
    }

    #[tool(
        description = "The unified diff of a proposal against the point where it branched off the video."
    )]
    fn proposal_diff(&self, Parameters(args): Parameters<ProposalArgs>) -> Result<String, String> {
        let files = self
            .board()?
            .proposal_diff(&args.slug, &args.name)
            .map_err(|e| e.to_string())?;
        Ok(text::diffs(&files))
    }

    #[tool(
        description = "The transcript of a video in cut order. Each line starts with the edited time of its first kept word and the number of its first word in the take (w12); count on from there. Removed words are in brackets with what removed them: [um so](filler)."
    )]
    fn read_transcript(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        Ok(transcript::render(&cut, &transcripts))
    }

    #[tool(
        description = "Propose a cut as a branch: remove spans of recorded words, or restore removed ones, by word number from read_transcript. A cut can only drop or bring back what was recorded, never add words. Replies with the words each span covers, so check them."
    )]
    fn propose_cut(&self, Parameters(args): Parameters<ProposeCutArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        let (cut, changes) = transcript::apply(&cut, &transcripts, &args.remove, &args.restore)?;
        let created = self.propose(
            &args.slug,
            &args.name,
            &args.subject,
            "cut.kdl",
            cut.to_kdl(),
        )?;
        let mut out = created;
        for change in changes {
            let _ = writeln!(out, "  {change}");
        }
        Ok(out)
    }

    #[tool(
        description = "The open review queue of a video: findings an auto-cut rule was not sure about, numbered, with their edited time, rule and words."
    )]
    fn review_queue(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        let mut out = String::new();
        for (index, (at, review)) in cut.review_queue().enumerate() {
            let take = &cut.chapters[at.chapter].takes[at.take].take;
            let _ = writeln!(
                out,
                "#{}  {}  {}  {:.1} s  \"{}\"",
                index + 1,
                take,
                review.by.as_str(),
                review.range.duration().0 as f64 / 1e6,
                transcript::words_in(transcripts.get(take), review.range)
            );
        }
        if out.is_empty() {
            out.push_str("the review queue is empty\n");
        }
        Ok(out)
    }

    #[tool(
        description = "Propose answers to review items as a branch: keep leaves the material in and tells the rule to leave it alone, cut removes it. Only answer items you are sure about."
    )]
    fn propose_review(
        &self,
        Parameters(args): Parameters<ProposeReviewArgs>,
    ) -> Result<String, String> {
        let mut cut = self.cut(&args.slug)?;
        let queue: Vec<_> = cut.review_queue().collect();
        for decision in &args.decisions {
            let (at, review) = decision
                .item
                .checked_sub(1)
                .and_then(|i| queue.get(i))
                .ok_or_else(|| format!("there is no review item #{}", decision.item))?;
            let answer = match decision.decision.as_str() {
                "keep" => Decision::Keep,
                "cut" => Decision::Cut,
                other => return Err(format!("`{other}` is not keep or cut")),
            };
            cut = cut
                .decide(*at, review.range, answer)
                .map_err(|e| e.to_string())?;
        }
        self.propose(
            &args.slug,
            &args.name,
            &args.subject,
            "cut.kdl",
            cut.to_kdl(),
        )
    }

    #[tool(
        description = "The caption fixes dictionary of a video and the words recognized with low confidence that no fix covers yet. Suggest new fixes in your reply as heard -> said; a person adds them in Caption fixes. Fixes are never proposals."
    )]
    fn caption_fixes(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        let mut out = String::from("fixes\n");
        if cut.caption_fixes.is_empty() {
            out.push_str("  (none)\n");
        }
        for fix in &cut.caption_fixes {
            let _ = writeln!(out, "  \"{}\" -> \"{}\"", fix.heard, fix.said);
        }
        let project = self.project(&args.slug, cut)?;
        let main = &shot_core::Timeline::evaluate(&project).outputs[0];
        let words = shot_ship::spoken::spoken_words(main, &transcripts);
        let unsure =
            shot_ship::check::captions(main.captions, true, None, &main.caption_fixes, &words, &[]);
        out.push_str("unsure words\n");
        match unsure.first() {
            Some(problem) => {
                let _ = writeln!(out, "  {}", problem.message);
            }
            None => out.push_str("  (none)\n"),
        }
        Ok(out)
    }

    #[tool(
        description = "Checks on a video's script.md: estimated spoken length per chapter, hook length, beats without a source and long stretches without a visual."
    )]
    fn script_checks(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let script = self.script(&args.slug)?;
        let report = shot_ship::script_check::check_script(&script);
        let mut out = format!("spoken about {}\n", shot_ship::clock(report.total));
        for (chapter, length) in script.chapters.iter().zip(&report.chapters) {
            let _ = writeln!(out, "  {}  {}", shot_ship::clock(*length), chapter.title);
        }
        if report.findings.is_empty() {
            out.push_str("all script checks pass\n");
        }
        for finding in &report.findings {
            let _ = writeln!(out, "{finding}");
        }
        Ok(out)
    }

    #[tool(
        description = "Propose a new script.md as a branch. The script must follow the script.md shape. Name alternatives you offer side by side alike, for example hook-a, hook-b and hook-c; the person accepts one."
    )]
    fn propose_script(
        &self,
        Parameters(args): Parameters<ProposeScriptArgs>,
    ) -> Result<String, String> {
        validate("script.md", &args.script)?;
        self.propose(
            &args.slug,
            &args.name,
            &args.subject,
            "script.md",
            args.script,
        )
    }

    #[tool(
        description = "A draft title and description for the upload: the hook, chapters at the moment their first word is heard, and the links the video shows or the script cites."
    )]
    fn describe(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        let project = self.project(&args.slug, cut)?;
        let timeline = shot_core::Timeline::evaluate(&project);
        let description = shot_ship::describe::describe(&project, timeline.main(), &transcripts);
        Ok(format!(
            "title: {}\n\n{}",
            description.title, description.text
        ))
    }

    #[tool(
        description = "The elements (facts) of one take as JSON lines, in take media time: clicks, silences, fillers, urls and the rest, with their t, to, confidence and source. Words are left out unless asked for. Use their t as the identity of an override."
    )]
    fn read_elements(
        &self,
        Parameters(args): Parameters<ReadElementsArgs>,
    ) -> Result<String, String> {
        let take = take_id(&args.take)?;
        let mut kinds = args
            .kinds
            .iter()
            .map(|k| ElementKind::parse(k).ok_or_else(|| format!("`{k}` is not an element kind")))
            .collect::<Result<Vec<_>, _>>()?;
        if kinds.is_empty() {
            kinds = ElementKind::ALL
                .iter()
                .copied()
                .filter(|k| *k != ElementKind::Word)
                .collect();
        }
        let board = self.board()?;
        let built = style::build_video(&style::Git(&board), &args.slug)?;
        let elements = built
            .elements
            .iter()
            .find(|e| *e.take() == take)
            .ok_or_else(|| {
                format!(
                    "take {take} has no elements in the main cut of {}",
                    args.slug
                )
            })?;
        let window = style::window(args.from_us, args.to_us)?;
        Ok(style::elements_text(elements, &kinds, window, STYLE_LIMIT))
    }

    #[tool(
        description = "The lock of a video, built from its files: every instance (cut, review, zoom, overlay, sfx, transition, layout, captions) with its resolved values and its provenance, `on=` the element, `layer=` and the `rule=` or `override=` that made it. Narrow it by take and time window."
    )]
    fn read_lock(&self, Parameters(args): Parameters<ReadLockArgs>) -> Result<String, String> {
        let take = args.take.as_deref().map(take_id).transpose()?;
        let board = self.board()?;
        let built = style::build_video(&style::Git(&board), &args.slug)?;
        let window = style::window(args.from_us, args.to_us)?;
        Ok(style::lock_text(
            &built.lock,
            take.as_ref(),
            window,
            STYLE_LIMIT,
        ))
    }

    #[tool(
        description = "Why a moment looks the way it does: the instances at a time in a take with the layer and rule or override behind each, and the overrides in cut.kdl there (an `off` leaves no instance)."
    )]
    fn explain_instance(
        &self,
        Parameters(args): Parameters<ExplainArgs>,
    ) -> Result<String, String> {
        let take = take_id(&args.take)?;
        let board = self.board()?;
        let built = style::build_video(&style::Git(&board), &args.slug)?;
        Ok(style::explain(&built, Some(&take), Micros(args.at_us)))
    }

    #[tool(
        description = "How far a rule reaches: the instances and elements the rule with this selector produces now, in how many of the channel's unshipped videos. Shipped videos keep their lock."
    )]
    fn rule_scope(&self, Parameters(args): Parameters<RuleScopeArgs>) -> Result<String, String> {
        let selector = Selector::parse(&args.selector).map_err(|e| e.to_string())?;
        let board = self.board()?;
        let reach = style::reach(&style::Git(&board), &selector)?;
        Ok(format!("on {selector}\n{reach}"))
    }

    #[tool(
        description = "Propose a change of one style rule as a branch, at the layer you choose: this video's format.kdl, the channel's format or the brand. The reply and the proposal's description state the scope: how many instances change in how many videos. Pick the narrowest layer that fits the person's intent."
    )]
    fn propose_rule(
        &self,
        Parameters(args): Parameters<ProposeRuleArgs>,
    ) -> Result<String, String> {
        let layer = style::RuleLayer::parse(&args.layer)?;
        let board = self.board()?;
        let repo = style::Git(&board);
        let files = style::rule_change(&repo, &args.slug, layer, &args.rule)?;
        self.propose_style(&board, &args.slug, &args.name, &args.subject, files)
    }

    #[tool(
        description = "Propose an override for one element as a branch: what this single element gets instead of what the rules gave it, written to cut.kdl. The element must exist (read_elements). The reply states the instances it changes."
    )]
    fn propose_override(
        &self,
        Parameters(args): Parameters<ProposeOverrideArgs>,
    ) -> Result<String, String> {
        let take = take_id(&args.take)?;
        let kind = ElementKind::parse(&args.kind)
            .ok_or_else(|| format!("`{}` is not an element kind", args.kind))?;
        let board = self.board()?;
        let repo = style::Git(&board);
        let files = style::override_change(
            &repo,
            &args.slug,
            &take,
            kind,
            Micros(args.at_us),
            &args.effects,
        )?;
        self.propose_style(&board, &args.slug, &args.name, &args.subject, files)
    }

    #[tool(
        description = "Frame task: the thumbnail candidates of a video as still images, best first. Runs only when the person started a thumbnail pick in the app; otherwise it refuses."
    )]
    fn thumbnail_frames(
        &self,
        Parameters(args): Parameters<VideoArgs>,
    ) -> Result<CallToolResult, String> {
        if !self.frame_task {
            return Err(
                "thumbnail_frames is a frame task: still frames go to Claude only when \
                        the person starts a thumbnail pick in Shot Studio"
                    .to_owned(),
            );
        }
        Board::open(&self.repo).map_err(|e| e.to_string())?;
        shot_board_check_name(&args.slug)?;
        let dir = self.video_dir(&args.slug).join(".shot/cache/thumbnails");
        let mut stills: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|_| {
                format!(
                    "no thumbnail candidates yet: run `shot thumbnails` on {}",
                    args.slug
                )
            })?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        stills.sort();
        let mut content = Vec::new();
        for still in stills {
            let bytes = std::fs::read(&still).map_err(|e| format!("{}: {e}", still.display()))?;
            let name = still.file_name().unwrap_or_default().to_string_lossy();
            content.push(ContentBlock::text(format!("candidate {name}")));
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ));
        }
        Ok(CallToolResult::success(content))
    }

    fn board(&self) -> Result<Board, String> {
        Board::open(&self.repo)
            .map_err(|e| format!("{} is not a channel repo: {e}", self.repo.display()))
    }

    fn video_dir(&self, slug: &str) -> PathBuf {
        self.repo.join("videos").join(slug)
    }

    /// The cut at the video's tip, empty when it has none yet.
    fn cut(&self, slug: &str) -> Result<Cut, String> {
        match self
            .board()?
            .read_file(slug, "cut.kdl")
            .map_err(|e| e.to_string())?
        {
            Some(text) => Cut::parse(&text).map_err(|e| e.to_string()),
            None => Ok(Cut::default()),
        }
    }

    fn script(&self, slug: &str) -> Result<Script, String> {
        let text = self
            .board()?
            .read_file(slug, "script.md")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{slug} has no script.md yet"))?;
        Script::parse(&text).map_err(|e| e.to_string())
    }

    fn project(&self, slug: &str, cut: Cut) -> Result<Project, String> {
        let board = self.board()?;
        let read = |file: &str| -> Result<String, String> {
            board
                .read_file(slug, file)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{slug} has no {file} yet"))
        };
        Ok(Project {
            video: Video::parse(&read("video.kdl")?).map_err(|e| e.to_string())?,
            script: Script::parse(&read("script.md")?).map_err(|e| e.to_string())?,
            format: Format::parse(&read("format.kdl")?).map_err(|e| e.to_string())?,
            cut,
        })
    }

    /// The cached transcripts of the takes `cut` uses, from the working tree.
    fn transcripts(&self, slug: &str, cut: &Cut) -> Result<HashMap<Id, Transcript>, String> {
        shot_board_check_name(slug)?;
        let dir = self.video_dir(slug);
        let mut found = HashMap::new();
        for take in cut.chapters.iter().flat_map(|c| &c.takes).map(|t| &t.take) {
            if found.contains_key(take) {
                continue;
            }
            if let Some(transcript) =
                shot_analyze::cache::read_cached(&dir, take).map_err(|e| e.to_string())?
            {
                found.insert(take.clone(), transcript);
            }
        }
        Ok(found)
    }

    fn propose(
        &self,
        slug: &str,
        name: &str,
        subject: &str,
        file: &str,
        text: String,
    ) -> Result<String, String> {
        let files = BTreeMap::from([(file.to_owned(), text)]);
        let proposal = self
            .board()?
            .create_proposal(slug, name, subject, &files)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "created proposal {}/{} at {}\n",
            proposal.slug,
            proposal.name,
            short(&proposal.head)
        ))
    }

    /// Proposes repo-relative `files` and states in the commit what they reach.
    fn propose_style(
        &self,
        board: &Board,
        slug: &str,
        name: &str,
        subject: &str,
        files: BTreeMap<String, String>,
    ) -> Result<String, String> {
        let impact = style::impact(&style::Git(board), slug, &files)?;
        let message = format!("{subject}\n\n{impact}");
        let proposal = board
            .create_repo_proposal(slug, name, &message, &files)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "created proposal {}/{} at {}\n{impact}",
            proposal.slug,
            proposal.name,
            short(&proposal.head)
        ))
    }

    /// Reads the project files from the branch tip of the video, else from the default branch.
    fn read_video_files(&self, slug: &str) -> Result<String, String> {
        let board = self.board()?;
        let mut out = String::new();
        let mut found = false;
        for file in VIDEO_FILES {
            let _ = writeln!(out, "== {file} ==");
            match board.read_file(slug, file).map_err(|e| e.to_string())? {
                Some(content) => {
                    found = true;
                    out.push_str(&content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                }
                None => out.push_str("(missing)\n"),
            }
        }
        if found {
            Ok(out)
        } else {
            Err(format!("no video `{slug}` in this channel"))
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ShotServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("shot", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Shot Studio channel board. Read videos, transcripts, the review queue and \
                 caption fixes, move cards, and propose changes to a video's cut and script. \
                 Read a take's elements and the lock with its provenance, and propose a rule \
                 at the narrowest layer or an override for one element; the reply says how \
                 many instances and videos it reaches. Proposals are branches the human \
                 reviews and accepts. A cut only removes or restores recorded words.",
            )
    }
}

/// A known project file must parse before it is proposed.
fn validate(file: &str, text: &str) -> Result<(), String> {
    let parsed = match file {
        "video.kdl" => Video::parse(text).map(drop),
        "cut.kdl" => Cut::parse(text).map(drop),
        "format.kdl" => Format::parse(text).map(drop),
        "script.md" => Script::parse(text).map(drop),
        _ => Ok(()),
    };
    parsed.map_err(|e| format!("{file} does not parse: {e}"))
}

fn take_id(text: &str) -> Result<Id, String> {
    Id::new(text).ok_or_else(|| format!("`{text}` is not a take id"))
}

/// Slugs end up in paths, so they are checked before touching the working tree.
fn shot_board_check_name(slug: &str) -> Result<(), String> {
    let valid = !slug.is_empty()
        && !slug.starts_with('-')
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!("`{slug}` is not a valid name"))
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}
