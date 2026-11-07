//! `shot ship`: render and check every cut, upload the main cut and the Shorts to YouTube, keep
//! the X clips as files, then move the card to `shipped` and tag the release.
//!
//! [`plan`] says what ships where, [`prepare`] (macOS) renders and checks it, and [`publish`]
//! uploads and tags. Each upload step lands in the [`Record`] as it happens, so a run that
//! stops halfway continues where it stopped and never uploads a cut twice.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use shot_board::Board;
use shot_core::{Date, DerivedKind, Id, Micros, OutputKind, Project, Status, Timeline, Transcript};

use crate::describe::describe;
use crate::record::Record;
use crate::spoken::duration;
use crate::youtube::{self, Metadata, Privacy, Sent, YouTube, YouTubeError};

/// Exports, caption sidecars and the thumbnail, all next to each other.
pub const EXPORT_DIR: &str = ".shot/export";

/// `.shot/export/<cut>.mp4`, where `shot export` and `shot ship` write a cut.
pub fn export_path(video_dir: &Path, cut: &str) -> PathBuf {
    video_dir.join(EXPORT_DIR).join(format!("{cut}.mp4"))
}

/// The main cut's thumbnail, `.shot/export/main.png` (or `.jpg`), when one was picked.
pub fn thumbnail(video_dir: &Path) -> Option<PathBuf> {
    let export = export_path(video_dir, "main");
    ["png", "jpg"]
        .map(|ext| export.with_extension(ext))
        .into_iter()
        .find(|p| p.exists())
}

/// Makes `still` the main cut's thumbnail.
pub fn pick_thumbnail(video_dir: &Path, still: &Path) -> io::Result<PathBuf> {
    let to = export_path(video_dir, "main").with_extension("png");
    fs::create_dir_all(to.parent().expect("has a parent"))?;
    let other = to.with_extension("jpg");
    if other.exists() {
        fs::remove_file(other)?;
    }
    fs::copy(still, &to)?;
    Ok(to)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    YouTube,
    /// Exported for posting by hand, such as an X clip.
    File,
}

/// One cut and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCut {
    pub name: String,
    pub kind: OutputKind,
    pub length: Micros,
    pub export: PathBuf,
    pub destination: Destination,
    /// For YouTube uploads.
    pub metadata: Option<Metadata>,
    /// The main cut's custom thumbnail.
    pub thumbnail: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub privacy: Privacy,
    pub made_for_kids: bool,
}

/// Every cut of the project in timeline order: main first, then the derived cuts.
pub fn plan(
    video_dir: &Path,
    project: &Project,
    transcripts: &HashMap<Id, Transcript>,
    options: Options,
) -> Vec<PlannedCut> {
    let description = describe(project, transcripts);
    let hook = project
        .script
        .chapters
        .first()
        .and_then(|c| c.beats.first())
        .map(|b| b.text.trim().to_owned())
        .unwrap_or_default();
    Timeline::evaluate(project)
        .outputs
        .iter()
        .map(|output| {
            let destination = match output.kind {
                OutputKind::Main | OutputKind::Derived(DerivedKind::Short) => Destination::YouTube,
                OutputKind::Derived(DerivedKind::XClip) => Destination::File,
            };
            let text = match output.kind {
                OutputKind::Main => description.text.trim_end().to_owned(),
                _ => hook.clone(),
            };
            PlannedCut {
                name: output.name.clone(),
                kind: output.kind,
                length: duration(output),
                export: export_path(video_dir, &output.name),
                destination,
                metadata: (destination == Destination::YouTube).then(|| Metadata {
                    title: description.title.clone(),
                    description: text,
                    privacy: options.privacy,
                    made_for_kids: options.made_for_kids,
                }),
                thumbnail: (output.kind == OutputKind::Main)
                    .then(|| thumbnail(video_dir))
                    .flatten(),
            }
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    #[error(
        "{} is not a video folder of a channel repo (<repo>/videos/<slug>): there is no card to \
         move and no repo to tag",
        .0.display()
    )]
    NoChannel(PathBuf),
    #[error(transparent)]
    Board(#[from] shot_board::Error),
    #[error(transparent)]
    YouTube(#[from] YouTubeError),
    #[error("{cut}: {message}")]
    Metadata { cut: String, message: String },
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{0}")]
    Render(String),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ShipError + '_ {
    move |source| ShipError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The channel repo a video folder belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub repo: PathBuf,
    pub slug: String,
}

impl Channel {
    /// The repo and slug of `<repo>/videos/<slug>`, checked to open as a board with that card.
    pub fn of(video_dir: &Path) -> Result<Self, ShipError> {
        let none = || ShipError::NoChannel(video_dir.to_owned());
        let dir = std::path::absolute(video_dir).map_err(|_| none())?;
        let slug = dir.file_name().and_then(|s| s.to_str()).ok_or_else(none)?;
        let videos = dir.parent().ok_or_else(none)?;
        if videos.file_name() != Some("videos".as_ref()) {
            return Err(none());
        }
        let repo = videos.parent().ok_or_else(none)?.to_owned();
        let board = Board::open(&repo).map_err(|_| none())?;
        if !board.cards()?.iter().any(|c| c.slug == slug) {
            return Err(shot_board::Error::UnknownVideo(slug.to_owned()).into());
        }
        Ok(Self {
            repo,
            slug: slug.to_owned(),
        })
    }
}

/// What a ship run is doing, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Rendering {
        cut: String,
        rendered: usize,
        to_render: usize,
    },
    Checking {
        cut: String,
    },
    Uploading {
        cut: String,
        sent: Sent,
    },
    Uploaded {
        cut: String,
        video_id: String,
    },
    /// A cut an earlier run already uploaded.
    AlreadyUploaded {
        cut: String,
        video_id: String,
    },
    Thumbnail {
        cut: String,
    },
    ThumbnailRefused {
        cut: String,
        reason: String,
    },
    /// A cut that ships as a file.
    File {
        cut: String,
        path: PathBuf,
    },
    Moved {
        slug: String,
    },
    Tagged {
        tag: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shipped {
    /// Cut name and video id of every YouTube upload.
    pub videos: Vec<(String, String)>,
    pub files: Vec<PathBuf>,
    pub tag: String,
}

/// Uploads the YouTube cuts (main first, so the Shorts can link to it), sets the thumbnail,
/// then moves the card to `shipped` and tags the release on `today`. Cuts the record already
/// holds are skipped, and the record is written after every step.
pub fn publish(
    video_dir: &Path,
    channel: &Channel,
    cuts: &[PlannedCut],
    youtube: &mut YouTube,
    today: Date,
    step: &mut dyn FnMut(Step),
) -> Result<Shipped, ShipError> {
    for cut in cuts {
        if let Some(metadata) = &cut.metadata
            && let Some(message) = metadata.problems().into_iter().next()
        {
            return Err(ShipError::Metadata {
                cut: cut.name.clone(),
                message,
            });
        }
        if !cut.export.exists() {
            return Err(io_error(&cut.export)(io::ErrorKind::NotFound.into()));
        }
    }
    let record_path = Record::path(video_dir);
    let mut record = Record::load(video_dir).map_err(io_error(&record_path))?;
    let save = |record: &Record| record.save(video_dir).map_err(io_error(&record_path));
    let mut shipped = Shipped {
        videos: Vec::new(),
        files: Vec::new(),
        tag: String::new(),
    };
    for cut in cuts {
        let Some(metadata) = &cut.metadata else {
            step(Step::File {
                cut: cut.name.clone(),
                path: cut.export.clone(),
            });
            shipped.files.push(cut.export.clone());
            continue;
        };
        let video_id = match record.video_id(&cut.name) {
            Some(id) => {
                step(Step::AlreadyUploaded {
                    cut: cut.name.clone(),
                    video_id: id.to_owned(),
                });
                id.to_owned()
            }
            None => {
                let mut metadata = metadata.clone();
                if cut.kind != OutputKind::Main
                    && let Some(main) = record.video_id("main")
                {
                    let link = format!("Full video: {}", youtube::watch_url(main));
                    metadata.description = [metadata.description.as_str(), &link]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                }
                let size = fs::metadata(&cut.export)
                    .map_err(io_error(&cut.export))?
                    .len();
                let entry = record.uploads.entry(cut.name.clone()).or_default();
                let resume = entry.session.clone().filter(|_| entry.size == size);
                entry.size = size;
                entry.privacy = metadata.privacy;
                let mut saved = Ok(());
                let id = youtube.upload(
                    &cut.export,
                    &metadata,
                    resume.as_deref(),
                    &mut |uri| {
                        let entry = record.uploads.get_mut(&cut.name).expect("just added");
                        entry.session = Some(uri.to_owned());
                        if saved.is_ok() {
                            saved = save(&record);
                        }
                    },
                    &mut |sent| {
                        step(Step::Uploading {
                            cut: cut.name.clone(),
                            sent,
                        })
                    },
                )?;
                saved?;
                let entry = record.uploads.get_mut(&cut.name).expect("just added");
                entry.video_id = Some(id.clone());
                entry.session = None;
                save(&record)?;
                step(Step::Uploaded {
                    cut: cut.name.clone(),
                    video_id: id.clone(),
                });
                id
            }
        };
        if let Some(still) = &cut.thumbnail
            && !record.uploads[&cut.name].thumbnail
        {
            let (image, mime) = thumbnail_image(still)?;
            match youtube.set_thumbnail(&video_id, &image, mime) {
                Ok(()) => {
                    record
                        .uploads
                        .get_mut(&cut.name)
                        .expect("uploaded above")
                        .thumbnail = true;
                    save(&record)?;
                    step(Step::Thumbnail {
                        cut: cut.name.clone(),
                    });
                }
                // A channel that is not verified may not set custom thumbnails. The video is
                // up, so the release goes on and the next run tries the thumbnail again.
                Err(e @ YouTubeError::Refused { .. }) => step(Step::ThumbnailRefused {
                    cut: cut.name.clone(),
                    reason: e.to_string(),
                }),
                Err(e) => return Err(e.into()),
            }
        }
        shipped.videos.push((cut.name.clone(), video_id));
    }
    let board = Board::open(&channel.repo)?;
    if board.move_card(&channel.slug, Status::Shipped)?.is_some() {
        step(Step::Moved {
            slug: channel.slug.clone(),
        });
    }
    let tag = match &record.release {
        Some(tag) => tag.clone(),
        None => {
            let tag = board.tag_release(&channel.slug, today)?.name;
            record.release = Some(tag.clone());
            save(&record)?;
            step(Step::Tagged { tag: tag.clone() });
            tag
        }
    };
    shipped.tag = tag;
    Ok(shipped)
}

/// The thumbnail as YouTube takes it: the file itself when it fits in 2 MB, otherwise scaled to
/// 1280x720 and written as JPEG.
fn thumbnail_image(path: &Path) -> Result<(Vec<u8>, &'static str), ShipError> {
    let bytes = fs::read(path).map_err(io_error(path))?;
    let jpeg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if bytes.len() <= youtube::THUMBNAIL_MAX {
        return Ok((bytes, if jpeg { "image/jpeg" } else { "image/png" }));
    }
    let bad = |e: image::ImageError| ShipError::Io {
        path: path.to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidData, e),
    };
    let image = image::load_from_memory(&bytes).map_err(bad)?.resize(
        1280,
        720,
        image::imageops::FilterType::Lanczos3,
    );
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&image.to_rgb8())
        .map_err(bad)?;
    Ok((out, "image/jpeg"))
}

/// Today's date on this machine's clock and time zone, the release date of a tag.
pub fn today() -> Date {
    // SAFETY: `time` and `localtime_r` only write the values passed to them.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::zeroed::<libc::tm>();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    Date::new(
        (tm.tm_year + 1900) as u16,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
    )
    .expect("the clock gives a valid date")
}

/// The renders and check results of every cut, ready for [`publish`].
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct Prepared {
    pub cuts: Vec<PlannedCut>,
    /// Problems `shot check` found, by cut.
    pub problems: Vec<(String, crate::check::Problem)>,
}

/// Renders every cut to its export path, reusing cached chunks, and runs `shot check` on each.
#[cfg(target_os = "macos")]
pub fn prepare(
    video_dir: &Path,
    project: &Project,
    transcripts: &HashMap<Id, Transcript>,
    options: Options,
    jobs: usize,
    step: &(dyn Fn(Step) + Sync),
) -> Result<Prepared, ShipError> {
    use shot_render::export::{ExportOptions, export};
    let cuts = plan(video_dir, project, transcripts, options);
    let mut problems = Vec::new();
    for cut in &cuts {
        let progress = |p: shot_render::export::Progress| {
            step(Step::Rendering {
                cut: cut.name.clone(),
                rendered: p.rendered,
                to_render: p.to_render,
            })
        };
        export(
            video_dir,
            project,
            &cut.name,
            &cut.export,
            ExportOptions { jobs },
            &progress,
        )
        .map_err(|e| ShipError::Render(format!("{}: {e}", cut.name)))?;
        step(Step::Checking {
            cut: cut.name.clone(),
        });
        let found = crate::media::check_cut(video_dir, project, &cut.name, &cut.export)
            .map_err(|e| ShipError::Render(format!("{}: {e}", cut.name)))?;
        problems.extend(found.into_iter().map(|p| (cut.name.clone(), p)));
    }
    Ok(Prepared { cuts, problems })
}
