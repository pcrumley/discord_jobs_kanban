//! Files attached to a card (resumes, cover letters, …).
//!
//! Each attachment is *copied* into `<data dir>/attachments/<card id>/` at attach time, so
//! the card always holds the exact version that was sent, regardless of what later happens
//! to the original file. The card itself only stores metadata; the files are the truth.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::store;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub added_at: String,
}

fn root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(store::data_dir(app)?.join("attachments"))
}

fn card_dir(app: &AppHandle, card_id: &str) -> Result<PathBuf, String> {
    let id = safe_component(card_id)?;
    let dir = root(app)?.join(id);
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Reject anything that could escape its directory when used as a single path component.
fn safe_component(s: &str) -> Result<&str, String> {
    let s = s.trim();
    if s.is_empty() || s == "." || s == ".." || s.contains(['/', '\\', '\0']) {
        return Err(format!("invalid name: {s:?}"));
    }
    Ok(s)
}

/// "resume.pdf" -> "resume (2).pdf" if needed, so two different files never overwrite each other.
fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

fn describe(path: &Path) -> Result<Attachment, String> {
    let meta = fs::metadata(path).map_err(|e| format!("could not stat {}: {e}", path.display()))?;
    Ok(Attachment {
        name: path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string(),
        size: meta.len(),
        added_at: jiff::Timestamp::now().to_string(),
    })
}

/// Copy each source file into the card's folder. Returns the metadata of the copies.
pub fn attach(app: &AppHandle, card_id: &str, sources: &[String]) -> Result<Vec<Attachment>, String> {
    let dir = card_dir(app, card_id)?;
    let mut out = Vec::new();
    for src in sources {
        let src = Path::new(src);
        if !src.is_file() {
            return Err(format!("{} is not a file", src.display()));
        }
        let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("bad file name: {}", src.display()))?;
        let target = unique_target(&dir, name);
        fs::copy(src, &target).map_err(|e| format!("could not copy {}: {e}", src.display()))?;
        out.push(describe(&target)?);
    }
    Ok(out)
}

/// Copy an attachment that already belongs to one card onto another (the "recent files" quick-pick).
pub fn attach_from_card(app: &AppHandle, card_id: &str, from_card_id: &str, name: &str) -> Result<Attachment, String> {
    let src = card_dir(app, from_card_id)?.join(safe_component(name)?);
    if !src.is_file() {
        return Err(format!("{name} is no longer available on the other card"));
    }
    let dir = card_dir(app, card_id)?;
    let target = unique_target(&dir, name);
    fs::copy(&src, &target).map_err(|e| format!("could not copy {name}: {e}"))?;
    describe(&target)
}

pub fn path_of(app: &AppHandle, card_id: &str, name: &str) -> Result<PathBuf, String> {
    let path = card_dir(app, card_id)?.join(safe_component(name)?);
    if !path.is_file() {
        return Err(format!("{name} is missing from disk"));
    }
    Ok(path)
}

pub fn remove(app: &AppHandle, card_id: &str, name: &str) -> Result<(), String> {
    let path = card_dir(app, card_id)?.join(safe_component(name)?);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {}: {e}", path.display())),
    }
}

/// Delete everything attached to a card (used when the card itself is deleted or a draft is abandoned).
pub fn remove_all(app: &AppHandle, card_id: &str) -> Result<(), String> {
    let dir = root(app)?.join(safe_component(card_id)?);
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_do_not_collide() {
        let dir = std::env::temp_dir().join(format!("kanban-att-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("resume.pdf"), b"x").unwrap();
        assert_eq!(unique_target(&dir, "resume.pdf").file_name().unwrap(), "resume (2).pdf");
        fs::write(dir.join("resume (2).pdf"), b"x").unwrap();
        assert_eq!(unique_target(&dir, "resume.pdf").file_name().unwrap(), "resume (3).pdf");
        assert_eq!(unique_target(&dir, "notes").file_name().unwrap(), "notes");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_path_escapes() {
        assert!(safe_component("../x").is_err());
        assert!(safe_component("a/b").is_err());
        assert!(safe_component("").is_err());
        assert!(safe_component("resume.pdf").is_ok());
    }
}
