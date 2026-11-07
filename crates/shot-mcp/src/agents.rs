//! What the app asks Claude: the script agent's commands and "Plan the week with Claude", as
//! prompts over the `shot` MCP tools. The CLI and the app send the same words.

/// Appended to Claude Code's system prompt for every bridge session.
pub const SYSTEM: &str = "You work inside Shot Studio through the `shot` MCP tools only. \
Read with them, and change a video only by proposing: propose_script for script.md, propose_cut \
for cuts. A proposal is a branch the person accepts or rejects, so never ask for confirmation, \
just propose. Name proposals with lowercase words and dashes. Keep the speaker's voice: read \
their other scripts with read_video before writing. Never invent facts; say what you are unsure \
about. End with two or three sentences on what you proposed and why, in plain text without \
Markdown.";

/// A request to the script agent, typed in the Script screen's prompt box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptCommand {
    /// Draft or redraft the script from a brief.
    Draft {
        brief: String,
    },
    /// Three alternative hooks, as proposals `hook-a`, `hook-b` and `hook-c`.
    Hooks,
    Tighten,
    MoreOpinion,
    BrollNotes,
    FactCheck,
    /// Another version of a proposal the person rejected, with the diff it had.
    AnotherTake {
        proposal: String,
        diff: String,
    },
    /// Free text.
    Ask(String),
}

impl ScriptCommand {
    /// The slash commands the Script screen offers, in order.
    pub const SLASH: [&str; 4] = ["/tighten", "/more opinion", "/b-roll notes", "/fact check"];

    /// Reads a slash command, anything else is free text.
    pub fn parse(input: &str) -> Self {
        let input = input.trim();
        match input.to_lowercase().as_str() {
            "/tighten" => Self::Tighten,
            "/more opinion" => Self::MoreOpinion,
            "/b-roll notes" => Self::BrollNotes,
            "/fact check" => Self::FactCheck,
            "/hooks" => Self::Hooks,
            _ => Self::Ask(input.to_owned()),
        }
    }

    /// The prompt for video `slug`.
    pub fn prompt(&self, slug: &str) -> String {
        let task = match self {
            Self::Draft { brief } => format!(
                "Draft the script from this brief:\n\n{brief}\n\nRead the video and its \
                 sources first. Then propose the whole script as `draft`, and three hooks for \
                 it as hook-a, hook-b and hook-c, each the same script with a different first \
                 beat."
            ),
            Self::Hooks => "Propose three alternative hooks as hook-a, hook-b and hook-c: each \
                            is the current script with only the first chapter's first beat \
                            rewritten. Make them differ in approach, not in wording."
                .to_owned(),
            Self::Tighten => "Tighten the script: cut repetition and filler, keep every point \
                              and source. Propose it as `tighten`."
                .to_owned(),
            Self::MoreOpinion => "Make the script more opinionated: state the speaker's take \
                                  plainly where it hedges, without adding claims the sources \
                                  do not support. Propose it as `more-opinion`."
                .to_owned(),
            Self::BrollNotes => "Run script_checks, then add `B-roll: ...` beats where a \
                                 stretch has no visual, describing what to show from the \
                                 sources. Propose it as `b-roll-notes`."
                .to_owned(),
            Self::FactCheck => "Fact check the script against its sources. Do not propose \
                                anything; list each claim that the sources do not support, \
                                with the beat it is in."
                .to_owned(),
            Self::AnotherTake { proposal, diff } => format!(
                "The person rejected proposal `{proposal}`, which changed this:\n\n{diff}\n\
                 Propose a different take on the same change as `{proposal}-2`, or the next \
                 free number."
            ),
            Self::Ask(text) => text.clone(),
        };
        format!("Video `{slug}`.\n\n{task}")
    }
}

/// "Plan the week with Claude": which ideas to script next against the cadence.
pub fn plan_week(cadence_per_week: u8) -> String {
    format!(
        "Plan this week's videos. The channel publishes {cadence_per_week} per week. List the \
         videos with list_videos and read the ideas and their sources with read_video. Pick \
         which ideas to script next and say why in one line each, considering target dates, \
         what is already in progress and variety of formats. Move the picked ideas to \
         scripting with move_card. End with the week as a short list."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_commands_parse_and_name_their_proposal() {
        assert_eq!(ScriptCommand::parse(" /tighten "), ScriptCommand::Tighten);
        assert_eq!(
            ScriptCommand::parse("make the close warmer"),
            ScriptCommand::Ask("make the close warmer".to_owned())
        );
        for command in ScriptCommand::SLASH {
            assert_ne!(
                ScriptCommand::parse(command),
                ScriptCommand::Ask(command.to_owned())
            );
        }
        assert!(
            ScriptCommand::BrollNotes
                .prompt("friday")
                .starts_with("Video `friday`.")
        );
    }
}
