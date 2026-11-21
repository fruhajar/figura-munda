use crate::model::FormatId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a format this tool handles")]
    Unsupported,
    #[error("{format:?} file is malformed: {detail}")]
    Malformed { format: FormatId, detail: String },
    #[error("refusing to touch this file: {0}")]
    Refused(String),
    #[error("cleaning would not have removed {0}")]
    VerifyFailed(String),
    #[error(
        "cleaning would have changed something the report never mentioned: {}. \
         That is a defect in this tool, not in your file, and nothing was written.",
        .0.join(", ")
    )]
    Unreported(Vec<String>),
    #[error("{0}")]
    Profile(#[from] crate::profile::ProfileError),
}

impl Error {
    pub fn malformed(format: FormatId, detail: impl Into<String>) -> Self {
        Error::Malformed { format, detail: detail.into() }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
