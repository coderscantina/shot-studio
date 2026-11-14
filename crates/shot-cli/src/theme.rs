//! `shot theme`: list the channel's themes and use, save, rename or delete one, the same
//! operations the app's theme window and the MCP server's theme tools run
//! ([`shot_mcp::brand`]).

use std::path::Path;

use clap::{Args, Subcommand};
use shot_core::Id;
use shot_mcp::brand;

#[derive(Args)]
pub struct ThemeArgs {
    #[command(subcommand)]
    action: ThemeAction,
}

#[derive(Subcommand)]
enum ThemeAction {
    /// List the themes, the channel's marked, and the overlay presets.
    List,
    /// Make a theme the channel's: one line of brand/brand.kdl changes.
    Use { theme: String },
    /// Save a copy of a theme's look as a new theme.
    Save {
        name: String,
        /// The theme to copy, the channel's by default.
        #[arg(long)]
        from: Option<String>,
    },
    /// Rename a theme, and the channel's `theme` line when it names it.
    Rename { from: String, to: String },
    /// Delete a theme that neither the channel nor a format layer uses.
    Delete { theme: String },
}

fn id(text: &str) -> Result<Id, String> {
    Id::new(text)
        .ok_or_else(|| format!("`{text}` is not a theme id: lowercase letters, digits and dashes"))
}

pub fn run(repo: &Path, args: ThemeArgs) -> Result<(), String> {
    match args.action {
        ThemeAction::List => print!("{}", brand::list_text(&brand::read(repo)?)),
        ThemeAction::Use { theme } => {
            brand::activate(repo, &id(&theme)?)?;
            println!("the channel uses theme {theme}");
        }
        ThemeAction::Save { name, from } => {
            let from = from.as_deref().map(id).transpose()?;
            brand::save(repo, &id(&name)?, from.as_ref())?;
            println!("saved theme {name}");
        }
        ThemeAction::Rename { from, to } => {
            brand::rename(repo, &id(&from)?, &id(&to)?)?;
            println!("renamed theme {from} to {to}");
        }
        ThemeAction::Delete { theme } => {
            brand::delete(repo, &id(&theme)?)?;
            println!("deleted theme {theme}");
        }
    }
    Ok(())
}
