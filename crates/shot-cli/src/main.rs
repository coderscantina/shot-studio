//! `shot`: the channel board, recording, export and the MCP server from the terminal.
//!
//! Every board subcommand is one call into `shot-board`. The words come from `shot_mcp::text`, so the
//! CLI and the MCP tools print the same thing.

mod agent;
mod analyze;
mod autocut;
mod caption_fix;
mod migrate;
mod models;
mod record;
mod restyle;
mod style;
mod transcribe;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use shot_board::{Activity, Board, Card, Proposal};
use shot_core::Status;
use shot_mcp::text;

#[cfg(target_os = "macos")]
mod build;
#[cfg(target_os = "macos")]
mod derived;
#[cfg(target_os = "macos")]
mod export;
#[cfg(target_os = "macos")]
mod overlays;
#[cfg(target_os = "macos")]
mod product;
#[cfg(target_os = "macos")]
mod ship;

#[derive(Parser)]
#[command(name = "shot", version, about = "Shot Studio from the terminal")]
struct Cli {
    /// The channel repo. Defaults to the current directory.
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the videos by column, or move a card.
    Board {
        #[command(flatten)]
        output: Output,
        #[command(subcommand)]
        action: Option<BoardAction>,
    },
    /// Show the newest commits across all video branches.
    Log {
        /// Maximum number of commits.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// List open proposals, optionally of one video.
    Proposals {
        slug: Option<String>,
        #[command(flatten)]
        output: Output,
    },
    /// Review a proposal.
    Proposal {
        #[command(subcommand)]
        action: ProposalAction,
    },
    /// Record a take into a video folder.
    Record(record::RecordArgs),
    /// Render a video's cut to an MP4, reusing cached segments.
    #[cfg(target_os = "macos")]
    Export(export::ExportArgs),
    /// Rebuild the lock and every cut, then check them; once, on every change, or a proposal's.
    #[cfg(target_os = "macos")]
    Build(build::BuildArgs),
    /// Write zooms on clicks, typing or the resting cursor, per format.kdl's auto-zoom rule.
    #[cfg(target_os = "macos")]
    Autozoom(product::AutoZoomArgs),
    /// Snapshot a commit range into sources/ and type it out as a code-diff overlay.
    #[cfg(target_os = "macos")]
    CodeDiff(product::CodeDiffArgs),
    /// Find link cards, highlights and b-roll slots, or place a bleep, blur or b-roll.
    #[cfg(target_os = "macos")]
    Overlays(overlays::OverlaysArgs),
    /// Add, list or remove Shorts and X clips; export one with `export --cut <name>`.
    #[cfg(target_os = "macos")]
    Derived(derived::DerivedArgs),
    /// Build a video's cut.lock, explain an instance, or measure a rule's scope.
    Style(style::StyleArgs),
    /// Turn a video written before the lock into content, overrides and style rules, once.
    Migrate(migrate::MigrateArgs),
    /// Rebuild a shipped video's frozen lock on purpose, for a new release.
    Restyle(restyle::RestyleArgs),
    /// Run the MCP server on stdio.
    Mcp(agent::McpArgs),
    /// Ask Claude through the local Claude Code CLI, with the MCP server attached.
    Agent(agent::AgentArgs),
    /// Check a cut before it ships. Prints only problems.
    #[cfg(target_os = "macos")]
    Check(ship::CheckArgs),
    /// Find thumbnail candidates by expression strength, or pick one.
    #[cfg(target_os = "macos")]
    Thumbnails(ship::ThumbnailsArgs),
    /// Draft the title and description with chapters at their first spoken word.
    #[cfg(target_os = "macos")]
    Describe(ship::DescribeArgs),
    /// Render, check and upload the main cut and Shorts to YouTube, then tag the release.
    #[cfg(target_os = "macos")]
    Ship(ship::ShipArgs),
    /// Download or list the analysis models.
    Models {
        #[command(subcommand)]
        action: models::ModelsAction,
    },
    /// Transcribe a take's mic track into the transcript cache.
    Transcribe(transcribe::TranscribeArgs),
    /// Run the auto-cut rules on a take and write the rough cut into cut.kdl.
    Autocut(autocut::AutocutArgs),
    /// Run a take's analysis again and rewrite its elements file.
    Analyze(analyze::AnalyzeArgs),
    /// Add, change or remove a caption fix: words the transcript mishears and what captions show.
    CaptionFix(caption_fix::CaptionFixArgs),
}

#[derive(Args)]
struct Output {
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum BoardAction {
    /// Move a video to another column with one commit.
    Move {
        slug: String,
        #[arg(value_parser = text::parse_status)]
        status: Status,
    },
}

#[derive(Subcommand)]
enum ProposalAction {
    /// Show what the proposal changes.
    Diff { slug: String, name: String },
    /// Merge the proposal into the video and delete its branch.
    Accept { slug: String, name: String },
    /// Delete the proposal branch.
    Reject { slug: String, name: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("shot: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Models { action } => return models::run(action),
        Command::Transcribe(args) => return transcribe::run(args),
        Command::Autocut(args) => return autocut::run(args),
        Command::Analyze(args) => return analyze::run(args),
        Command::CaptionFix(args) => return caption_fix::run(args),
        Command::Style(args) => return style::run(cli.repo, args),
        Command::Migrate(args) => return migrate::run(args),
        Command::Restyle(args) => return restyle::run(cli.repo, args),
        _ => {}
    }
    if let Command::Mcp(args) = cli.command {
        return agent::mcp(cli.repo, args);
    }
    if let Command::Agent(args) = cli.command {
        return agent::run(&cli.repo, args);
    }
    #[cfg(target_os = "macos")]
    match cli.command {
        Command::Check(args) => return ship::check(args),
        Command::Thumbnails(args) => return ship::thumbnails(args),
        Command::Describe(args) => return ship::describe(args),
        Command::Ship(args) => return ship::run_ship(args),
        _ => {}
    }
    #[cfg(target_os = "macos")]
    if let Command::Export(args) = cli.command {
        return export::run(args);
    }
    #[cfg(target_os = "macos")]
    if let Command::Build(args) = cli.command {
        return build::run(args);
    }
    #[cfg(target_os = "macos")]
    if let Command::Autozoom(args) = cli.command {
        return product::auto_zoom(args);
    }
    #[cfg(target_os = "macos")]
    if let Command::CodeDiff(args) = cli.command {
        return product::code_diff(args);
    }
    #[cfg(target_os = "macos")]
    if let Command::Overlays(args) = cli.command {
        return overlays::run(args);
    }
    #[cfg(target_os = "macos")]
    if let Command::Derived(args) = cli.command {
        return derived::run(args);
    }

    if let Command::Record(args) = cli.command {
        return record::run(args);
    }

    let board = Board::open(&cli.repo)
        .map_err(|e| format!("{} is not a channel repo: {e}", cli.repo.display()))?;
    let fail = |e: shot_board::Error| e.to_string();
    match cli.command {
        Command::Board {
            output,
            action: None,
        } => {
            let cards = board.cards().map_err(fail)?;
            print_as(output, &cards_json(&cards), || text::board(&cards));
        }
        Command::Board {
            action: Some(BoardAction::Move { slug, status }),
            ..
        } => match board.move_card(&slug, status).map_err(fail)? {
            Some(sha) => println!("moved {slug} to {status} in {}", &sha[..7]),
            None => println!("{slug} is already in {status}"),
        },
        Command::Log { limit, output } => {
            let entries = board.activity(limit).map_err(fail)?;
            print_as(output, &activity_json(&entries), || {
                text::activity(&entries)
            });
        }
        Command::Proposals { slug, output } => {
            let found = board.proposals(slug.as_deref()).map_err(fail)?;
            print_as(output, &proposals_json(&found), || text::proposals(&found));
        }
        Command::Proposal {
            action: ProposalAction::Diff { slug, name },
        } => print!(
            "{}",
            text::diffs(&board.proposal_diff(&slug, &name).map_err(fail)?)
        ),
        Command::Proposal {
            action: ProposalAction::Accept { slug, name },
        } => {
            let accepted = board.accept_proposal(&slug, &name).map_err(fail)?;
            let how = if accepted.fast_forward {
                "fast-forward"
            } else {
                "merge commit"
            };
            println!(
                "accepted {name} into {slug} ({how}) at {}",
                &accepted.head[..7]
            );
        }
        Command::Proposal {
            action: ProposalAction::Reject { slug, name },
        } => {
            board.reject_proposal(&slug, &name).map_err(fail)?;
            println!("rejected {slug}/{name}");
        }
        Command::Mcp(_)
        | Command::Agent(_)
        | Command::Record(_)
        | Command::Models { .. }
        | Command::Transcribe(_)
        | Command::Autocut(_)
        | Command::Style(_)
        | Command::Migrate(_)
        | Command::Restyle(_)
        | Command::Analyze(_)
        | Command::CaptionFix(_) => {
            unreachable!("handled before opening the board")
        }
        #[cfg(target_os = "macos")]
        Command::Export(_)
        | Command::Build(_)
        | Command::Autozoom(_)
        | Command::CodeDiff(_)
        | Command::Overlays(_)
        | Command::Derived(_)
        | Command::Check(_)
        | Command::Thumbnails(_)
        | Command::Describe(_)
        | Command::Ship(_) => {
            unreachable!("handled before opening the board")
        }
    }
    Ok(())
}

fn print_as(output: Output, json: &Value, text: impl FnOnce() -> String) {
    if output.json {
        println!("{json:#}");
    } else {
        print!("{}", text());
    }
}

fn cards_json(cards: &[Card]) -> Value {
    cards
        .iter()
        .map(|card| match &card.video {
            Ok(video) => json!({
                "slug": card.slug,
                "has_branch": card.has_branch,
                "title": video.title,
                "status": video.status.as_str(),
                "format": video.format.as_str(),
                "target_date": video.target_date.map(|d| {
                    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
                }),
            }),
            Err(error) => json!({
                "slug": card.slug,
                "has_branch": card.has_branch,
                "error": error.to_string(),
            }),
        })
        .collect()
}

fn activity_json(entries: &[Activity]) -> Value {
    entries
        .iter()
        .map(|e| {
            json!({
                "slug": e.slug,
                "sha": e.sha,
                "time": e.time,
                "subject": e.subject,
                "author": e.author,
            })
        })
        .collect()
}

fn proposals_json(found: &[Proposal]) -> Value {
    found
        .iter()
        .map(|p| {
            json!({
                "slug": p.slug,
                "name": p.name,
                "head": p.head,
                "subject": p.subject,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("shot").chain(args.iter().copied()))
    }

    #[test]
    fn repo_defaults_to_the_current_directory_and_works_before_or_after_the_subcommand() {
        assert_eq!(parse(&["board"]).unwrap().repo, PathBuf::from("."));
        assert_eq!(
            parse(&["--repo", "/a", "log"]).unwrap().repo,
            PathBuf::from("/a")
        );
        assert_eq!(
            parse(&["proposals", "--repo", "/b"]).unwrap().repo,
            PathBuf::from("/b")
        );
    }

    #[test]
    fn board_move_takes_a_slug_and_a_known_column() {
        let cli = parse(&["board", "move", "alpha", "ready"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Board {
                action: Some(BoardAction::Move {
                    status: Status::Ready,
                    ..
                }),
                ..
            }
        ));
        let error = parse(&["board", "move", "alpha", "done"]).err().unwrap();
        assert!(error.to_string().contains("expected one of: ideas"));
    }

    #[test]
    fn proposal_review_needs_slug_and_name() {
        assert!(parse(&["proposal", "accept", "alpha", "tighter-intro"]).is_ok());
        assert!(parse(&["proposal", "reject", "alpha"]).is_err());
        assert!(parse(&["mcp"]).is_ok());
        assert!(parse(&["mcp", "--log", "/tmp/x", "--frame-task"]).is_ok());
    }
}
