//! The second surface: a directory on a machine.
//!
//! Nothing here knows what a browser is. The same three things are learned — recognise the target by
//! its shape, act on it, check the result it was meant to produce — and everything else is an
//! adapter: what counts as state (the entries of a directory) and what counts as an action (a shell
//! command). The rules, the gate and the receipts are the same objects the browser surface uses,
//! which is the whole point: a document written for a page governs a directory too, because it never
//! knew what a page was.

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command as Proc;

use crate::cdp::Element;

pub struct Shell {
    pub dir: PathBuf,
}

impl Shell {
    pub fn new(dir: &str) -> Result<Self> {
        let dir = PathBuf::from(dir);
        anyhow::ensure!(dir.is_dir(), "not a directory: {}", dir.display());
        Ok(Self { dir })
    }

    /// What is in the directory, in the shape the browser offers: a list of things, each with a role
    /// and a name, so the same decision code and the same rules can read it without being taught a
    /// second vocabulary.
    pub fn entries(&self) -> Result<Vec<Element>> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                !p.file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(true)
            })
            .collect();
        paths.sort();
        let mut out = Vec::new();
        for p in paths {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let md = std::fs::metadata(&p)?;
            let is_dir = md.is_dir();
            out.push(Element {
                handle: format!("file:{name}"),
                // The tag says which surface's reading applies to an action on this thing. An entry
                // in a directory is acted on by running a command, and what a command commits is not
                // readable from a browser's reading of a page — the gate asks the adapter instead.
                tag: "command".into(),
                role: if is_dir { "directory".into() } else { "file".into() },
                name,
                href: String::new(),
                on: true,
                checked: None,
                // Where a page shows a field's value, a directory shows an entry's size: the thing
                // about it a person would notice, and enough for the model to tell two apart.
                value: if is_dir { None } else { Some(human_size(md.len())) },
            });
        }
        Ok(out)
    }

    /// The directory as text, which is what a person would see after running `ls` here. This is what
    /// the final check reads.
    pub fn listing(&self) -> Result<String> {
        let mut s = String::new();
        for e in self.entries()? {
            // A folder is described by what it holds. Without this the finished state of a sorting
            // task reads as "twelve empty folders" — the invoices are all inside them, out of sight,
            // and a check that cannot see them cannot tell finished work from untouched work.
            let described = if e.role == "directory" {
                // What is inside, by name. A count alone is not something a check can be held to:
                // asked whether invoices ended up in the right folders, a check that can only see
                // "five files inside" has to guess, and guessing is how correct work gets failed.
                let mut inside: Vec<String> = std::fs::read_dir(self.dir.join(&e.name))
                    .map(|d| {
                        d.filter_map(|x| x.ok())
                            .map(|x| x.file_name().to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                inside.sort();
                let shown: Vec<String> = inside.iter().take(8).cloned().collect();
                let more = if inside.len() > shown.len() {
                    format!(", and {} more", inside.len() - shown.len())
                } else {
                    String::new()
                };
                format!("{} files inside: {}{more}", inside.len(), shown.join(", "))
            } else {
                e.value.clone().unwrap_or_default()
            };
            s.push_str(&format!("{} ({described})\n", e.name));
        }
        if s.is_empty() {
            s.push_str("(the directory is empty)\n");
        }
        Ok(s)
    }

    /// A fingerprint of the directory. A step counts when this changes — the same reading as "the
    /// page changed", and it is what makes a move that worked visible to the runtime that made it.
    pub fn kind(&self) -> Result<String> {
        let names: Vec<String> = self.entries()?.into_iter().map(|e| e.name).collect();
        Ok(format!("dir:{}", names.join(",")))
    }

    /// Run one command, from this directory. The command is whatever the decision named; nothing here
    /// adds to it, and nothing here retries it.
    pub fn run(&self, cmd: &str) -> Result<(bool, String)> {
        let out = Proc::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.dir)
            .output()?;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let text: String = text.chars().take(500).collect();
        Ok((out.status.success(), text.trim().to_string()))
    }
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} b")
    } else {
        format!("{:.1} kb", n as f64 / 1024.0)
    }
}

/// Does this command change the directory?
///
/// Read rather than guessed from the model's intent: the commands that write are named, and a
/// redirection anywhere in the line counts as writing. Anything not recognised as writing is treated
/// as reading — so an unfamiliar command is gated, which is the direction to fail in.
pub fn writes(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    if matches!(
        first,
        "mv" | "rm"
            | "cp"
            | "mkdir"
            | "rmdir"
            | "touch"
            | "chmod"
            | "chown"
            | "ln"
            | "truncate"
            | "sed"
            | "tee"
            | "dd"
            | "patch"
            | "tar"
            | "unzip"
            | "gzip"
            | "install"
    ) {
        return true;
    }
    cmd.contains('>') || cmd.contains(" -i ") || cmd.contains(" -delete")
}

/// How the gate and the rules see a command.
///
/// A command that changes the directory is where a commit happens — the same reading as a button on a
/// page — and one that only reads is a navigation: it changes nothing where it acts. The rules do not
/// know what a browser is, so it is one document that governs both surfaces.
pub fn observed(cmd: &str, approval: bool) -> crate::policy::Observed {
    let commits = writes(cmd);
    crate::policy::Observed {
        action: "click".into(),
        tag: "command".into(),
        role: if commits { "button".into() } else { "link".into() },
        has_href: !commits,
        approval,
    }
}
