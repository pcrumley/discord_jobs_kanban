# Job Kanban

A small desktop kanban board for running a job search, built with [Tauri](https://tauri.app) (Rust backend, plain HTML/JS frontend).

## Features

- Columns for Inbox, Wishlist, Applied, Interviewing, Offer, Rejected. Drag cards between them.
- Cards track company, role, location, salary, posting URL, contact, notes, and how long they've sat in the current stage.
- **Discord capture**: share a job link into a private Discord channel (from your phone, from WhatsApp, anywhere) and it appears in Inbox with company, role, location and salary filled in from the posting page. Setup instructions are in the app under ⚙ Discord.
- **Triage mode**: step through Inbox cards one at a time with keyboard shortcuts (keep, applied, pass, later).
- Tracking and referral parameters are stripped from links; short links are resolved.
- Auto-saves to a JSON file in the per-user app data directory (on Linux, `~/.local/share/dev.patrick.jobkanban/board.json`).

## Running

Prerequisites: a Rust toolchain, `cargo install tauri-cli`, and on Linux the Tauri system packages:

```
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
```

Then:

```
cargo tauri dev      # run with live reload
cargo tauri build    # produce .deb / AppImage under src-tauri/target/release/bundle/
```

Run the backend tests with `cargo test` inside `src-tauri/`.

## Layout

- `src-tauri/` — Rust backend: persistence, inbox folder, Discord poller, job-page parser.
- `ui/` — frontend: `index.html`, `style.css`, `app.js`. No build step.

## License

GNU AFFERO GENERAL PUBLIC LICENSE
