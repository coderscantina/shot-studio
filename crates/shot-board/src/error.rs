use shot_core::ParseError;

/// Why a card's `video.kdl` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CardError {
    #[error("videos/{slug}/video.kdl does not exist")]
    Missing { slug: String },
    #[error("videos/{slug}/{error}")]
    Parse { slug: String, error: ParseError },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("the repo has no default branch with a commit (looked for main and master)")]
    NoDefaultBranch,
    #[error("no video `{0}` in this channel")]
    UnknownVideo(String),
    #[error("`{0}` is not a valid name, use lowercase letters, digits and dashes")]
    InvalidName(String),
    #[error("`{0}` is not a path inside the video folder")]
    InvalidPath(String),
    #[error(transparent)]
    Card(#[from] CardError),
    #[error("a proposal needs at least one changed file")]
    EmptyProposal,
    #[error("proposal `{name}` for `{slug}` already exists")]
    ProposalExists { slug: String, name: String },
    #[error("no proposal `{name}` for `{slug}`")]
    UnknownProposal { slug: String, name: String },
    #[error("accepting the proposal conflicts in: {}", paths.join(", "))]
    Conflict { paths: Vec<String> },
    #[error("`{0}` is not shipped yet")]
    NotShipped(String),
    #[error("release tag `{0}` already exists")]
    TagExists(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
