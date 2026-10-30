//! Polls a Discord channel over the REST API and turns every message that contains
//! a link into a card in the inbox folder.
//!
//! Polling (rather than the gateway websocket) keeps this small and means a launch
//! after days away simply picks up where the last processed message left off.

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::store;
use crate::{jobinfo, Card};

const API: &str = "https://discord.com/api/v10";
const POLL_EVERY: Duration = Duration::from_secs(15);
const CHECK: &str = "✅";

static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://[^\s<>()\[\]]+").unwrap());

#[derive(Debug, Default, Clone, Serialize)]
pub struct Status {
    pub configured: bool,
    pub last_poll: Option<String>,
    pub last_error: Option<String>,
    pub captured: u64,
}

pub struct DiscordState {
    pub status: Mutex<Status>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: String,
    #[serde(default)]
    content: String,
    author: Author,
    #[serde(default)]
    embeds: Vec<Embed>,
    #[serde(default)]
    reactions: Vec<Reaction>,
    #[serde(default)]
    timestamp: String,
}

#[derive(Debug, Deserialize)]
struct Author {
    #[serde(default)]
    bot: bool,
    #[serde(default)]
    username: String,
}

#[derive(Debug, Deserialize)]
struct Embed {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Reaction {
    emoji: Emoji,
    #[serde(default)]
    me: bool,
}

#[derive(Debug, Deserialize)]
struct Emoji {
    #[serde(default)]
    name: Option<String>,
}

fn api_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("DiscordBot (https://github.com/patrick/kanban, 0.1)")
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client")
}

async fn api_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or(body);
    match status.as_u16() {
        401 => "Discord rejected the bot token (401)".to_string(),
        403 => format!("Bot lacks access to that channel (403): {msg}"),
        404 => "Channel not found (404). Check the channel ID and that the bot was invited to the server.".to_string(),
        _ => format!("Discord API error {}: {msg}", status.as_u16()),
    }
}

/// Verify a token + channel pair. Returns a human-readable description on success.
pub async fn test_connection(token: &str, channel_id: &str) -> Result<String, String> {
    let client = api_client();
    let auth = format!("Bot {}", token.trim());

    let me = client.get(format!("{API}/users/@me")).header("Authorization", &auth).send().await.map_err(|e| e.to_string())?;
    if !me.status().is_success() {
        return Err(api_error(me).await);
    }
    let me: serde_json::Value = me.json().await.map_err(|e| e.to_string())?;
    let bot_name = me.get("username").and_then(|v| v.as_str()).unwrap_or("bot").to_string();

    let ch = client
        .get(format!("{API}/channels/{}", channel_id.trim()))
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !ch.status().is_success() {
        return Err(api_error(ch).await);
    }
    let ch: serde_json::Value = ch.json().await.map_err(|e| e.to_string())?;
    let ch_name = ch.get("name").and_then(|v| v.as_str()).map(|n| format!("#{n}")).unwrap_or_else(|| "the DM".into());

    // Reading history is the permission people most often forget; probe it now.
    let probe = client
        .get(format!("{API}/channels/{}/messages?limit=1", channel_id.trim()))
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !probe.status().is_success() {
        return Err(format!("Connected as {bot_name}, but cannot read messages in {ch_name}: {}", api_error(probe).await));
    }

    Ok(format!("Connected as {bot_name}, watching {ch_name}"))
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let client = api_client();
        let page_client = jobinfo::client();
        loop {
            let result = poll_once(&app, &client, &page_client).await;
            {
                let state = app.state::<DiscordState>();
                let mut st = state.status.lock().unwrap();
                st.last_poll = Some(jiff::Timestamp::now().to_string());
                match result {
                    Ok(Some(n)) => {
                        st.configured = true;
                        st.last_error = None;
                        st.captured += n;
                    }
                    Ok(None) => st.configured = false,
                    Err(e) => {
                        st.configured = true;
                        st.last_error = Some(e);
                    }
                }
            }
            tokio::time::sleep(POLL_EVERY).await;
        }
    });
}

/// Returns `Ok(None)` when Discord isn't configured, otherwise the number of cards captured.
async fn poll_once(app: &AppHandle, client: &reqwest::Client, page_client: &reqwest::Client) -> Result<Option<u64>, String> {
    let mut settings = store::load_settings(app)?;
    let token = settings.discord_token.trim().to_string();
    let channel = settings.discord_channel_id.trim().to_string();
    if token.is_empty() || channel.is_empty() {
        return Ok(None);
    }
    let auth = format!("Bot {token}");

    let mut url = format!("{API}/channels/{channel}/messages?limit=50");
    if !settings.discord_last_message_id.is_empty() {
        url = format!("{API}/channels/{channel}/messages?limit=100&after={}", settings.discord_last_message_id);
    }
    let resp = client.get(&url).header("Authorization", &auth).send().await.map_err(|e| format!("Discord unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let mut messages: Vec<Message> = resp.json().await.map_err(|e| format!("bad response from Discord: {e}"))?;
    messages.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));

    let mut captured = 0;
    for msg in messages {
        if !msg.author.bot && !already_handled(&msg) {
            if let Some(card) = card_from_message(&msg, page_client).await {
                write_inbox(app, &card)?;
                captured += 1;
                // Best effort: the check mark is feedback for the human, not our source of truth.
                let _ = client
                    .put(format!("{API}/channels/{channel}/messages/{}/reactions/{CHECK}/@me", msg.id))
                    .header("Authorization", &auth)
                    .header("Content-Length", "0")
                    .send()
                    .await;
            }
        }
        settings.discord_last_message_id = msg.id.clone();
        store::save_settings(app, &settings)?;
    }
    if captured > 0 {
        let _ = app.emit("inbox-changed", ());
    }
    Ok(Some(captured))
}

fn already_handled(msg: &Message) -> bool {
    msg.reactions.iter().any(|r| r.me && r.emoji.name.as_deref() == Some(CHECK))
}

async fn card_from_message(msg: &Message, page_client: &reqwest::Client) -> Option<Card> {
    let link = URL_RE
        .find(&msg.content)
        .map(|m| m.as_str().trim_end_matches(['.', ',', '!', '?']).to_string())
        .or_else(|| msg.embeds.iter().find_map(|e| e.url.clone()))?;

    let mut card = Card::new();
    card.url = jobinfo::clean_url(&link);
    card.source = "discord".into();
    card.unreviewed = true;
    if !msg.timestamp.is_empty() {
        card.created_at = msg.timestamp.clone();
    }

    // Whatever text accompanied the link becomes the notes.
    let mut notes: Vec<String> = Vec::new();
    let remainder = URL_RE.replace_all(&msg.content, "").trim().to_string();
    if !remainder.is_empty() {
        notes.push(remainder);
    }

    match jobinfo::fetch(page_client, &link).await {
        Ok(info) => {
            if !info.url.is_empty() {
                card.url = info.url;
            }
            card.company = info.company;
            card.role = info.role;
            card.location = info.location;
            card.salary = info.salary;
        }
        Err(e) => {
            // Discord usually unfurled the link already; use that as a second-best title.
            if let Some(embed) = msg.embeds.iter().find(|e| e.title.is_some()) {
                card.role = embed.title.clone().unwrap_or_default();
                if let Some(d) = &embed.description {
                    notes.push(d.clone());
                }
            }
            notes.push(format!("(Could not read the posting page: {e})"));
        }
    }
    if card.company.is_empty() && card.role.is_empty() {
        card.company = card.url.clone();
    }

    notes.push(format!("Shared via Discord by {}", msg.author.username));
    card.notes = notes.join("\n\n");
    Some(card)
}

fn write_inbox(app: &AppHandle, card: &Card) -> Result<(), String> {
    let dir = store::inbox_dir(app)?;
    let text = serde_json::to_string_pretty(card).map_err(|e| e.to_string())?;
    store::atomic_write(&dir.join(format!("{}.json", card.id)), &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs network; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn bad_token_gives_clear_error() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let err = test_connection("not-a-real-token", "1").await.unwrap_err();
        assert!(err.contains("401"), "got: {err}");
    }
}
