//! What the filesystem keeps about a file, which no amount of cleaning inside
//! the file will touch.
//!
//! The download-origin marks matter most: Windows writes a `Zone.Identifier`
//! alternate data stream recording which URL a file came from, and desktop
//! Linux writes the same thing into `user.xdg.origin.url`. Both survive being
//! attached to an email.

use crate::model::{Action, Class, Finding, Impact};
use crate::profile::Profile;
use crate::timeutil::Civil;
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The two shapes a Windows zone mark takes on a filesystem that has no
/// alternate data streams: a `:Zone.Identifier` suffix, and the `.Identifier`
/// sidecar that tools emit when copying one off NTFS.
fn zone_paths(path: &Path) -> Vec<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    vec![
        dir.join(format!("{name}:Zone.Identifier")),
        dir.join(format!("{name}.Identifier")),
    ]
}

const ORIGIN_XATTRS: &[&str] = &["user.xdg.origin.url", "user.xdg.referrer.url"];

fn civil_of(t: SystemTime) -> Option<Civil> {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    Some(Civil::from_unix_seconds(secs as i64))
}

pub fn inspect(path: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Ok(meta) = fs::metadata(path) {
        if let Some(c) = meta.modified().ok().and_then(civil_of) {
            out.push(
                Finding::new(Class::Filesystem, "fs:mtime", "last modification time, kept by the filesystem")
                    .value(c.to_iso()),
            );
        }
        if let Some(c) = meta.created().ok().and_then(civil_of) {
            out.push(
                Finding::new(Class::Filesystem, "fs:birthtime", "creation time, kept by the filesystem")
                    .value(c.to_iso())
                    .advisory()
                    .note(
                        "This becomes the moment of cleaning whichever profile is used, because \
                         writing a new file is what cleaning is.",
                    ),
            );
        }
    }
    for p in zone_paths(path) {
        if let Ok(body) = fs::read_to_string(&p) {
            out.push(
                Finding::new(
                    Class::Filesystem,
                    "fs:zone-identifier",
                    "a record that this file was downloaded, and from where",
                )
                .value(body.trim().replace('\n', " · "))
                .note("Windows writes this beside the file; it travels with a copy."),
            );
        }
    }
    #[cfg(unix)]
    for (name, value) in list_xattrs(path) {
        let is_origin = ORIGIN_XATTRS.contains(&name.as_str());
        let label = if is_origin {
            "extended attribute recording where the file was downloaded from"
        } else {
            "extended attribute"
        };
        out.push(
            Finding::new(Class::Filesystem, format!("fs:xattr:{name}"), label)
                .value(if value.is_empty() { name.clone() } else { format!("{name} = {value}") }),
        );
    }
    out
}

#[cfg(unix)]
fn list_xattrs(path: &Path) -> Vec<(String, String)> {
    let Ok(names) = xattr::list(path) else { return Vec::new() };
    names
        .map(|n| {
            let name = n.to_string_lossy().to_string();
            let value = xattr::get(path, &n)
                .ok()
                .flatten()
                .map(|v| {
                    if v.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
                        String::from_utf8_lossy(&v).to_string()
                    } else {
                        format!("{} bytes", v.len())
                    }
                })
                .unwrap_or_default();
            (name, value)
        })
        .collect()
}

#[cfg(not(unix))]
fn list_xattrs(_path: &Path) -> Vec<(String, String)> {
    Vec::new()
}

/// Apply the filesystem part of a profile to a file that has just been written.
///
/// Returns the human-readable list of what it did, for the diff.
pub fn apply(path: &Path, profile: &Profile) -> std::io::Result<Vec<String>> {
    let mut done = Vec::new();
    let action = profile.action(Class::Filesystem);
    if action == Action::Keep {
        return Ok(done);
    }

    #[cfg(unix)]
    for (name, _) in list_xattrs(path) {
        if xattr::remove(path, &name).is_ok() {
            done.push(format!("removed the extended attribute {name}"));
        }
    }
    for p in zone_paths(path) {
        if p.exists() && fs::remove_file(&p).is_ok() {
            done.push("removed the download-origin record beside the file".to_string());
        }
    }

    let when = match action {
        // `reset` uses the time of cleaning, which is honest and matches every
        // other file written today. `normalise` uses the shared constant, which
        // is uniform but conspicuous on a filesystem.
        Action::Reset => SystemTime::now(),
        _ => SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(profile.constants.timestamp.unix_seconds().max(0) as u64),
    };
    let f = fs::File::options().write(true).open(path)?;
    f.set_times(FileTimes::new().set_accessed(when).set_modified(when))?;
    done.push(match action {
        Action::Reset => "reset the file's timestamps to now".to_string(),
        _ => format!(
            "set the file's timestamps to {}",
            profile.constants.timestamp.to_iso()
        ),
    });
    Ok(done)
}

/// Copy a source file's timestamps onto a destination, for the case where the
/// profile keeps the filesystem class: a cleaned copy should not silently
/// acquire today's date as its only change.
pub fn carry_times(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(from)?;
    let f = fs::File::options().write(true).open(to)?;
    let mut times = FileTimes::new().set_modified(meta.modified()?);
    if let Ok(a) = meta.accessed() {
        times = times.set_accessed(a);
    }
    f.set_times(times)
}

/// Whether removing filesystem metadata will be visible to the user.
pub fn impact() -> Impact {
    Impact::Metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_paths_cover_both_spellings() {
        let p = Path::new("/tmp/photo.jpg");
        let got: Vec<String> = zone_paths(p).iter().map(|p| p.display().to_string()).collect();
        assert!(got.contains(&"/tmp/photo.jpg:Zone.Identifier".to_string()));
        assert!(got.contains(&"/tmp/photo.jpg.Identifier".to_string()));
    }
}
