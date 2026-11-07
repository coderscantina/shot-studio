//! `shot caption-fix`: add, change or remove a caption fix in `cut.kdl`.

use std::fs;
use std::path::PathBuf;

use clap::Args;
use shot_core::{CaptionFix, Cut};

#[derive(Args)]
pub struct CaptionFixArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The words as the transcript has them, matched ignoring case.
    heard: String,
    /// What captions show instead. Leave out to remove the fix.
    said: Option<String>,
}

pub fn run(args: CaptionFixArgs) -> Result<(), String> {
    let path = args.video.join("cut.kdl");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cut = Cut::parse(&text).map_err(|e| format!("cut.kdl:{e}"))?;
    let heard = args.heard.trim().to_owned();
    if heard.is_empty() {
        return Err("the heard words are empty".into());
    }
    let same = |fix: &CaptionFix| fix.heard.to_lowercase() == heard.to_lowercase();
    match args.said.map(|s| s.trim().to_owned()) {
        Some(said) if said.is_empty() => return Err("the replacement is empty".into()),
        Some(said) => match cut.caption_fixes.iter_mut().find(|f| same(f)) {
            Some(fix) => fix.said = said,
            None => cut.caption_fixes.push(CaptionFix { heard, said }),
        },
        None => {
            let before = cut.caption_fixes.len();
            cut.caption_fixes.retain(|f| !same(f));
            if cut.caption_fixes.len() == before {
                return Err(format!("no caption fix for \"{heard}\""));
            }
        }
    }
    let tmp = args.video.join(".cut.kdl.tmp");
    fs::write(&tmp, cut.to_kdl())
        .and_then(|()| fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for fix in &cut.caption_fixes {
        println!("{:?} -> {:?}", fix.heard, fix.said);
    }
    Ok(())
}
