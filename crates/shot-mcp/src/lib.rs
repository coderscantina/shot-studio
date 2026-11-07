//! The `shot` MCP server: the channel board for outside agents, over stdio.
//!
//! Every tool is a thin call into `shot-board` and returns text only ([0005]). Accepting or
//! rejecting a proposal is the human's review action, so it is deliberately not a tool.
//!
//! [0005]: ../../docs/decisions/0005-privacy-text-only-to-claude.md

pub mod text;

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

use git2::{BranchType, Repository};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::stdio;
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use shot_board::Board;

/// The files of a video folder that `read_video` returns.
const VIDEO_FILES: [&str; 4] = ["video.kdl", "script.md", "cut.kdl", "format.kdl"];
const DEFAULT_ACTIVITY_LIMIT: usize = 20;

/// Serves the channel repo at `repo` on stdin and stdout until the client disconnects.
pub async fn serve_stdio(repo: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    ShotServer::new(repo)
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

/// One MCP session over one channel repo. The repo is opened per call, so nothing is cached and
/// changes made by the app or by git in between are always seen.
#[derive(Clone)]
pub struct ShotServer {
    repo: PathBuf,
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

#[tool_router]
impl ShotServer {
    pub fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            tool_router: Self::tool_router(),
        }
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
        description = "Propose changes to a video as a branch with one commit. Nothing changes on the video until the human accepts the proposal in the app or with `shot proposal accept`."
    )]
    fn create_proposal(
        &self,
        Parameters(args): Parameters<CreateProposalArgs>,
    ) -> Result<String, String> {
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

    fn board(&self) -> Result<Board, String> {
        Board::open(&self.repo)
            .map_err(|e| format!("{} is not a channel repo: {e}", self.repo.display()))
    }

    /// Reads the project files from the branch tip of the video, else from the default branch.
    fn read_video_files(&self, slug: &str) -> Result<String, String> {
        let valid = !slug.is_empty()
            && !slug.starts_with('-')
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            return Err(format!(
                "`{slug}` is not a valid slug, use lowercase letters, digits and dashes"
            ));
        }
        self.board()?;
        let git = |e: git2::Error| e.to_string();
        let repo = Repository::open(&self.repo).map_err(git)?;
        let tip = [format!("video/{slug}"), "main".into(), "master".into()]
            .iter()
            .find_map(|name| {
                repo.find_branch(name, BranchType::Local)
                    .ok()?
                    .get()
                    .peel_to_commit()
                    .ok()
            })
            .or_else(|| repo.head().ok()?.peel_to_commit().ok())
            .ok_or_else(|| "the repo has no commits".to_owned())?;
        let tree = tip.tree().map_err(git)?;

        let mut out = String::new();
        let mut found = false;
        for file in VIDEO_FILES {
            let path = format!("videos/{slug}/{file}");
            let _ = writeln!(out, "== {file} ==");
            match tree.get_path(path.as_ref()) {
                Ok(entry) => {
                    found = true;
                    let blob = repo.find_blob(entry.id()).map_err(git)?;
                    let content = String::from_utf8_lossy(blob.content());
                    out.push_str(&content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Err(_) => out.push_str("(missing)\n"),
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
                "Shot Studio channel board. Read videos, move cards and propose changes to a \
                 video's files. Proposals are reviewed and accepted by the human.",
            )
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}
