//! Walking a folder. The batch case is the normal case: nobody cleans one file
//! at a time before sending a directory of photographs.

use crate::format;
use crate::model::FormatId;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub format: FormatId,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    pub recursive: bool,
    /// Include files whose name begins with a dot.
    pub hidden: bool,
    /// Skip anything larger than this, in bytes. Zero means no limit.
    pub max_bytes: u64,
}

impl ScanOptions {
    pub fn recursive() -> Self {
        ScanOptions { recursive: true, ..Default::default() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    /// Files that were looked at and are not a format this tool handles. Shown,
    /// not hidden: a folder run that silently skipped a RAW file would be a lie.
    pub skipped: Vec<PathBuf>,
}

/// Sniff one path. Reads only the first few kilobytes.
pub fn candidate(path: &Path) -> Option<Candidate> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let head = read_head(path)?;
    let ext = format::extension(path);
    let handler = format::for_bytes(&head, ext.as_deref())?;
    Some(Candidate { path: path.to_path_buf(), format: handler.id(), bytes: meta.len() })
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

pub fn scan(root: &Path, options: &ScanOptions) -> Scan {
    let mut out = Scan::default();
    let depth = if options.recursive { usize::MAX } else { 1 };
    let walker = WalkDir::new(root).max_depth(depth).follow_links(false).sort_by_file_name();
    for entry in walker.into_iter().filter_entry(|e| options.hidden || !is_hidden(e.path())) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        // Our own partial writes are never input.
        if path.extension().is_some_and(|e| e == "fm-part") {
            continue;
        }
        match candidate(path) {
            Some(c) => {
                if options.max_bytes > 0 && c.bytes > options.max_bytes {
                    out.skipped.push(path.to_path_buf());
                } else {
                    out.candidates.push(c);
                }
            }
            None => out.skipped.push(path.to_path_buf()),
        }
    }
    out
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

impl Scan {
    pub fn total_bytes(&self) -> u64 {
        self.candidates.iter().map(|c| c.bytes).sum()
    }

    /// Counts per format, for the "what am I about to do" line.
    pub fn by_format(&self) -> Vec<(FormatId, usize)> {
        let mut out: Vec<(FormatId, usize)> = Vec::new();
        for c in &self.candidates {
            match out.iter_mut().find(|(f, _)| *f == c.format) {
                Some(slot) => slot.1 += 1,
                None => out.push((c.format, 1)),
            }
        }
        out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        out
    }

    pub fn summary(&self) -> String {
        if self.candidates.is_empty() {
            return format!("Nothing to clean here ({} file(s) skipped).", self.skipped.len());
        }
        let parts: Vec<String> = self
            .by_format()
            .iter()
            .map(|(f, n)| format!("{n} {}", f.noun()))
            .collect();
        format!("{} ({} skipped)", parts.join(", "), self.skipped.len())
    }
}
