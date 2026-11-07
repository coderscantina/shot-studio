//! `shot models fetch|list`: the pinned models of decision 0008.

use std::io::Write;
use std::path::PathBuf;

use clap::Subcommand;
use shot_analyze::models::{self, ModelSpec, ModelState, Progress};

#[derive(Subcommand)]
pub enum ModelsAction {
    /// Download every missing model, verifying its checksum.
    Fetch,
    /// Show which models are present.
    List,
}

pub fn run(action: ModelsAction) -> Result<(), String> {
    let dir = models::models_dir();
    match action {
        ModelsAction::List => {
            println!("{}", dir.display());
            for model in models::manifest() {
                let state = match model.state(&dir) {
                    ModelState::Present => "present".to_owned(),
                    ModelState::Partial(done) => format!("partial, {}", percent(done, model.size)),
                    ModelState::Missing => "missing".to_owned(),
                };
                println!("  {:<24} {:>8}  {state}", model.name, megabytes(model.size));
            }
        }
        ModelsAction::Fetch => {
            for model in models::manifest() {
                let path = ensure(&model)?;
                println!("{} {}", model.name, path.display());
            }
        }
    }
    Ok(())
}

/// Returns the model's path, downloading it with progress on stderr when missing.
pub fn ensure(model: &ModelSpec) -> Result<PathBuf, String> {
    let mut shown = None;
    let path = model
        .ensure(&models::models_dir(), |Progress { done, total }| {
            let step = done * 100 / total.max(1);
            if shown != Some(step) {
                shown = Some(step);
                eprint!("\r{} {}", model.name, percent(done, total));
                let _ = std::io::stderr().flush();
            }
        })
        .map_err(|e| e.to_string())?;
    if shown.is_some() {
        eprintln!();
    }
    Ok(path)
}

fn percent(done: u64, total: u64) -> String {
    format!("{}%", done * 100 / total.max(1))
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000))
}
