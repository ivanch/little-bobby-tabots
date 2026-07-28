mod commands;
mod playlists;
mod voice_idle;
mod web;

use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc},
};

use serenity::{
    all::{Command, CreateCommand, GatewayIntents, Interaction, Ready, VoiceState},
    async_trait,
    client::{Client, Context, EventHandler},
};
use songbird::SerenityInit;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

use commands::guild_state::GuildMusicState;
use commands::preplay::PrePlayConfig;
use playlists::PlaylistLibrary;

/// Shared application data available to all command handlers.
#[derive(Default)]
pub struct Data {
    /// Per-guild music state (queue, current track, etc.)
    pub music_states: RwLock<HashMap<u64, Arc<RwLock<GuildMusicState>>>>,
    /// Serializes playback mutations so Songbird and displayed queue state stay ordered.
    pub music_operation_locks: Mutex<HashMap<u64, Arc<Mutex<()>>>>,
    /// One cancellable empty-channel timer per guild.
    pub(crate) empty_channel_timers: Mutex<HashMap<u64, voice_idle::EmptyChannelTimer>>,
    /// Monotonic identity used to prevent stale timers from disconnecting newer calls.
    pub(crate) next_empty_channel_timer_generation: AtomicU64,
    /// Process-wide pre-play defaults loaded from the environment.
    pub preplay_config: PrePlayConfig,
    /// Read-only predefined playlists shared by Discord and the dashboard.
    pub playlist_library: PlaylistLibrary,
}

impl Data {
    pub fn new(preplay_config: PrePlayConfig, playlist_library: PlaylistLibrary) -> Self {
        Self {
            preplay_config,
            playlist_library,
            ..Self::default()
        }
    }

    pub async fn music_state(&self, guild_id: u64) -> Arc<RwLock<GuildMusicState>> {
        let mut states = self.music_states.write().await;
        states
            .entry(guild_id)
            .or_insert_with(|| Arc::new(RwLock::new(GuildMusicState::new())))
            .clone()
    }

    pub async fn music_operation_lock(&self, guild_id: u64) -> Arc<Mutex<()>> {
        let mut locks = self.music_operation_locks.lock().await;
        locks
            .entry(guild_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

/// The main event handler for the bot.
struct Handler;

#[async_trait]
impl EventHandler for Handler {
    /// Called once the bot has connected and is ready.
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("Connected as {}", ready.user.name);

        // Register all slash commands globally.
        // Global commands take up to 1 hour to propagate on Discord.
        // For faster testing, use guild-specific commands instead.
        let commands = vec![
            CreateCommand::new("ping").description("Check if the bot is alive"),
            CreateCommand::new("play")
                .description("Play a song or queue videos from YouTube playlist")
                .add_option(
                    serenity::all::CreateCommandOption::new(
                        serenity::all::CommandOptionType::String,
                        "query",
                        "Song name, direct URL, or explicit YouTube playlist URL",
                    )
                    .required(true),
                ),
            CreateCommand::new("playlist")
                .description("Queue a predefined text playlist")
                .add_option(
                    serenity::all::CreateCommandOption::new(
                        serenity::all::CommandOptionType::String,
                        "name",
                        "Playlist filename without the .txt extension",
                    )
                    .required(true),
                ),
            CreateCommand::new("skip").description("Skip the current song"),
            CreateCommand::new("clear")
                .description("Clear queued tracks without stopping the current song"),
            CreateCommand::new("leave").description("Stop playback and leave the voice channel"),
            CreateCommand::new("pause").description("Pause the currently playing song"),
            CreateCommand::new("resume").description("Resume paused playback"),
            CreateCommand::new("queue").description("Show the current queue"),
            CreateCommand::new("preplay")
                .description("Enable or update probabilistic between-track audio")
                .add_option(
                    serenity::all::CreateCommandOption::new(
                        serenity::all::CommandOptionType::String,
                        "url",
                        "YouTube video URL (defaults to PREPLAY_URL)",
                    )
                    .required(false),
                ),
            CreateCommand::new("stop-preplay")
                .description("Disable probabilistic between-track audio"),
        ];

        if let Ok(guild_id_str) = std::env::var("GUILD_ID") {
            if let Ok(guild_id_val) = guild_id_str.trim().parse::<u64>() {
                let guild_id = serenity::all::GuildId::new(guild_id_val);
                match guild_id.set_commands(&ctx.http, commands).await {
                    Ok(cmds) => info!(
                        "Registered {} guild-specific slash commands for guild {}",
                        cmds.len(),
                        guild_id
                    ),
                    Err(e) => error!("Failed to register guild slash commands: {e}"),
                }

                // Discord displays global and guild-specific commands together. Remove
                // any global commands left from an earlier global registration so the
                // configured guild does not show every command twice.
                match Command::set_global_commands(&ctx.http, Vec::<CreateCommand>::new()).await {
                    Ok(_) => {
                        info!("Cleared global slash commands while using guild-specific commands")
                    }
                    Err(e) => warn!("Failed to clear global slash commands: {e}"),
                }
                return;
            }
        }

        match Command::set_global_commands(&ctx.http, commands).await {
            Ok(cmds) => info!("Registered {} global slash commands", cmds.len()),
            Err(e) => error!("Failed to register slash commands: {e}"),
        }
    }

    /// Called on every incoming interaction (slash commands, buttons, etc.)
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else {
            return;
        };

        let data = ctx
            .data
            .read()
            .await
            .get::<commands::DataKey>()
            .cloned()
            .expect("Data should always be present");

        let result = match command.data.name.as_str() {
            "ping" => commands::ping::run(&ctx, &command).await,
            "play" => commands::play::run(&ctx, &command, &data).await,
            "playlist" => commands::playlist::run(&ctx, &command, &data).await,
            "skip" => commands::skip::run(&ctx, &command, &data).await,
            "clear" => commands::clear::run(&ctx, &command, &data).await,
            "leave" => commands::leave::run(&ctx, &command, &data).await,
            "pause" => commands::pause::run(&ctx, &command, &data).await,
            "resume" => commands::resume::run(&ctx, &command, &data).await,
            "queue" => commands::queue::run(&ctx, &command, &data).await,
            "preplay" => commands::preplay::run_enable(&ctx, &command, &data).await,
            "stop-preplay" => commands::preplay::run_stop(&ctx, &command, &data).await,
            other => {
                error!("Unknown command: {other}");
                Ok(())
            }
        };

        if let Err(e) = result {
            error!("Error handling command '{}': {e}", command.data.name);
        }
    }

    /// Start or cancel empty-channel timers whenever voice membership changes.
    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        let Some(guild_id) = new.guild_id else {
            return;
        };

        let data = ctx
            .data
            .read()
            .await
            .get::<commands::DataKey>()
            .cloned()
            .expect("Data should always be present");

        voice_idle::refresh(&ctx, &data, guild_id).await;
    }
}

#[tokio::main]
async fn main() {
    // Load a .env file if present (convenient for local dev).
    // Does nothing if the file doesn't exist.
    let _ = dotenvy::dotenv();

    // Initialise structured logging. Set RUST_LOG env var to control level.
    // e.g. RUST_LOG=little_bobby_tabots=debug,songbird=info
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let token = std::env::var("DISCORD_TOKEN")
        .expect("DISCORD_TOKEN must be set (in environment or .env file)");

    // Minimal intents: guild messages for voice state lookups, guild voice states
    // for knowing which channel the user is in.
    let intents = GatewayIntents::non_privileged() | GatewayIntents::GUILD_VOICE_STATES;

    let preplay_config = PrePlayConfig::from_env()
        .unwrap_or_else(|error| panic!("Invalid pre-play configuration: {error}"));
    let shared_data = Arc::new(Data::new(preplay_config, PlaylistLibrary::from_env()));

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler)
        .register_songbird()
        .type_map_insert::<commands::DataKey>(Arc::clone(&shared_data))
        .await
        .expect("Failed to create serenity client");

    info!("Starting Bobby TaBot…");

    let songbird = {
        let client_data = client.data.read().await;
        client_data
            .get::<songbird::serenity::SongbirdKey>()
            .cloned()
            .expect("Songbird must be registered")
    };
    let web_data = Arc::clone(&shared_data);
    tokio::spawn(async move {
        if let Err(error) = web::serve(web_data, songbird).await {
            error!("Dashboard server stopped: {error}");
        }
    });

    if let Err(e) = client.start().await {
        error!("Client error: {e}");
    }
}
