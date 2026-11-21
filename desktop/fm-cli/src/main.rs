//! `fm`, the command-line front end.
//!
//! Inspection is the default and cleaning is opt-in, which is the same order the
//! GUI puts them in: a user who runs `fm` on a folder should learn what is in it
//! before anything is written.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use fm_core::pipeline::{self, Disposition};
use fm_core::profile::Profile;
use fm_core::report::{self, RecipientView, Report, Verification};
use fm_core::scan::{self, ScanOptions};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "fm",
    about = "Shows you what a file reveals about you, then removes it.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report what is in a file or folder, and change nothing.
    Inspect {
        paths: Vec<PathBuf>,
        #[arg(short, long)]
        recursive: bool,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Show what the recipient of the file would see.
        #[arg(long)]
        recipient: bool,
    },
    /// Clean a file or folder according to a profile.
    Clean {
        paths: Vec<PathBuf>,
        /// A built-in profile name, or a path to a profile file.
        #[arg(short, long, default_value = "standard")]
        profile: String,
        /// Write the cleaned files here, keeping their names.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Replace the originals.
        #[arg(long)]
        in_place: bool,
        #[arg(short, long)]
        recursive: bool,
        /// Report what would change and write nothing.
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Proceed even where cleaning removes visible content.
        #[arg(long)]
        allow_content_loss: bool,
        /// Replace files already at the destination. Without this, a
        /// destination holding anything but this file cleaned is refused.
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    /// Re-inspect files and assert they are clean under a profile.
    Verify {
        paths: Vec<PathBuf>,
        #[arg(short, long, default_value = "standard")]
        profile: String,
        #[arg(short, long)]
        recursive: bool,
    },
    /// Show the profiles, or write one out to edit.
    Profile {
        /// Name of a built-in profile. Omit to list them all.
        name: Option<String>,
        /// Write the profile here as a file you can edit.
        #[arg(long)]
        save: Option<PathBuf>,
    },
}

fn load_profile(spec: &str) -> Result<Profile> {
    if let Some(p) = Profile::by_name(spec) {
        return Ok(p);
    }
    let path = Path::new(spec);
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the profile {}", path.display()))?;
        return Profile::from_toml(&text)
            .with_context(|| format!("the profile {} is not usable", path.display()));
    }
    let names: Vec<String> = Profile::builtins().iter().map(|p| p.name.clone()).collect();
    bail!("no profile called `{spec}`. Built in: {}", names.join(", "))
}

/// Everything the user named, expanded through any folders.
fn collect(paths: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        bail!("nothing to do: name a file or a folder");
    }
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for path in paths {
        if path.is_dir() {
            let options = ScanOptions { recursive, ..Default::default() };
            let found = scan::scan(path, &options);
            eprintln!("{}: {}", path.display(), found.summary());
            skipped += found.skipped.len();
            out.extend(found.candidates.into_iter().map(|c| c.path));
        } else if path.is_file() {
            out.push(path.clone());
        } else {
            bail!("{} is not a file or a folder", path.display());
        }
    }
    if out.is_empty() {
        bail!("nothing here this tool handles ({skipped} file(s) skipped)");
    }
    Ok(out)
}

fn destination(source: &Path, out: &Option<PathBuf>, in_place: bool) -> Result<PathBuf> {
    if in_place {
        return Ok(source.to_path_buf());
    }
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let name = source.file_name().context("a file with no name")?;
        return Ok(dir.join(name));
    }
    // Beside the original, which keeps the original available to diff against.
    let stem = source.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let parent = source.parent().unwrap_or(Path::new("."));
    Ok(match source.extension() {
        Some(ext) => parent.join(format!("{stem}-clean.{}", ext.to_string_lossy())),
        None => parent.join(format!("{stem}-clean")),
    })
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Inspect { paths, recursive, json, recipient } => {
            let files = collect(&paths, recursive)?;
            let mut failures = 0;
            for path in &files {
                match pipeline::inspect_file(path) {
                    Ok(ins) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&ins)?);
                        } else {
                            print!("{}", report::text_report(&ins, &path.display().to_string()));
                            if recipient {
                                println!("\n  {}", RecipientView::of(&ins).sentence());
                            }
                            println!();
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", path.display());
                        failures += 1;
                    }
                }
            }
            if failures > 0 {
                bail!("{failures} of {} file(s) could not be read", files.len());
            }
        }

        Command::Clean {
            paths,
            profile,
            out,
            in_place,
            recursive,
            dry_run,
            allow_content_loss,
            overwrite,
            json,
        } => {
            let profile = load_profile(&profile)?;
            profile.validate()?;
            if profile.inert() {
                bail!("the `{}` profile changes nothing; use `fm inspect`", profile.name);
            }
            let files = collect(&paths, recursive)?;

            // Visible content loss is confirmed before anything is written, not
            // reported afterwards.
            if !dry_run && !allow_content_loss {
                let mut risky: Vec<(PathBuf, String)> = Vec::new();
                for path in &files {
                    let Ok(ins) = pipeline::inspect_file(path) else { continue };
                    for f in report::destructive(&ins, &profile) {
                        risky.push((path.clone(), f.label.clone()));
                    }
                }
                if !risky.is_empty() {
                    eprintln!(
                        "This would remove content you can see, not only metadata:\n"
                    );
                    for (path, what) in &risky {
                        eprintln!("  {}  {what}", path.display());
                    }
                    bail!(
                        "\n{} change(s) would remove visible content. Re-run with \
                         --allow-content-loss once you have read the list, or with --dry-run to \
                         see the full diff first.",
                        risky.len()
                    );
                }
            }

            let disposition = match (in_place, overwrite) {
                (true, _) => Disposition::InPlace,
                (false, true) => Disposition::Overwrite,
                (false, false) => Disposition::Copy,
            };

            // Two sources with the same name clean to one destination, and in a
            // parallel run whichever finished last would silently be the one
            // kept. Caught here, where the whole list is known, rather than
            // raced for.
            if !dry_run {
                let mut seen: std::collections::HashMap<PathBuf, &PathBuf> = Default::default();
                let mut clashes: Vec<String> = Vec::new();
                for path in &files {
                    let dest = destination(path, &out, in_place)?;
                    if let Some(first) = seen.insert(dest.clone(), path) {
                        clashes.push(format!(
                            "  {} and {} would both be written to {}",
                            first.display(),
                            path.display(),
                            dest.display()
                        ));
                    }
                }
                if !clashes.is_empty() {
                    bail!(
                        "{} name collision(s) at the destination, so nothing was written:\n{}",
                        clashes.len(),
                        clashes.join("\n")
                    );
                }
            }
            let results: Vec<(PathBuf, Result<String>)> = files
                .par_iter()
                .map(|path| {
                    let rendered = (|| -> Result<String> {
                        let mut text = String::new();
                        if dry_run {
                            let data = std::fs::read(path)?;
                            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
                            let cleaned = fm_core::clean_bytes(&data, ext.as_deref(), &profile)?;
                            text.push_str(&format!(
                                "{}  {}\n{}",
                                path.display(),
                                cleaned.counts.sentence(),
                                report::text_diff(&cleaned.changes, &profile)
                            ));
                            text.push_str(&format!(
                                "  would go from {} to {} bytes\n",
                                cleaned.before.bytes, cleaned.after.bytes
                            ));
                        } else {
                            let dest = destination(path, &out, in_place)?;
                            let cleaned =
                                pipeline::clean_file(path, &dest, &profile, disposition)?;
                            text.push_str(&format!(
                                "{} -> {}\n  {}\n{}",
                                path.display(),
                                dest.display(),
                                cleaned.counts.sentence(),
                                report::text_diff(&cleaned.changes, &profile)
                            ));
                            text.push_str(&format!(
                                "  {}\n",
                                cleaned.verification.anonymity_sentence()
                            ));
                        }
                        Ok(text)
                    })();
                    (path.clone(), rendered)
                })
                .collect();

            let mut failures = 0;
            for (path, result) in results {
                match result {
                    Ok(text) => {
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({ "file": path.display().to_string(), "report": text })
                            );
                        } else {
                            print!("{text}");
                            println!();
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", path.display());
                        failures += 1;
                    }
                }
            }
            if failures > 0 {
                bail!("{failures} file(s) were not cleaned");
            }
        }

        Command::Verify { paths, profile, recursive } => {
            let profile = load_profile(&profile)?;
            let files = collect(&paths, recursive)?;
            let mut dirty = 0;
            for path in &files {
                match pipeline::verify_file(path, &profile) {
                    Ok(v) => {
                        println!("{}", path.display());
                        print!("{}", report::text_verify(&v));
                        if !v.clean() {
                            dirty += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", path.display());
                        dirty += 1;
                    }
                }
            }
            if dirty > 0 {
                bail!("{dirty} of {} file(s) are not clean", files.len());
            }
        }

        Command::Profile { name, save } => match name {
            None => {
                for p in Profile::builtins() {
                    println!("{:<10} {}", p.name, p.description);
                }
                println!(
                    "\nShow one with `fm profile <name>`, or write it out with \
                     `fm profile <name> --save my-profile.toml` and edit any line."
                );
            }
            Some(name) => {
                let p = load_profile(&name)?;
                match save {
                    Some(path) => {
                        std::fs::write(&path, p.to_toml())?;
                        println!("wrote {}", path.display());
                    }
                    None => print!("{}", report::text_profile(&p)),
                }
            }
        },
    }
    Ok(())
}

/// Verification of a file that arrived from elsewhere cannot know what was done
/// to it, which the text output says in so many words.
#[allow(dead_code)]
fn _doc(v: &Verification, r: &Report) {
    let _ = (v.clean(), r.headline());
}
