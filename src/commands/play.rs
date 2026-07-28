use std::{sync::Arc, time::Duration};

use rand::Rng;
use serenity::{
    all::{
        CommandInteraction, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
        CreateInteractionResponseMessage, ResolvedValue, UserId,
    },
    client::Context,
};
use songbird::{
    events::{Event, EventContext, EventData, EventHandler as SongbirdEventHandler, TrackEvent},
    input::{Compose, YoutubeDl},
    tracks::Track as SongbirdTrack,
};
use tracing::{error, info};

use crate::{
    commands::{guild_state::Track, preplay, youtube_playlist},
    playlists::Playlist,
    Data,
};

/// /play {query} — join the user's voice channel and play or queue tracks.
pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    data: &Arc<Data>,
) -> Result<(), serenity::Error> {
    let query = match command.data.options().first() {
        Some(opt) => match &opt.value {
            ResolvedValue::String(value) => value.to_string(),
            _ => return reply_ephemeral(ctx, command, "❌ Expected a text query.").await,
        },
        None => {
            return reply_ephemeral(ctx, command, "❌ Please provide a song name or URL.").await
        }
    };

    let guild_id = match command.guild_id {
        Some(id) => id,
        None => {
            return reply_ephemeral(
                ctx,
                command,
                "❌ This command can only be used in a server.",
            )
            .await
        }
    };

    // Extract the ChannelId and drop CacheRef before awaiting.
    let channel_id = {
        ctx.cache.guild(guild_id).and_then(|guild| {
            guild
                .voice_states
                .get(&command.user.id)
                .and_then(|state| state.channel_id)
        })
    };

    let channel_id = match channel_id {
        Some(id) => id,
        None => {
            return reply_ephemeral(ctx, command, "❌ You need to be in a voice channel first.")
                .await
        }
    };

    // Playlist expansion and normal metadata lookup can take a few seconds.
    command.defer(&ctx.http).await?;

    let request = match resolve_request(&query, Some(command.user.id)).await {
        Ok(request) => request,
        Err(message) => return edit_reply(ctx, command, &message).await,
    };

    // Playback-changing operations must remain ordered across /play, track-end
    // events, and /leave. Resolve external metadata before entering this section.
    let operation_lock = data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;

    let songbird = songbird::get(ctx)
        .await
        .expect("Songbird must be registered");
    let handler_lock = match songbird.join(guild_id, channel_id).await {
        Ok(handler) => handler,
        Err(error) => {
            error!("Failed to join voice channel: {error}");
            drop(_operation_guard);
            return edit_reply(
                ctx,
                command,
                "❌ Failed to join your voice channel. Do I have permission to connect?",
            )
            .await;
        }
    };

    let was_idle =
        enqueue_resolved_request(&handler_lock, guild_id.get(), data, &songbird, &request).await;
    drop(_operation_guard);

    reply_for_request(ctx, command, &request, was_idle).await
}

pub(crate) struct ResolvedPlayRequest {
    pub tracks: Vec<Track>,
    pub playlist_title: Option<String>,
}

pub(crate) async fn resolve_request(
    query: &str,
    requested_by: Option<UserId>,
) -> Result<ResolvedPlayRequest, String> {
    if youtube_playlist::is_youtube_playlist_url(query) {
        let playlist = youtube_playlist::resolve(query).await.map_err(|error| {
            error!("yt-dlp playlist lookup failed for '{query}': {error}");
            "❌ Could not load that YouTube playlist. It may be private, unavailable, or empty."
                .to_string()
        })?;

        let tracks = playlist
            .tracks
            .into_iter()
            .map(|track| Track {
                title: track.title,
                url: track.url,
                requested_by,
                playback_id: None,
            })
            .collect();

        return Ok(ResolvedPlayRequest {
            tracks,
            playlist_title: Some(playlist.title),
        });
    }

    Ok(ResolvedPlayRequest {
        tracks: vec![resolve_track(query, requested_by).await?],
        playlist_title: None,
    })
}

/// Resolve every entry in one predefined text playlist before it mutates playback.
pub(crate) async fn resolve_predefined_playlist(
    playlist: &Playlist,
    requested_by: Option<UserId>,
) -> Result<ResolvedPlayRequest, String> {
    let mut tracks = Vec::with_capacity(playlist.entries.len());

    for entry in &playlist.entries {
        if youtube_playlist::is_youtube_playlist_url(&entry.query) {
            return Err(format!(
                "❌ Playlist `{}` line {} must be a single song or URL, not a YouTube playlist. Nothing was queued.",
                playlist.name, entry.line_number
            ));
        }

        let track = resolve_track(&entry.query, requested_by)
            .await
            .map_err(|error| {
                error!(
                    playlist = %playlist.name,
                    line = entry.line_number,
                    query = %entry.query,
                    "Could not resolve predefined playlist entry: {error}"
                );
                format!(
                    "❌ Could not resolve playlist `{}` line {}. Nothing was queued.",
                    playlist.name, entry.line_number
                )
            })?;
        tracks.push(track);
    }

    Ok(ResolvedPlayRequest {
        tracks,
        playlist_title: Some(playlist.name.clone()),
    })
}

/// Resolve a single song query or direct media URL.
async fn resolve_track(query: &str, requested_by: Option<UserId>) -> Result<Track, String> {
    // URLs are sent to yt-dlp as-is; search text resolves to one YouTube result.
    let source_url = if is_url(query) {
        query.to_string()
    } else {
        format!("ytsearch1:{query}")
    };
    let mut source = YoutubeDl::new(reqwest::Client::new(), source_url.clone());
    let metadata = source.aux_metadata().await.map_err(|error| {
        error!("yt-dlp metadata fetch failed for '{query}': {error}");
        format!("❌ Could not find `{query}`. The link may be private, geo-blocked, or invalid.")
    })?;
    let title = metadata.title.unwrap_or_else(|| query.to_string());
    info!("Resolved track: {title}");

    let resolved_url = metadata
        .source_url
        .clone()
        .filter(|url| !url.is_empty())
        .unwrap_or(source_url);

    Ok(Track {
        title,
        url: resolved_url,
        requested_by,
        playback_id: None,
    })
}

/// Append a resolved batch to Songbird and mirror the same order in guild state.
/// The caller must hold the guild operation lock for the whole mutation.
pub(crate) async fn enqueue_resolved_request(
    handler_lock: &Arc<tokio::sync::Mutex<songbird::Call>>,
    guild_id: u64,
    data: &Arc<Data>,
    songbird: &Arc<songbird::Songbird>,
    request: &ResolvedPlayRequest,
) -> bool {
    // Keep the state write lock free while Songbird creates every source.
    let (was_idle, queued_tracks) = {
        let mut handler = handler_lock.lock().await;
        let was_idle = handler.queue().is_empty();
        let client = reqwest::Client::new();
        let mut queued_tracks = request.tracks.clone();

        for track in &mut queued_tracks {
            let songbird_track =
                create_songbird_track(track, guild_id, data, songbird, client.clone());
            handler.enqueue(songbird_track).await;
        }

        (was_idle, queued_tracks)
    };

    let state_arc = data.music_state(guild_id).await;
    state_arc
        .write()
        .await
        .enqueue_batch(queued_tracks, was_idle);
    was_idle
}

pub(crate) fn create_songbird_track(
    track: &mut Track,
    guild_id: u64,
    data: &Arc<Data>,
    songbird: &Arc<songbird::Songbird>,
    client: reqwest::Client,
) -> SongbirdTrack {
    let source = YoutubeDl::new(client, track.url.clone());
    let mut songbird_track = SongbirdTrack::new(source.into());
    let playback_id = songbird_track.uuid.to_string();
    track.playback_id = Some(playback_id.clone());
    songbird_track.events.add_event(
        EventData::new(
            Event::Track(TrackEvent::End),
            MusicTrackEndHandler {
                guild_id,
                data: Arc::clone(data),
                songbird: Arc::clone(songbird),
                playback_id,
            },
        ),
        Duration::ZERO,
    );
    songbird_track
}

pub(crate) async fn reply_for_request(
    ctx: &Context,
    command: &CommandInteraction,
    request: &ResolvedPlayRequest,
    was_idle: bool,
) -> Result<(), serenity::Error> {
    let first_track = request
        .tracks
        .first()
        .expect("resolved playback requests must contain a track");

    match (&request.playlist_title, was_idle) {
        (Some(playlist_title), true) => {
            edit_reply_embed(
                ctx,
                command,
                "🎵 Now Playing",
                &format!(
                    "**{}**\nPlaylist: **{}**\n{} more track(s) queued\nRequested by <@{}>",
                    first_track.title,
                    playlist_title,
                    request.tracks.len().saturating_sub(1),
                    command.user.id,
                ),
                0x57F287,
            )
            .await
        }
        (Some(playlist_title), false) => {
            edit_reply_embed(
                ctx,
                command,
                "➕ Playlist Added to Queue",
                &format!(
                    "**{}**\n{} track(s) added\nRequested by <@{}>",
                    playlist_title,
                    request.tracks.len(),
                    command.user.id,
                ),
                0x5865F2,
            )
            .await
        }
        (None, true) => {
            edit_reply_embed(
                ctx,
                command,
                "🎵 Now Playing",
                &format!(
                    "**{}**\nRequested by <@{}>",
                    first_track.title, command.user.id
                ),
                0x57F287,
            )
            .await
        }
        (None, false) => {
            edit_reply_embed(
                ctx,
                command,
                "➕ Added to Queue",
                &format!(
                    "**{}**\nRequested by <@{}>",
                    first_track.title, command.user.id
                ),
                0x5865F2,
            )
            .await
        }
    }
}

fn is_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

pub(crate) async fn reply_ephemeral(
    ctx: &Context,
    command: &CommandInteraction,
    content: &str,
) -> Result<(), serenity::Error> {
    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await
}

pub(crate) async fn edit_reply(
    ctx: &Context,
    command: &CommandInteraction,
    content: &str,
) -> Result<(), serenity::Error> {
    command
        .edit_response(
            &ctx.http,
            serenity::all::EditInteractionResponse::new().content(content),
        )
        .await
        .map(|_| ())
}

async fn edit_reply_embed(
    ctx: &Context,
    command: &CommandInteraction,
    title: &str,
    description: &str,
    colour: u32,
) -> Result<(), serenity::Error> {
    let embed = CreateEmbed::new()
        .title(title)
        .description(description)
        .colour(colour)
        .footer(CreateEmbedFooter::new("Bobby TaBot"));

    command
        .edit_response(
            &ctx.http,
            serenity::all::EditInteractionResponse::new().embed(embed),
        )
        .await
        .map(|_| ())
}

struct MusicTrackEndHandler {
    guild_id: u64,
    data: Arc<Data>,
    songbird: Arc<songbird::Songbird>,
    playback_id: String,
}

#[serenity::async_trait]
impl SongbirdEventHandler for MusicTrackEndHandler {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let operation_lock = self.data.music_operation_lock(self.guild_id).await;
        let _operation_guard = operation_lock.lock().await;
        let state_arc = {
            let states = self.data.music_states.read().await;
            states.get(&self.guild_id).cloned()
        };

        if let Some(state_arc) = &state_arc {
            let (preplay_url, has_next_music) = {
                let state = state_arc.read().await;
                // Removing a queued Songbird track fires its End event. Ignore
                // those events unless this playback instance is the current song.
                if !state.is_current_playback(&self.playback_id) {
                    return Some(Event::Cancel);
                }

                (state.preplay_url.clone(), !state.queue.is_empty())
            };
            let roll = rand::rng().random_range(0..100);

            let mut preplay_inserted = false;

            if let Some(preplay_url) = preplay_url {
                if has_next_music {
                    let guild_id = serenity::all::GuildId::new(self.guild_id);

                    if let Some(handler_lock) = self.songbird.get(guild_id) {
                        let mut handler = handler_lock.lock().await;
                        let songbird_has_next = handler.queue().len() > 1;

                        if preplay::transition_should_insert(
                            true,
                            has_next_music,
                            songbird_has_next,
                            self.data.preplay_config.chance_percent,
                            roll,
                        ) {
                            let source =
                                YoutubeDl::new(reqwest::Client::new(), preplay_url.clone());
                            let mut preplay_track = SongbirdTrack::new(source.into());
                            preplay_track.events.add_event(
                                EventData::new(
                                    Event::Track(TrackEvent::Error),
                                    PrePlayErrorHandler {
                                        guild_id: self.guild_id,
                                    },
                                ),
                                Duration::ZERO,
                            );
                            preplay_track.events.add_event(
                                EventData::new(
                                    Event::Track(TrackEvent::End),
                                    PrePlayEndHandler {
                                        guild_id: self.guild_id,
                                        data: Arc::clone(&self.data),
                                    },
                                ),
                                Duration::ZERO,
                            );
                            let preplay_handle = handler.enqueue_with_preload(preplay_track, None);
                            let preplay_id = preplay_handle.uuid();
                            let inserted = handler.queue().modify_queue(|queue| {
                                preplay::move_matching_to_next(queue, |track| {
                                    track.uuid() == preplay_id
                                })
                            });

                            if inserted {
                                preplay_inserted = true;
                                info!(
                                    guild_id = self.guild_id,
                                    chance_percent = self.data.preplay_config.chance_percent,
                                    "Inserted pre-play audio before next music track"
                                );
                            } else {
                                error!(
                                    guild_id = self.guild_id,
                                    "Could not position pre-play audio in Songbird queue"
                                );
                                drop(preplay_handle.stop());
                            }
                        }
                    }
                }
            }

            let mut state = state_arc.write().await;
            if !preplay_inserted || !state.begin_preplay() {
                state.advance();
            }
        }

        Some(Event::Cancel)
    }
}

struct PrePlayEndHandler {
    guild_id: u64,
    data: Arc<Data>,
}

#[serenity::async_trait]
impl SongbirdEventHandler for PrePlayEndHandler {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let operation_lock = self.data.music_operation_lock(self.guild_id).await;
        let _operation_guard = operation_lock.lock().await;
        let state_arc = {
            let states = self.data.music_states.read().await;
            states.get(&self.guild_id).cloned()
        };

        if let Some(state_arc) = state_arc {
            let mut state = state_arc.write().await;
            state.finish_preplay();
        }

        Some(Event::Cancel)
    }
}

struct PrePlayErrorHandler {
    guild_id: u64,
}

#[serenity::async_trait]
impl SongbirdEventHandler for PrePlayErrorHandler {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        error!(
            guild_id = self.guild_id,
            "Pre-play audio encountered a playback source error"
        );
        Some(Event::Cancel)
    }
}
