//! The seam between the file tools and where the files are. `LocalBackend` reads this machine. A remote backend (over ssh)
//! would implement the same trait later; that is not built yet. The arguments are checked by the tool layer (tool.rs)
//! before a backend sees them.

/// `read_file`. `offset` is the first line to show, 1-based. `limit` is the most lines to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    pub path: String,
    pub offset: usize,
    pub limit: usize,
}

/// `list_dir`. No path is the local directory. `depth` is the number of levels to list (1 is the folder itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRequest {
    pub path: Option<String>,
    pub depth: usize,
}

/// `glob`. No path is the local directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobRequest {
    pub pattern: String,
    pub path: Option<String>,
}

/// `grep`. No path is the local directory. `glob` limits the files searched by name. `max_results` is already capped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepRequest {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub case_insensitive: bool,
    pub max_results: usize,
}

/// The read-only file operations. Each returns the text the model reads, or the error text it reads.
pub trait FsBackend {
    fn read_file(&self, req: &ReadRequest) -> Result<String, String>;
    fn list_dir(&self, req: &ListRequest) -> Result<String, String>;
    fn glob(&self, req: &GlobRequest) -> Result<String, String>;
    fn grep(&self, req: &GrepRequest) -> Result<String, String>;
}
