//! `shot`: the channel board, recording and the MCP server from the terminal.
//!
//! Every subcommand is one call into `shot-board`. The words come from `shot_mcp::text`, so the
//! CLI and the MCP tools print the same thing.

mod record;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use shot_board::{Activity, Board, Card, Proposal};
use shot_core::Status;
use shot_mcp::text;

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
    /// Run the MCP server on stdio.
    Mcp,
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
    if let Command::Mcp = cli.command {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(shot_mcp::serve_stdio(cli.repo))
            .map_err(|e| e.to_string());
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
        Command::Mcp | Command::Record(_) => unreachable!("handled before opening the board"),
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
    }
}
