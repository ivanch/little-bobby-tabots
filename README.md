# 🤖 Little Bobby TaBots

[![Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/Container-Docker%20Compose-blue.svg)](https://www.docker.com/)
[![Security Audit](https://img.shields.io/badge/Security-100%25%20Vulnerability%20Free-success.svg)](https://rustsec.org/)

**Little Bobby TaBots** is a highly optimized, lightweight, self-hosted Discord music bot written in Rust. It is designed to run locally or inside a Docker container with minimal memory and CPU footprints.

*Name origin: A nod to the classic XKCD "Robert'); DROP TABLE Students;--" comic.*

---

## ✨ Features

*   **Native Audio Decoding**: Uses `symphonia` natively inside the Rust binary to decode Ogg, MP3, FLAC, and AAC, avoiding heavy external background decoding loops.
*   **Discord Voice E2EE**: Built on `songbird 0.6.0` to support Discord's mandatory end-to-end voice encryption (DAVE protocol), eliminating websocket timeouts and disconnects.
*   **Vulnerability-Free**: Uses `native-tls` (OpenSSL) backend to keep compile-time dependencies 100% free of security advisories. Fully audited via `cargo audit`.
*   **Streamed Playback**: Downloads nothing to disk. Audio is piped directly from `yt-dlp` via `ffmpeg` to memory buffers, leaving zero temp file waste.
*   **Automatic Voice Cleanup**: Stops playback, clears the queue, and disconnects after the voice channel has no human listeners for 10 minutes.
*   **Probabilistic Pre-Play Audio**: Optionally inserts a configured YouTube clip between queued music tracks using a configurable percentage chance.
*   **Predefined Playlists**: Read text playlists from a local folder and queue them from Discord or the dashboard.
*   **Slash Commands**: Supports full modern slash interaction registry with guild-level instant registration.
*   **Web Dashboard**: A responsive Preact interface for adding tracks, viewing and reordering the queue, clearing upcoming songs, and controlling playback in light or dark mode.

---

## 📋 Commands Reference

| Command | Description |
| :--- | :--- |
| `/play <query>` | Connects to your voice channel and plays/queues a song (searches YouTube/SoundCloud or accepts direct URLs). An explicit YouTube `/playlist?list=...` URL queues resolvable videos in playlist order. |
| `/playlist <name>` | Queues the songs from `<name>.txt` in the configured predefined-playlists folder. |
| `/pause` | Pauses playback of the current track. |
| `/resume` | Resumes playing the paused track. |
| `/skip` | Skips the current track and starts the next one in the queue. |
| `/clear` | Clears all upcoming tracks without stopping the current track. |
| `/queue` | Shows an embed list of the currently playing track and the upcoming playlist. |
| `/preplay [url]` | Enables or updates between-track audio. Uses `PREPLAY_URL` when no URL is supplied. |
| `/stop-preplay` | Disables future between-track audio for the server. |
| `/leave` | Stops playback, clears the queue, and disconnects the bot from the voice channel. |
| `/ping` | A diagnostics command to confirm the bot is active and responsive. |

---

## 🚀 Getting Started

### 1. Discord Bot Setup
1. Go to the [Discord Developer Portal](https://discord.com/developers/applications).
2. Create a new Application called **Little Bobby TaBots** (or your preferred name).
3. Under the **Bot** tab, create a bot user and copy the **Token**.
4. Scroll down under the **Bot** tab and ensure **Guild Voice States** intent is enabled under **Privileged Gateway Intents**.
5. Under the **Installation** tab, set scopes to `bot` and `applications.commands`. Under permissions, grant `Connect`, `Speak`, and `Send Messages`.
6. Use the generated link to invite the bot to your Discord server.

### 2. Configuration
Create a `.env` file in the project root:
```env
DISCORD_TOKEN=your_copied_discord_bot_token_here
GUILD_ID=your_test_server_id_here
VOICE_CHANNEL_ID=the_voice_channel_for_dashboard_playback
PREPLAY_URL=https://www.youtube.com/watch?v=your_video_id
PREPLAY_CHANCE_PERCENT=75
PLAYLISTS_HOST_DIR=./playlists
```
> [!NOTE]
> Setting `GUILD_ID` registers slash commands instantly in your test server on bot startup and removes this bot's old global commands, preventing duplicate entries. Without it, commands are registered globally and can take up to an hour to populate.
> `VOICE_CHANNEL_ID` lets the dashboard connect the bot when it is not already in voice. If the bot is connected, the dashboard uses its current channel.
> `PREPLAY_URL` is optional when a URL is supplied directly to `/preplay`. `PREPLAY_CHANCE_PERCENT` is optional and defaults to `75`; valid values are `0` through `100`.
> `PLAYLISTS_HOST_DIR` is the host folder mounted read-only at `/playlists` by Docker Compose. It defaults to `./playlists`.
> If an existing `.env` already sets `PLAYLISTS_DIR` to the host playlist folder, Compose uses it as a fallback bind path; `PLAYLISTS_HOST_DIR` takes precedence.

### Predefined playlist files

Create a `.txt` file in the configured folder, such as `playlists/road-trip.txt`. Its filename without `.txt` is the playlist name used by `/playlist` and shown in the dashboard.

Each non-empty line is a song search or direct media URL. Blank lines are ignored; order is preserved. A playlist can contain up to 100 entries, with a maximum of 500 characters per line. Every entry must resolve successfully before anything is queued.

```text
Daft Punk Get Lucky
https://www.youtube.com/watch?v=dQw4w9WgXcQ
Massive Attack Teardrop
```

The dashboard refreshes its playlist selector automatically and queues the selected file using the same rules as `/playlist`.

---

## 🐳 Running with Docker Compose (Recommended)

The easiest way to run the bot is containerized via Docker Compose. The multi-stage build compiles a statically linked `musl` release binary and bundles it inside a lightweight Alpine container with `ffmpeg` and `yt-dlp` pre-configured.

1.  Start the bot container in the background:
    ```bash
    docker compose up -d
    ```
2.  Follow the live logs to confirm it successfully connects:
    ```bash
    docker compose logs -f
    ```
3.  Open the dashboard at [http://localhost:3000](http://localhost:3000).
4.  Stop the bot container:
    ```bash
    docker compose down
    ```

The dashboard controls the single server configured by `GUILD_ID`. Keep port
`3000` on a trusted network unless you place an authenticated reverse proxy in
front of it.

---

## 🛠️ Running Locally (Without Docker)

To run the project directly from your shell, you will need the following installed:
*   [Rust toolchain (stable)](https://rustup.rs/)
*   [ffmpeg](https://ffmpeg.org/) (must be in your system `PATH`)
*   [yt-dlp](https://github.com/yt-dlp/yt-dlp) (must be in your system `PATH`)
*   [Node.js](https://nodejs.org/) 22 or newer (for building the dashboard)

1.  Build the dashboard:
    ```bash
    cd web
    npm install
    npm run build
    cd ..
    ```
2.  Compile and run the release binary:
    ```bash
    cargo run --release
    ```

For local runs, set `PLAYLISTS_DIR=./playlists` (or another readable folder) in `.env` before starting the bot.

---

## 🔒 Security Auditing

To run a security check on dependencies:
```bash
cargo install cargo-audit
cargo audit
```
*Current status:* **`error: 0 vulnerabilities found!`**
