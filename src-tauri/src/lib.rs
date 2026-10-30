//! Backend for the job-search kanban board.
//!
//! The frontend owns the board state and this side persists it as JSON. Cards can
//! also arrive from outside the UI through an inbox folder, fed by the Discord poller.

mod attachments;
mod discord;
mod jobinfo;
mod store;

use std::fs;
use std::sync::Mutex;

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use attachments::Attachment;
use discord::DiscordState;
use store::Settings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub salary: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub stage_changed_at: String,
    /// Where the card came from: "" for the UI, "discord" for the poller.
    #[serde(default)]
    pub source: String,
    /// True until the user opens the card for the first time.
    #[serde(default)]
    pub unreviewed: bool,
    /// Files copied into the attachments folder for this card.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Any fields the frontend adds later survive a round-trip through an older backend.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Card {
    pub fn new() -> Self {
        let now = jiff::Timestamp::now().to_string();
        Card {
            id: format!("{:x}{:04x}", jiff::Timestamp::now().as_millisecond(), rand_suffix()),
            company: String::new(),
            role: String::new(),
            url: String::new(),
            location: String::new(),
            salary: String::new(),
            contact: String::new(),
            notes: String::new(),
            created_at: now.clone(),
            stage_changed_at: now,
            source: String::new(),
            unreviewed: false,
            attachments: Vec::new(),
            extra: Default::default(),
        }
    }
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}

/// A few bits of entropy so two cards created in the same millisecond don't collide.
fn rand_suffix() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    (std::collections::hash_map::RandomState::new().build_hasher().finish() & 0xffff) as u16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Board {
    #[serde(default)]
    pub columns: Vec<Column>,
}

impl Default for Board {
    fn default() -> Self {
        let col = |id: &str, title: &str| Column { id: id.into(), title: title.into(), cards: Vec::new() };
        Board {
            columns: vec![
                col("inbox", "Inbox"),
                col("wishlist", "Wishlist"),
                col("applied", "Applied"),
                col("interviewing", "Interviewing"),
                col("offer", "Offer"),
                col("rejected", "Rejected"),
            ],
        }
    }
}

// ---------- board ----------

#[tauri::command]
fn load_board(app: AppHandle) -> Result<Board, String> {
    store::load_board(&app)
}

#[tauri::command]
fn save_board(app: AppHandle, board: Board) -> Result<(), String> {
    store::save_board(&app, &board)
}

#[tauri::command]
fn board_path(app: AppHandle) -> Result<String, String> {
    store::board_path(&app).map(|p| p.display().to_string())
}

// ---------- inbox ----------

/// Move every card waiting in the inbox folder out to the caller, deleting the files.
#[tauri::command]
fn take_inbox(app: AppHandle) -> Result<Vec<Card>, String> {
    let dir = store::inbox_dir(&app)?;
    let mut cards = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("could not read inbox: {e}"))?;
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        match serde_json::from_str::<Card>(&text) {
            Ok(card) => {
                cards.push(card);
                let _ = fs::remove_file(&path);
            }
            // Leave unparseable files alone: another process may still be writing it.
            Err(e) => eprintln!("inbox: skipping {}: {e}", path.display()),
        }
    }
    Ok(cards)
}

#[tauri::command]
async fn fetch_job_details(url: String) -> Result<jobinfo::JobInfo, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("No URL to fetch".into());
    }
    jobinfo::fetch(&jobinfo::client(), &url).await
}

// ---------- attachments ----------

#[tauri::command]
fn attach_files(app: AppHandle, card_id: String, paths: Vec<String>) -> Result<Vec<Attachment>, String> {
    attachments::attach(&app, &card_id, &paths)
}

#[tauri::command]
fn attach_from_card(app: AppHandle, card_id: String, from_card_id: String, name: String) -> Result<Attachment, String> {
    attachments::attach_from_card(&app, &card_id, &from_card_id, &name)
}

#[tauri::command]
fn open_attachment(app: AppHandle, card_id: String, name: String) -> Result<(), String> {
    let path = attachments::path_of(&app, &card_id, &name)?;
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_attachment(app: AppHandle, card_id: String, name: String) -> Result<(), String> {
    let path = attachments::path_of(&app, &card_id, &name)?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_attachment(app: AppHandle, card_id: String, name: String) -> Result<(), String> {
    attachments::remove(&app, &card_id, &name)
}

#[tauri::command]
fn remove_card_attachments(app: AppHandle, card_id: String) -> Result<(), String> {
    attachments::remove_all(&app, &card_id)
}

// ---------- settings / discord ----------

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    store::load_settings(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let mut current = store::load_settings(&app)?;
    let channel_changed = current.discord_channel_id.trim() != settings.discord_channel_id.trim();
    current.discord_token = settings.discord_token.trim().to_string();
    current.discord_channel_id = settings.discord_channel_id.trim().to_string();
    if channel_changed {
        // A new channel means the stored "last seen" message id no longer applies.
        current.discord_last_message_id.clear();
    }
    store::save_settings(&app, &current)
}

#[tauri::command]
async fn test_discord(token: String, channel_id: String) -> Result<String, String> {
    discord::test_connection(&token, &channel_id).await
}

#[tauri::command]
fn discord_status(state: tauri::State<'_, DiscordState>) -> discord::Status {
    state.status.lock().unwrap().clone()
}

// ---------- app ----------

struct InboxWatcher(#[allow(dead_code)] Mutex<notify::RecommendedWatcher>);

fn start_inbox_watcher(app: &AppHandle) -> Result<(), String> {
    let dir = store::inbox_dir(app)?;
    let handle = app.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = handle.emit("inbox-changed", ());
        }
    })
    .map_err(|e| format!("could not create inbox watcher: {e}"))?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("could not watch {}: {e}", dir.display()))?;
    app.manage(InboxWatcher(Mutex::new(watcher)));
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // reqwest is built without a bundled crypto provider; make ring the process default.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Remembers window size and position between launches.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(DiscordState { status: Mutex::new(discord::Status::default()) })
        .setup(|app| {
            let handle = app.handle().clone();
            if let Err(e) = start_inbox_watcher(&handle) {
                eprintln!("{e}");
            }
            discord::start(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_board,
            save_board,
            board_path,
            take_inbox,
            fetch_job_details,
            attach_files,
            attach_from_card,
            open_attachment,
            reveal_attachment,
            remove_attachment,
            remove_card_attachments,
            get_settings,
            save_settings,
            test_discord,
            discord_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
