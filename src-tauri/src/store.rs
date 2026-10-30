//! Paths and on-disk persistence: the board, user settings, and the inbox folder.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::Board;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub discord_token: String,
    #[serde(default)]
    pub discord_channel_id: String,
    /// Snowflake of the newest Discord message we've processed.
    #[serde(default)]
    pub discord_last_message_id: String,
}

pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("could not resolve app data dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

pub fn board_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("board.json"))
}

pub fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

pub fn inbox_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("inbox");
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Write `text` to `path` via a temp file and rename, so a crash never leaves a half-written file.
pub fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

pub fn load_board(app: &AppHandle) -> Result<Board, String> {
    let path = board_path(app)?;
    if !path.exists() {
        return Ok(Board::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not a valid board: {e}", path.display()))
}

pub fn save_board(app: &AppHandle, board: &Board) -> Result<(), String> {
    let text = serde_json::to_string_pretty(board).map_err(|e| e.to_string())?;
    atomic_write(&board_path(app)?, &text)
}

pub fn load_settings(app: &AppHandle) -> Result<Settings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid settings: {e}", path.display()))
}

pub fn save_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    atomic_write(&path, &text)?;
    // The file holds a bot token; keep it readable by this user only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
