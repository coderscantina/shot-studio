//! `shot check`, `shot thumbnails`, `shot describe` and `shot ship`: a cut before and as it ships.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;
use shot_core::{Id, OutputKind, Project, Transcript};
use shot_ship::oauth::{self, AuthError, Client, Endpoints, Keychain};
use shot_ship::ship::{self, Channel, Destination, Options, PlannedCut, ShipError, Step};
use shot_ship::youtube::{Privacy, YouTube, YouTubeError};

use crate::export::load;

#[derive(Args)]
pub struct CheckArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The cut to check.
    #[arg(long, default_value = "main")]
    cut: String,
    /// The exported file. Defaults to .shot/export/<cut>.mp4 in the video folder, where
    /// `shot export` writes it.
    #[arg(long)]
    out: Option<PathBuf>,
}

/// Prints only the problems; a cut that passes prints nothing and exits 0.
pub fn check(args: CheckArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let export = args
        .out
        .unwrap_or_else(|| export_path(&args.video, &args.cut));
    let problems = shot_ship::media::check_cut(&args.video, &project, &args.cut, &export)
        .map_err(|e| e.to_string())?;
    for problem in &problems {
        println!("{problem}");
    }
    match problems.len() {
        0 => Ok(()),
        1 => Err(format!("1 problem in {}", args.cut)),
        n => Err(format!("{n} problems in {}", args.cut)),
    }
}

#[derive(Args)]
pub struct ThumbnailsArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// How many candidates to write.
    #[arg(long, default_value_t = 6)]
    count: usize,
    /// Use this still as the main cut's thumbnail instead of finding candidates.
    #[arg(long)]
    pick: Option<PathBuf>,
}

pub fn thumbnails(args: ThumbnailsArgs) -> Result<(), String> {
    if let Some(still) = args.pick {
        let to = ship::pick_thumbnail(&args.video, &still)
            .map_err(|e| format!("{}: {e}", still.display()))?;
        println!("thumbnail is {}", to.display());
        return Ok(());
    }
    let project = load(&args.video)?;
    let found = shot_ship::thumbnails::find(&args.video, &project, args.count)
        .map_err(|e| e.to_string())?;
    if found.is_empty() {
        return Err("no face found in the camera frames the main cut shows".to_owned());
    }
    for (index, candidate) in found.iter().enumerate() {
        println!(
            "{}. {} at {} (score {:.2})  {}",
            index + 1,
            candidate.take,
            shot_ship::clock(candidate.at),
            candidate.score,
            candidate
                .path
                .as_deref()
                .map(Path::display)
                .map(|p| p.to_string())
                .unwrap_or_default()
        );
    }
    Ok(())
}

#[derive(Args)]
pub struct DescribeArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
}

pub fn describe(args: DescribeArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let transcripts = transcripts(&args.video, &project)?;
    let description = shot_ship::describe::describe(&project, &transcripts);
    println!("{}\n\n{}", description.title, description.text.trim_end());
    Ok(())
}

fn export_path(video: &Path, cut: &str) -> PathBuf {
    ship::export_path(video, cut)
}

#[derive(Args)]
pub struct ShipArgs {
    /// The video folder, at <repo>/videos/<slug> in a channel repo.
    #[arg(long)]
    video: PathBuf,
    /// Render and check every cut and print what would ship, without uploading or tagging.
    #[arg(long)]
    dry_run: bool,
    /// Ship even when `shot check` finds problems.
    #[arg(long)]
    force: bool,
    /// Who can see the uploads.
    #[arg(long, default_value = "private", value_parser = parse_privacy)]
    privacy: Privacy,
    /// Declare the videos as made for kids. Without it they are declared not made for kids.
    #[arg(long)]
    made_for_kids: bool,
    /// Encoder sessions running at the same time while rendering.
    #[arg(long, default_value_t = 3)]
    jobs: usize,
}

fn parse_privacy(value: &str) -> Result<Privacy, String> {
    Privacy::ALL
        .into_iter()
        .find(|p| p.as_str() == value)
        .ok_or_else(|| "expected one of: private, unlisted, public".to_owned())
}

/// Renders and checks every cut, refuses on problems unless `--force`, then uploads the main cut
/// and the Shorts, keeps X clips as files, moves the card to shipped and tags the release.
pub fn run_ship(args: ShipArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let transcripts = transcripts(&args.video, &project)?;
    // Everything a real ship needs besides the renders is checked before the minutes of
    // rendering: the card to move and the OAuth client.
    let channel = Channel::of(&args.video);
    let client = Client::load(&oauth::client_path());
    if !args.dry_run {
        channel.as_ref().map_err(ToString::to_string)?;
        client.as_ref().map_err(ToString::to_string)?;
    }
    let options = Options {
        privacy: args.privacy,
        made_for_kids: args.made_for_kids,
    };
    let interactive = std::io::stderr().is_terminal();
    let progress = |step: Step| match step {
        Step::Rendering {
            cut,
            rendered,
            to_render,
        } if interactive => {
            eprint!("\rrendering {cut}: {rendered}/{to_render} chunks");
            let _ = std::io::stderr().flush();
        }
        Step::Checking { cut } => {
            if interactive {
                eprintln!();
            }
            eprintln!("checking {cut}");
        }
        _ => {}
    };
    let prepared = ship::prepare(
        &args.video,
        &project,
        &transcripts,
        options,
        args.jobs,
        &progress,
    )
    .map_err(|e| e.to_string())?;
    for (cut, problem) in &prepared.problems {
        println!("{cut}: {problem}");
    }
    print_plan(&args.video, &prepared.cuts, channel.as_ref().ok());
    let problems = prepared.problems.len();
    if args.dry_run {
        if let Err(e) = &channel {
            println!("a real ship stops here: {e}");
        }
        if let Err(e) = &client {
            println!("a real ship stops here: {e}");
        }
        if problems > 0 && !args.force {
            println!("a real ship stops here: {problems} check problems, fix them or pass --force");
        }
        println!("dry run: nothing uploaded, nothing tagged");
        return Ok(());
    }
    if problems > 0 && !args.force {
        return Err(format!(
            "{problems} check problems, fix them or pass --force to ship anyway"
        ));
    }
    let (channel, client) = (channel.expect("checked"), client.expect("checked"));
    let store = Box::new(Keychain::new(&client));
    let mut youtube = YouTube::new(client, store, Endpoints::default());
    let fail = |e: YouTubeError| e.to_string();
    if !youtube.signed_in().map_err(fail)? {
        sign_in(&mut youtube)?;
    }
    let mut step = print_step(interactive);
    let shipped = match ship::publish(
        &args.video,
        &channel,
        &prepared.cuts,
        &mut youtube,
        ship::today(),
        &mut step,
    ) {
        // The record keeps what already went up, so the second run continues from there.
        Err(ShipError::YouTube(YouTubeError::Auth(AuthError::Expired))) => {
            sign_in(&mut youtube)?;
            ship::publish(
                &args.video,
                &channel,
                &prepared.cuts,
                &mut youtube,
                ship::today(),
                &mut step,
            )
        }
        result => result,
    }
    .map_err(|e| e.to_string())?;
    println!(
        "shipped {} as {} ({})",
        channel.slug,
        shipped.tag,
        args.privacy.as_str()
    );
    Ok(())
}

fn sign_in(youtube: &mut YouTube) -> Result<(), String> {
    eprintln!("signing in to YouTube in the browser");
    youtube
        .sign_in(&mut |url| {
            eprintln!("if no browser opens, visit {url}");
            Command::new("open").arg(url).status().map(drop)
        })
        .map_err(|e| e.to_string())
}

fn print_step(interactive: bool) -> impl FnMut(Step) {
    move |step| match step {
        Step::Uploading { cut, sent } if interactive => {
            eprint!(
                "\ruploading {cut}: {:.0}%",
                sent.bytes as f64 * 100.0 / sent.total.max(1) as f64
            );
            let _ = std::io::stderr().flush();
        }
        Step::Uploaded { cut, video_id } => {
            if interactive {
                eprintln!();
            }
            println!(
                "uploaded {cut}: {}",
                shot_ship::youtube::watch_url(&video_id)
            );
        }
        Step::AlreadyUploaded { cut, video_id } => println!(
            "{cut} was uploaded before: {}",
            shot_ship::youtube::watch_url(&video_id)
        ),
        Step::Thumbnail { cut } => println!("set the thumbnail of {cut}"),
        Step::ThumbnailRefused { cut, reason } => {
            println!("the thumbnail of {cut} was not set, set it in YouTube Studio: {reason}")
        }
        Step::File { cut, path } => println!("{cut} ships as a file: {}", path.display()),
        Step::Moved { slug } => println!("moved {slug} to shipped"),
        Step::Tagged { tag } => println!("tagged {tag}"),
        _ => {}
    }
}

fn print_plan(video: &Path, cuts: &[PlannedCut], channel: Option<&Channel>) {
    let relative = |path: &Path| {
        path.strip_prefix(video)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    for cut in cuts {
        let (w, h) = cut.kind.aspect();
        let kind = match cut.kind {
            OutputKind::Main => "main".to_owned(),
            OutputKind::Derived(kind) => format!("{} {}", kind.as_str(), cut.name),
        };
        let size = std::fs::metadata(&cut.export).map_or(0, |m| m.len());
        let to = match (&cut.destination, &cut.metadata) {
            (Destination::YouTube, Some(m)) => format!(
                "YouTube, {}, {}",
                m.privacy.as_str(),
                if m.made_for_kids {
                    "made for kids"
                } else {
                    "not made for kids"
                }
            ),
            _ => "a file to post by hand".to_owned(),
        };
        println!(
            "\n{kind}  {w}:{h}  {}  {} ({:.1} MB) to {to}",
            shot_ship::clock(cut.length),
            relative(&cut.export),
            size as f64 / 1e6
        );
        if let Some(metadata) = &cut.metadata {
            println!("  title: {}", metadata.title);
            for problem in metadata.problems() {
                println!("  refused by YouTube: {problem}");
            }
            for line in metadata.description.lines() {
                println!("  | {line}");
            }
        }
        match (&cut.kind, &cut.thumbnail) {
            (OutputKind::Main, Some(still)) => println!("  thumbnail: {}", relative(still)),
            (OutputKind::Main, None) => println!("  thumbnail: none, YouTube picks a frame"),
            _ => {}
        }
    }
    if let Some(channel) = channel {
        println!(
            "\nthen: move {} to shipped and tag the release v{} (or its next .N)",
            channel.slug,
            {
                let d = ship::today();
                format!("{}.{}.{}", d.year() % 100, d.month(), d.day())
            }
        );
    }
}

fn transcripts(video: &Path, project: &Project) -> Result<HashMap<Id, Transcript>, String> {
    let mut found = HashMap::new();
    for take in project
        .cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|t| &t.take)
    {
        if !found.contains_key(take)
            && let Some(transcript) =
                shot_analyze::cache::read_cached(video, take).map_err(|e| e.to_string())?
        {
            found.insert(take.clone(), transcript);
        }
    }
    Ok(found)
}
