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
pub mod brand;
pub mod bridge;
mod confine;
pub mod style;
mod tee;
pub mod text;
pub mod transcript;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::path::PathBuf;

use base64::Engine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use shot_board::Board;
use shot_core::{
    Cut, Decision, ElementKind, Format, FormatLayer, Id, Micros, Project, Script, Selector,
    Transcript, Video,
};

use crate::transcript::WordSpan;

/// The files of a video folder that `read_video` returns.
const VIDEO_FILES: [&str; 4] = ["video.kdl", "script.md", "cut.kdl", "format.kdl"];
const DEFAULT_ACTIVITY_LIMIT: usize = 20;
/// The most elements or instances one style tool prints.
const STYLE_LIMIT: usize = 300;
/// Where `shot thumbnails` writes its candidates, below the video folder.
const THUMBNAILS: &str = ".shot/cache/thumbnails";

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
/// changes made by the app or by git in between are always seen. A video's files are read from
/// its worktree ([0037]), uncommitted edits included, and transcripts from the cache its
/// `.shot/` links to, the main checkout's `videos/<slug>/.shot/cache`.
///
/// [0037]: ../../docs/decisions/0037-a-worktree-per-video-branch.md
#[derive(Clone)]
pub struct ShotServer {
    repo: PathBuf,
    frame_task: bool,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListVideosArgs {
    /// Only videos in this column: ideas, scripting, ready, recorded, editing or shipped.
    status: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct VideoArgs {
    /// The video slug, for example `why-we-deploy-on-fridays`.
    slug: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveCardArgs {
    /// The video slug.
    slug: String,
    /// The target column: ideas, scripting, ready, recorded, editing or shipped.
    status: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivityArgs {
    /// Maximum number of commits, newest first. Defaults to 20.
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListProposalsArgs {
    /// Only proposals of this video.
    slug: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct ProposalArgs {
    /// The video slug.
    slug: String,
    /// The proposal name.
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct ReviewDecision {
    /// The item number from `review_queue`.
    item: usize,
    /// `keep` or `cut`.
    decision: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct ProposeScriptArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes, for example `hook-a`.
    name: String,
    /// The commit subject.
    subject: String,
    /// The whole new script.md: `# title`, `## chapter`, `- beat` with `[label](source)` links.
    script: String,
    /// Text files to add to the video's `sources/` with the script, by path below `sources/`,
    /// for example a snapshot of an article the beats link to as `sources/article.md`.
    #[serde(default)]
    sources: BTreeMap<String, String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct ExplainArgs {
    /// The video slug.
    slug: String,
    /// The take id.
    take: String,
    /// The time to explain, in the take's media time, microseconds.
    at_us: u64,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RuleScopeArgs {
    /// The rule's selector as the lock writes it, for example `silence min=500_000`.
    selector: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListThemesArgs {}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ThemeArgs {
    /// The theme id, its file `brand/themes/<id>.kdl`, as list_themes shows it.
    theme: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveThemeArgs {
    /// The new theme's id: lowercase letters, digits and dashes.
    name: String,
    /// The theme whose look to copy; the channel's theme when left out.
    from: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameThemeArgs {
    /// The theme's id now.
    from: String,
    /// Its new id: lowercase letters, digits and dashes.
    to: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProposePresetArgs {
    /// The video slug.
    slug: String,
    /// Proposal name: lowercase letters, digits and dashes.
    name: String,
    /// The commit subject.
    subject: String,
    /// The overlay preset's id, as list_themes shows it.
    preset: String,
    /// The take id.
    take: String,
    /// Where the overlay starts, media time of the take in microseconds.
    at_us: u64,
}

#[tool_router]
impl ShotServer {
    pub fn new(repo: PathBuf) -> Self {
        let tool_router = Self::tool_router();
        #[cfg(test)]
        let tool_router = tool_router + Self::panic_router();
        Self {
            repo,
            frame_task: false,
            tool_router,
        }
    }

    /// Allows the frame task tools in this session.
    pub fn with_frame_task(mut self, allowed: bool) -> Self {
        self.frame_task = allowed;
        self
    }

    #[tool(
        description = "List the videos of the channel by board column with title, format and target date. Videos whose video.kdl cannot be parsed are listed separately with file:line:column.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "Read the text of one video as it is being edited, uncommitted edits included: video.kdl, script.md, cut.kdl and format.kdl. Files that do not exist yet are marked as missing.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn read_video(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        self.read_video_files(&args.slug)
    }

    #[tool(
        description = "Move a video to another board column. This is one commit on the video's branch.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "The newest commits across all video branches, newest first, with time, author and subject.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn activity(&self, Parameters(args): Parameters<ActivityArgs>) -> Result<String, String> {
        let limit = args.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT);
        let entries = self.board()?.activity(limit).map_err(|e| e.to_string())?;
        Ok(text::activity(&entries))
    }

    #[tool(
        description = "List open proposals, optionally of one video.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
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
        description = "Propose changes to a video's text files as a branch with one commit. Every known project file must parse. Nothing changes on the video until the human accepts the proposal in the app or with `shot proposal accept`. Prefer propose_cut for cuts and propose_script for the script.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn create_proposal(
        &self,
        Parameters(args): Parameters<CreateProposalArgs>,
    ) -> Result<String, String> {
        for (file, text) in &args.files {
            confine::relative(file)?;
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
        description = "The unified diff of a proposal against the point where it branched off the video.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn proposal_diff(&self, Parameters(args): Parameters<ProposalArgs>) -> Result<String, String> {
        let files = self
            .board()?
            .proposal_diff(&args.slug, &args.name)
            .map_err(|e| e.to_string())?;
        Ok(text::diffs(&files))
    }

    #[tool(
        description = "The transcript of a video in cut order. Each line starts with the edited time of its first kept word and the number of its first word in the take (w12); count on from there. Removed words are in brackets with what removed them: [um so](filler).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn read_transcript(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let built = self.built(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &built.cut)?;
        Ok(transcript::render(&built.cut, &built.lock, &transcripts))
    }

    #[tool(
        description = "Propose a cut as a branch: remove spans of recorded words, or restore removed ones, by word number from read_transcript. A cut can only drop or bring back what was recorded, never add words. Replies with the words each span covers, so check them.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn propose_cut(&self, Parameters(args): Parameters<ProposeCutArgs>) -> Result<String, String> {
        let built = self.built(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &built.cut)?;
        let (cut, changes) = transcript::apply(
            &built.cut,
            &built.lock,
            &transcripts,
            &args.remove,
            &args.restore,
        )?;
        let created = self.propose(
            &args.slug,
            &args.name,
            &args.subject,
            BTreeMap::from([("cut.kdl".to_owned(), cut.to_kdl())]),
        )?;
        let mut out = created;
        for change in changes {
            let _ = writeln!(out, "  {change}");
        }
        Ok(out)
    }

    #[tool(
        description = "The open review queue of a video: elements a cut rule was not sure about, numbered, with their take, element kind, length and words.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn review_queue(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let built = self.built(&args.slug)?;
        let cut = &built.cut;
        let transcripts = self.transcripts(&args.slug, cut)?;
        let mut out = String::new();
        for (index, (at, review)) in cut.review_queue(&built.lock).enumerate() {
            let take = &cut.chapters[at.chapter].takes[at.take].take;
            let _ = writeln!(
                out,
                "#{}  {}  {}  {:.1} s  \"{}\"",
                index + 1,
                take,
                review.kind,
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
        description = "Propose answers to review items as a branch: keep leaves the element in and tells the rule to leave it alone, cut removes it. Both are overrides in cut.kdl. Only answer items you are sure about.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn propose_review(
        &self,
        Parameters(args): Parameters<ProposeReviewArgs>,
    ) -> Result<String, String> {
        let built = self.built(&args.slug)?;
        let mut cut = built.cut.clone();
        let queue: Vec<_> = built.cut.review_queue(&built.lock).collect();
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
                .decide(*at, review.range, answer, &built.lock)
                .map_err(|e| e.to_string())?;
        }
        self.propose(
            &args.slug,
            &args.name,
            &args.subject,
            BTreeMap::from([("cut.kdl".to_owned(), cut.to_kdl())]),
        )
    }

    #[tool(
        description = "The caption fixes dictionary of a video and the words recognized with low confidence that no fix covers yet. Suggest new fixes in your reply as heard -> said; a person adds them in Caption fixes. Fixes are never proposals.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        let timeline = shot_ship::lock::timeline(&self.video_dir(&args.slug)?, &project)
            .map_err(|e| e.to_string())?;
        let main = timeline.main();
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
        description = "Checks on a video's script.md: estimated spoken length per chapter, hook length, beats without a source and long stretches without a visual.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "Propose a new script.md as a branch, optionally with text files for sources/ that its beats link to. The script must follow the script.md shape. Name alternatives you offer side by side alike, for example hook-a, hook-b and hook-c; the person accepts one.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn propose_script(
        &self,
        Parameters(args): Parameters<ProposeScriptArgs>,
    ) -> Result<String, String> {
        validate("script.md", &args.script)?;
        let mut files = BTreeMap::new();
        for (name, text) in args.sources {
            confine::relative(&name)?;
            files.insert(format!("sources/{name}"), text);
        }
        files.insert("script.md".to_owned(), args.script);
        self.propose(&args.slug, &args.name, &args.subject, files)
    }

    #[tool(
        description = "A draft title and description for the upload: the hook, chapters at the moment their first word is heard or their intro starts, and the links the video shows or the script cites.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn describe(&self, Parameters(args): Parameters<VideoArgs>) -> Result<String, String> {
        let cut = self.cut(&args.slug)?;
        let transcripts = self.transcripts(&args.slug, &cut)?;
        let project = self.project(&args.slug, cut)?;
        let dir = self.video_dir(&args.slug)?;
        let timeline = shot_ship::lock::timeline(&dir, &project).map_err(|e| e.to_string())?;
        // A brand export cannot place yet still leaves a draft, its chapters said to be early.
        let (bookends, note) = match shot_ship::lock::bookends(&dir, &project) {
            Ok(bookends) => (bookends, String::new()),
            Err(e) => (
                Default::default(),
                format!(
                    "note: chapter times leave out the intro and outro, which export cannot place: {e}\n\n"
                ),
            ),
        };
        let description =
            shot_ship::describe::describe(&project, timeline.main(), &bookends, &transcripts);
        Ok(format!(
            "{note}title: {}\n\n{}",
            description.title, description.text
        ))
    }

    #[tool(
        description = "The elements (facts) of one take as JSON lines, in take media time: clicks, silences, fillers, urls and the rest, with their t, to, confidence and source. Words are left out unless asked for. Use their t as the identity of an override.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        let built = self.built(&args.slug)?;
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
        description = "The lock of a video, built from its files: every instance (cut, review, zoom, overlay, sfx, transition, layout, captions) with its resolved values and its provenance, `on=` the element, `layer=` and the `rule=` or `override=` that made it. Narrow it by take and time window.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn read_lock(&self, Parameters(args): Parameters<ReadLockArgs>) -> Result<String, String> {
        let take = args.take.as_deref().map(take_id).transpose()?;
        let built = self.built(&args.slug)?;
        let window = style::window(args.from_us, args.to_us)?;
        Ok(style::lock_text(
            &built.lock,
            take.as_ref(),
            window,
            STYLE_LIMIT,
        ))
    }

    #[tool(
        description = "Why a moment looks the way it does: the instances at a time in a take with the layer and rule or override behind each, and the overrides in cut.kdl there (an `off` leaves no instance).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn explain_instance(
        &self,
        Parameters(args): Parameters<ExplainArgs>,
    ) -> Result<String, String> {
        let take = take_id(&args.take)?;
        let built = self.built(&args.slug)?;
        Ok(style::explain(&built, Some(&take), Micros(args.at_us)))
    }

    #[tool(
        description = "How far a rule reaches: the instances and elements the rule with this selector produces now, in how many of the channel's unshipped videos. Shipped videos keep their lock.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn rule_scope(&self, Parameters(args): Parameters<RuleScopeArgs>) -> Result<String, String> {
        let selector = Selector::parse(&args.selector).map_err(|e| e.to_string())?;
        let board = self.board()?;
        let reach = style::reach(&style::Git(&board), &selector)?;
        Ok(format!("on {selector}\n{reach}"))
    }

    #[tool(
        description = "Propose a change of one style rule as a branch, at the layer you choose: this video's format.kdl, the channel's format or the brand. The reply and the proposal's description state the scope: how many instances change in how many videos. Pick the narrowest layer that fits the person's intent.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
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
        description = "Propose an override for one element as a branch: what this single element gets instead of what the rules gave it, written to cut.kdl. The element must exist (read_elements). The reply states the instances it changes.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
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
        description = "Frame task: the thumbnail candidates of a video as still images, best first. Runs only when the person started a thumbnail pick in the app; otherwise it refuses.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        let folder = self.video_dir(&args.slug)?;
        let dir = confine::resolve(&self.repo, &args.slug, &folder, THUMBNAILS)?;
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
            let name = still.file_name().unwrap_or_default().to_string_lossy();
            let real = confine::resolve(
                &self.repo,
                &args.slug,
                &folder,
                &format!("{THUMBNAILS}/{name}"),
            )?;
            let bytes = std::fs::read(&real).map_err(|e| format!("{}: {e}", real.display()))?;
            content.push(ContentBlock::text(format!("candidate {name}")));
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ));
        }
        Ok(CallToolResult::success(content))
    }

    #[tool(
        description = "The channel's themes, the one it uses marked (active), and its overlay presets with template and length.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn list_themes(&self, Parameters(_): Parameters<ListThemesArgs>) -> Result<String, String> {
        Ok(brand::list_text(&brand::read(&self.repo)?))
    }

    #[tool(
        description = "Make a theme the channel's: changes the one `theme` line of brand/brand.kdl in the channel's working tree, as the app's theme window does. Every video built afterwards uses it, unless its format layer names another theme; shipped videos keep their look.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn use_theme(&self, Parameters(args): Parameters<ThemeArgs>) -> Result<String, String> {
        let theme = theme_id(&args.theme)?;
        brand::activate(&self.repo, &theme)?;
        Ok(format!("the channel uses theme {theme}\n"))
    }

    #[tool(
        description = "Save a copy of a theme's look as a new theme, brand/themes/<name>.kdl. The channel keeps using its theme.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn save_theme(&self, Parameters(args): Parameters<SaveThemeArgs>) -> Result<String, String> {
        let name = theme_id(&args.name)?;
        let from = args.from.as_deref().map(theme_id).transpose()?;
        brand::save(&self.repo, &name, from.as_ref())?;
        Ok(format!("saved theme {name}\n"))
    }

    #[tool(
        description = "Rename a theme: its file, and the `theme` line of brand/brand.kdl when the channel uses it. A theme a format layer names is not renamed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn rename_theme(
        &self,
        Parameters(args): Parameters<RenameThemeArgs>,
    ) -> Result<String, String> {
        let (from, to) = (theme_id(&args.from)?, theme_id(&args.to)?);
        brand::rename(&self.repo, &from, &to)?;
        Ok(format!("renamed theme {from} to {to}\n"))
    }

    #[tool(
        description = "Delete a theme's file. The theme the channel uses and a theme a format layer names are not deleted.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    fn delete_theme(&self, Parameters(args): Parameters<ThemeArgs>) -> Result<String, String> {
        let theme = theme_id(&args.theme)?;
        brand::delete(&self.repo, &theme)?;
        Ok(format!("deleted theme {theme}\n"))
    }

    #[tool(
        description = "Propose placing one of the brand's overlay presets in a video as a branch: an overlay node with the preset's template and parameters, from a take's media time for the preset's length, in the take block that keeps that moment. To place a preset wherever an element occurs, propose a rule with `overlay preset=<id>` instead.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    fn propose_preset(
        &self,
        Parameters(args): Parameters<ProposePresetArgs>,
    ) -> Result<String, String> {
        let take = take_id(&args.take)?;
        let preset =
            Id::new(&args.preset).ok_or_else(|| format!("`{}` is not a preset id", args.preset))?;
        let themes = brand::read(&self.repo)?;
        let overlay = themes
            .preset(&preset)
            .map_err(|e| e.to_string())?
            .place(Micros(args.at_us));
        let cut = self
            .cut(&args.slug)?
            .place_overlay(&take, overlay)
            .map_err(|e| e.to_string())?;
        let files = BTreeMap::from([("cut.kdl".to_owned(), cut.to_kdl())]);
        self.propose(&args.slug, &args.name, &args.subject, files)
    }

    fn board(&self) -> Result<Board, String> {
        Board::open(&self.repo)
            .map_err(|e| format!("{} is not a channel repo: {e}", self.repo.display()))
    }

    /// The video's folder, in its worktree once it is committed, see [`style::video_folder`].
    fn video_dir(&self, slug: &str) -> Result<PathBuf, String> {
        confine::slug(slug)?;
        let folder = style::video_folder(&self.repo, slug)?;
        confine::resolve(&self.repo, slug, &folder, "")
    }

    /// Opens the video's folder as every front end does, so a video written before the lock is
    /// migrated before a tool reads it, and returns its files.
    fn open(&self, slug: &str) -> Result<style::Disk, String> {
        let dir = self.video_dir(slug)?;
        if dir.join("video.kdl").exists() {
            let _lock =
                shot_fs::WriteLock::acquire(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            style::open(&dir)?;
        }
        Ok(style::Disk::locate(&dir)?.0)
    }

    /// The files of the video's folder as they are, for reads after [`ShotServer::open`].
    fn disk(&self, slug: &str) -> Result<style::Disk, String> {
        Ok(style::Disk::locate(&self.video_dir(slug)?)?.0)
    }

    /// The text of `file` in the video's folder, `None` when it has none yet.
    fn read(&self, disk: &style::Disk, slug: &str, file: &str) -> Result<Option<String>, String> {
        let path = style::video_path(slug, file);
        style::Repo::read(disk, slug, &path)?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8")))
            .transpose()
    }

    /// The video with the lock its files build.
    fn built(&self, slug: &str) -> Result<style::Built, String> {
        style::build_video(&self.open(slug)?, slug)
    }

    /// The video's cut, empty when it has none yet.
    fn cut(&self, slug: &str) -> Result<Cut, String> {
        match self.read(&self.open(slug)?, slug, "cut.kdl")? {
            Some(text) => Cut::parse(&text).map_err(|e| e.to_string()),
            None => Ok(Cut::default()),
        }
    }

    fn script(&self, slug: &str) -> Result<Script, String> {
        let text = self
            .read(&self.disk(slug)?, slug, "script.md")?
            .ok_or_else(|| format!("{slug} has no script.md yet"))?;
        Script::parse(&text).map_err(|e| e.to_string())
    }

    fn project(&self, slug: &str, cut: Cut) -> Result<Project, String> {
        let disk = self.disk(slug)?;
        let read = |file: &str| -> Result<String, String> {
            self.read(&disk, slug, file)?
                .ok_or_else(|| format!("{slug} has no {file} yet"))
        };
        let video = Video::parse(&read("video.kdl")?).map_err(|e| e.to_string())?;
        let format_text = self.read(&disk, slug, "format.kdl")?;
        Ok(Project {
            script: Script::parse(&read("script.md")?).map_err(|e| e.to_string())?,
            format: resolved_format(&disk, slug, &video, format_text.as_deref())?,
            video,
            cut,
        })
    }

    /// The cached transcripts of the takes `cut` uses, from the working tree.
    fn transcripts(&self, slug: &str, cut: &Cut) -> Result<HashMap<Id, Transcript>, String> {
        let dir = self.video_dir(slug)?;
        let mut found = HashMap::new();
        for take in cut.chapters.iter().flat_map(|c| &c.takes).map(|t| &t.take) {
            if found.contains_key(take) {
                continue;
            }
            let cache = shot_analyze::cache::cache_path(&dir, take);
            let relative = cache.strip_prefix(&dir).unwrap_or(&cache);
            confine::resolve(&self.repo, slug, &dir, &relative.to_string_lossy())?;
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
        files: BTreeMap<String, String>,
    ) -> Result<String, String> {
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

    /// Reads the project files from the video's worktree, opened as the front ends open it.
    fn read_video_files(&self, slug: &str) -> Result<String, String> {
        let disk = self.open(slug)?;
        let mut out = String::new();
        let mut found = false;
        for file in VIDEO_FILES {
            let _ = writeln!(out, "== {file} ==");
            match self.read(&disk, slug, file)? {
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
    /// Runs the tool on its own task, so a panic in it is an error result and the session
    /// keeps serving.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let server = self.clone();
        let name = request.name.to_string();
        let ran = tokio::spawn(async move {
            let call = ToolCallContext::new(&server, request, context);
            server.tool_router.call(call).await
        })
        .await;
        ran.unwrap_or_else(|e| {
            let cause = e.try_into_panic().ok().map_or_else(String::new, |p| {
                match (p.downcast_ref::<&str>(), p.downcast_ref::<String>()) {
                    (Some(m), _) => format!(": {m}"),
                    (_, Some(m)) => format!(": {m}"),
                    _ => String::new(),
                }
            });
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "tool {name} failed unexpectedly{cause}"
            ))])
            .into())
        })
    }

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

/// A video's format: its layer of differences over the preset and the channel's
/// `formats/<format>.kdl`, as the app and export resolve it.
fn resolved_format(
    disk: &style::Disk,
    slug: &str,
    video: &Video,
    text: Option<&str>,
) -> Result<Format, String> {
    let layer = text
        .map(|t| FormatLayer::parse(t).map_err(|e| format!("format.kdl:{e}")))
        .transpose()?
        .unwrap_or_default();
    let path = format!("formats/{}.kdl", video.format);
    let channel = style::Repo::read(disk, slug, &path)?
        .map(|bytes| {
            let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
            FormatLayer::parse(&text).map_err(|e| format!("{path}:{e}"))
        })
        .transpose()?;
    Ok(layer.apply(&video.format.preset().resolve(channel.iter())))
}

/// A known project file must parse before it is proposed.
fn validate(file: &str, text: &str) -> Result<(), String> {
    let parsed = match file {
        "video.kdl" => Video::parse(text).map(drop),
        "cut.kdl" => Cut::parse(text).map(drop),
        "format.kdl" => FormatLayer::parse(text).map(drop),
        "script.md" => Script::parse(text).map(drop),
        _ => Ok(()),
    };
    parsed.map_err(|e| format!("{file} does not parse: {e}"))
}

fn theme_id(text: &str) -> Result<Id, String> {
    Id::new(text)
        .ok_or_else(|| format!("`{text}` is not a theme id: lowercase letters, digits and dashes"))
}

fn take_id(text: &str) -> Result<Id, String> {
    Id::new(text).ok_or_else(|| format!("`{text}` is not a take id"))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool that only exists in this crate's unit tests.
    #[tool_router(router = panic_router, vis = "pub(crate)")]
    impl ShotServer {
        #[tool(
            description = "Panics.",
            annotations(
                read_only_hint = true,
                destructive_hint = false,
                idempotent_hint = true,
                open_world_hint = false,
            )
        )]
        fn panics(&self) -> Result<String, String> {
            panic!("boom")
        }
    }

    #[derive(Clone, Default)]
    struct Client;

    impl rmcp::ClientHandler for Client {
        fn get_info(&self) -> rmcp::model::ClientConfig {
            rmcp::model::ClientConfig::default()
        }
    }

    #[tokio::test]
    async fn a_panicking_tool_is_an_error_and_the_session_keeps_working() {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let server = ShotServer::new(PathBuf::from("/nowhere"));
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = Client.serve(client_io).await.unwrap();

        let result = client
            .call_tool(CallToolRequestParams::new("panics"))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = format!("{:?}", result.content);
        assert!(text.contains("panics failed unexpectedly: boom"), "{text}");

        let after = client
            .call_tool(CallToolRequestParams::new("list_videos"))
            .await
            .unwrap();
        assert!(format!("{:?}", after.content).contains("not a channel repo"));
    }
}
