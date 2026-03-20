use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum VeloError {
    NotARepo,
    AlreadyInitialized,
    NestedRepo(PathBuf),
    InvalidInput(String),
    CorruptRepo(String),
    Io(std::io::Error),
    Db(rusqlite::Error),
}

impl fmt::Display for VeloError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VeloError::NotARepo => write!(
                f,
                "Not a Velo repository. Run 'velo init' to initialize one here."
            ),
            VeloError::AlreadyInitialized => {
                write!(f, "Repository already initialized in this directory.")
            }
            VeloError::NestedRepo(p) => write!(
                f,
                "Already inside a Velo repository at '{}'. Nested repositories are not supported.",
                p.display()
            ),
            VeloError::InvalidInput(s) => write!(f, "{}", s),
            VeloError::CorruptRepo(s) => write!(f, "Corrupt repository: {}", s),
            VeloError::Io(e) => write!(f, "I/O error: {}", e),
            VeloError::Db(e) => write!(f, "Database error: {}", e),
        }
    }
}

impl std::error::Error for VeloError {}

impl From<std::io::Error> for VeloError {
    fn from(e: std::io::Error) -> Self {
        VeloError::Io(e)
    }
}

impl From<rusqlite::Error> for VeloError {
    fn from(e: rusqlite::Error) -> Self {
        VeloError::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, VeloError>;
