use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, GuildId};
use songbird::{tracks::PlayMode, Songbird};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::{error, info, warn};

use crate::{
    commands::{guild_state::Track, play},
    playlists::PlaylistError,
    Data,
};

#[derive(Clone)]
struct WebState {
    data: Arc<Data>,
    songbird: Arc<Songbird>,
    guild_id: Option<GuildId>,
    voice_channel_id: Option<ChannelId>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueResponse {
    configured: bool,
    connected: bool,
    can_add: bool,
    paused: bool,
    guild_id: Option<String>,
    voice_channel_id: Option<String>,
    current: Option<TrackResponse>,
    queue: Vec<TrackResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackResponse {
    id: String,
    title: String,
    url: String,
    requested_by: Option<String>,
}

#[derive(Deserialize)]
struct AddTrackRequest {
    query: String,
}

#[derive(Deserialize)]
struct QueuePlaylistRequest {
    name: String,
}

#[derive(Serialize)]
struct PlaylistListResponse {
    playlists: Vec<String>,
}

#[derive(Deserialize)]
struct ReorderRequest {
    from: usize,
    to: usize,
}

#[derive(Serialize)]
struct MutationResponse {
    message: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

pub async fn serve(
    data: Arc<Data>,
    songbird: Arc<Songbird>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind_address = std::env::var("WEB_BIND")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse::<SocketAddr>()?;
    let static_dir =
        PathBuf::from(std::env::var("DASHBOARD_DIR").unwrap_or_else(|_| "web/dist".to_string()));
    let index_file = static_dir.join("index.html");

    if !index_file.is_file() {
        warn!(
            path = %index_file.display(),
            "Dashboard files are missing; build the Preact app or set DASHBOARD_DIR"
        );
    }

    let state = WebState {
        data,
        songbird,
        guild_id: env_id("GUILD_ID").map(GuildId::new),
        voice_channel_id: env_id("VOICE_CHANNEL_ID").map(ChannelId::new),
    };

    let static_files = ServeDir::new(&static_dir).fallback(ServeFile::new(index_file));
    let app = Router::new()
        .route(
            "/api/queue",
            get(get_queue).post(add_track).delete(clear_queue),
        )
        .route("/api/queue/reorder", post(reorder_queue))
        .route("/api/queue/{index}", delete(remove_track))
        .route("/api/playlists", get(get_playlists))
        .route("/api/playlists/queue", post(queue_playlist))
        .route("/api/playback/{action}", post(playback_action))
        .fallback_service(static_files)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_address).await?;
    info!("Dashboard listening on http://{bind_address}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_queue(State(app): State<WebState>) -> Json<QueueResponse> {
    let (current, queue) = match app.guild_id {
        Some(guild_id) => {
            let state_arc = {
                let states = app.data.music_states.read().await;
                states.get(&guild_id.get()).cloned()
            };

            match state_arc {
                Some(state_arc) => {
                    let music = state_arc.read().await;
                    (
                        music.current.as_ref().map(|track| track_response(track, 0)),
                        music
                            .queue
                            .iter()
                            .enumerate()
                            .map(|(index, track)| track_response(track, index + 1))
                            .collect(),
                    )
                }
                None => (None, Vec::new()),
            }
        }
        None => (None, Vec::new()),
    };

    let (connected, paused, active_channel_id) = match app.guild_id {
        Some(guild_id) => playback_snapshot(&app.songbird, guild_id).await,
        None => (false, false, None),
    };

    Json(QueueResponse {
        configured: app.guild_id.is_some(),
        connected,
        can_add: app.guild_id.is_some() && (connected || app.voice_channel_id.is_some()),
        paused,
        guild_id: app.guild_id.map(|id| id.get().to_string()),
        voice_channel_id: active_channel_id
            .or_else(|| app.voice_channel_id.map(|id| id.get().to_string())),
        current,
        queue,
    })
}

async fn add_track(
    State(app): State<WebState>,
    Json(payload): Json<AddTrackRequest>,
) -> Result<(StatusCode, Json<MutationResponse>), ApiError> {
    let query = payload.query.trim();
    if query.is_empty() {
        return Err(ApiError::bad_request("Enter a song name or URL."));
    }
    if query.chars().count() > 500 {
        return Err(ApiError::bad_request(
            "The search is too long. Keep it under 500 characters.",
        ));
    }

    let guild_id = configured_guild(&app)?;

    // Resolve network metadata before taking the per-guild mutation lock.
    let request = play::resolve_request(query, None)
        .await
        .map_err(ApiError::bad_request)?;

    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;
    let handler_lock = connected_or_join(&app, guild_id).await?;

    let added_count = request.tracks.len();
    let first_title = request
        .tracks
        .first()
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "Track".to_string());
    let was_idle = play::enqueue_resolved_request(
        &handler_lock,
        guild_id.get(),
        &app.data,
        &app.songbird,
        &request,
    )
    .await;

    let message = match request.playlist_title {
        Some(title) => format!("Added {added_count} tracks from {title}."),
        None if was_idle => format!("Now playing {first_title}."),
        None => format!("Added {first_title} to the queue."),
    };

    Ok((StatusCode::CREATED, Json(MutationResponse { message })))
}

async fn get_playlists(
    State(app): State<WebState>,
) -> Result<Json<PlaylistListResponse>, ApiError> {
    let playlists = app
        .data
        .playlist_library
        .list()
        .await
        .map_err(playlist_api_error)?;

    Ok(Json(PlaylistListResponse { playlists }))
}

async fn queue_playlist(
    State(app): State<WebState>,
    Json(payload): Json<QueuePlaylistRequest>,
) -> Result<(StatusCode, Json<MutationResponse>), ApiError> {
    let guild_id = configured_guild(&app)?;
    let playlist = app
        .data
        .playlist_library
        .load(&payload.name)
        .await
        .map_err(playlist_api_error)?;
    let request = play::resolve_predefined_playlist(&playlist, None)
        .await
        .map_err(ApiError::bad_request)?;
    let added_count = request.tracks.len();
    let first_title = request
        .tracks
        .first()
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "Track".to_string());
    let playlist_name = request
        .playlist_title
        .as_deref()
        .unwrap_or("playlist")
        .to_string();

    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;
    let handler_lock = connected_or_join(&app, guild_id).await?;
    let was_idle = play::enqueue_resolved_request(
        &handler_lock,
        guild_id.get(),
        &app.data,
        &app.songbird,
        &request,
    )
    .await;

    let message = if was_idle {
        format!("Now playing {first_title}; added {added_count} tracks from {playlist_name}.")
    } else {
        format!("Added {added_count} tracks from {playlist_name}.")
    };

    Ok((StatusCode::CREATED, Json(MutationResponse { message })))
}

async fn clear_queue(State(app): State<WebState>) -> Result<Json<MutationResponse>, ApiError> {
    let guild_id = configured_guild(&app)?;
    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;

    if let Some(handler_lock) = app.songbird.get(guild_id) {
        let handler = handler_lock.lock().await;
        let removed = handler
            .queue()
            .modify_queue(|queue| queue.split_off(queue.len().min(1)));
        for track in removed {
            drop(track.stop());
        }
    }

    let state_arc = app.data.music_state(guild_id.get()).await;
    let cleared = state_arc.write().await.clear_queue();
    Ok(Json(MutationResponse {
        message: match cleared {
            0 => "The upcoming queue is already empty.".to_string(),
            1 => "Cleared 1 upcoming track.".to_string(),
            count => format!("Cleared {count} upcoming tracks."),
        },
    }))
}

async fn remove_track(
    State(app): State<WebState>,
    Path(index): Path<usize>,
) -> Result<Json<MutationResponse>, ApiError> {
    let guild_id = configured_guild(&app)?;
    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;
    let state_arc = app.data.music_state(guild_id.get()).await;

    {
        let state = state_arc.read().await;
        if index >= state.queue.len() {
            return Err(ApiError::not_found("That track is no longer in the queue."));
        }
    }

    let handler_lock = app
        .songbird
        .get(guild_id)
        .ok_or_else(|| ApiError::conflict("The bot is not connected to voice."))?;
    let removed = {
        let handler = handler_lock.lock().await;
        handler
            .queue()
            .modify_queue(|queue| queue.remove(index + 1))
    }
    .ok_or_else(|| ApiError::conflict("The playback queue changed. Please try again."))?;
    drop(removed.stop());

    let removed_track = state_arc
        .write()
        .await
        .queue
        .remove(index)
        .ok_or_else(|| ApiError::conflict("The queue changed. Please refresh."))?;

    Ok(Json(MutationResponse {
        message: format!("Removed {}.", removed_track.title),
    }))
}

async fn reorder_queue(
    State(app): State<WebState>,
    Json(payload): Json<ReorderRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    let guild_id = configured_guild(&app)?;
    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;
    let state_arc = app.data.music_state(guild_id.get()).await;

    let queue_len = state_arc.read().await.queue.len();
    if payload.from >= queue_len || payload.to >= queue_len {
        return Err(ApiError::bad_request(
            "The queue changed before that track could be moved.",
        ));
    }
    if payload.from == payload.to {
        return Ok(Json(MutationResponse {
            message: "The track is already there.".to_string(),
        }));
    }

    let handler_lock = app
        .songbird
        .get(guild_id)
        .ok_or_else(|| ApiError::conflict("The bot is not connected to voice."))?;
    let moved = {
        let handler = handler_lock.lock().await;
        handler.queue().modify_queue(|queue| {
            let Some(track) = queue.remove(payload.from + 1) else {
                return false;
            };
            queue.insert(payload.to + 1, track);
            true
        })
    };
    if !moved {
        return Err(ApiError::conflict(
            "The playback queue changed. Please try again.",
        ));
    }

    let mut music = state_arc.write().await;
    let track = music
        .queue
        .remove(payload.from)
        .ok_or_else(|| ApiError::conflict("The queue changed. Please refresh."))?;
    let title = track.title.clone();
    music.queue.insert(payload.to, track);

    Ok(Json(MutationResponse {
        message: format!("Moved {title}."),
    }))
}

async fn playback_action(
    State(app): State<WebState>,
    Path(action): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    let guild_id = configured_guild(&app)?;
    let operation_lock = app.data.music_operation_lock(guild_id.get()).await;
    let _operation_guard = operation_lock.lock().await;
    let handler_lock = app
        .songbird
        .get(guild_id)
        .ok_or_else(|| ApiError::conflict("The bot is not connected to voice."))?;
    let handler = handler_lock.lock().await;
    let queue = handler.queue();

    if queue.is_empty() {
        return Err(ApiError::conflict("There is nothing playing."));
    }

    let message = match action.as_str() {
        "pause" => {
            queue
                .pause()
                .map_err(|error| ApiError::conflict(format!("Could not pause: {error}")))?;
            "Playback paused."
        }
        "resume" => {
            queue
                .resume()
                .map_err(|error| ApiError::conflict(format!("Could not resume: {error}")))?;
            "Playback resumed."
        }
        "skip" => {
            queue
                .skip()
                .map_err(|error| ApiError::conflict(format!("Could not skip: {error}")))?;
            "Skipped to the next track."
        }
        _ => return Err(ApiError::not_found("Unknown playback action.")),
    };

    Ok(Json(MutationResponse {
        message: message.to_string(),
    }))
}

async fn connected_or_join(
    app: &WebState,
    guild_id: GuildId,
) -> Result<Arc<tokio::sync::Mutex<songbird::Call>>, ApiError> {
    if let Some(handler_lock) = app.songbird.get(guild_id) {
        let connected = handler_lock.lock().await.current_channel().is_some();
        if connected {
            return Ok(handler_lock);
        }
    }

    let channel_id = app.voice_channel_id.ok_or_else(|| {
        ApiError::conflict(
            "The bot is not in voice. Set VOICE_CHANNEL_ID so the dashboard can connect it.",
        )
    })?;

    app.songbird
        .join(guild_id, channel_id)
        .await
        .map_err(|error| {
            error!("Dashboard failed to join voice channel: {error}");
            ApiError::conflict(
                "Could not join the configured voice channel. Check the bot's permissions.",
            )
        })
}

async fn playback_snapshot(
    songbird: &Arc<Songbird>,
    guild_id: GuildId,
) -> (bool, bool, Option<String>) {
    let Some(handler_lock) = songbird.get(guild_id) else {
        return (false, false, None);
    };

    let (channel_id, current_handle) = {
        let handler = handler_lock.lock().await;
        (handler.current_channel(), handler.queue().current())
    };
    let paused = match current_handle {
        Some(handle) => handle
            .get_info()
            .await
            .map(|track| matches!(track.playing, PlayMode::Pause))
            .unwrap_or(false),
        None => false,
    };

    (
        channel_id.is_some(),
        paused,
        channel_id.map(|id| id.0.get().to_string()),
    )
}

fn configured_guild(app: &WebState) -> Result<GuildId, ApiError> {
    app.guild_id.ok_or_else(|| {
        ApiError::unavailable("Set GUILD_ID to choose which server this dashboard controls.")
    })
}

fn playlist_api_error(error: PlaylistError) -> ApiError {
    match error {
        PlaylistError::NotFound { name } => {
            ApiError::not_found(format!("The predefined playlist '{name}' was not found."))
        }
        PlaylistError::Empty { name } => {
            ApiError::bad_request(format!("The predefined playlist '{name}' has no tracks."))
        }
        PlaylistError::TooManyTracks { name, maximum } => ApiError::bad_request(format!(
            "The predefined playlist '{name}' has more than {maximum} tracks."
        )),
        PlaylistError::LineTooLong {
            name,
            line_number,
            maximum,
        } => ApiError::bad_request(format!(
            "Playlist '{name}' line {line_number} is longer than {maximum} characters."
        )),
        error @ (PlaylistError::Directory { .. } | PlaylistError::Read { .. }) => {
            error!("Predefined playlists are unavailable: {error}");
            ApiError::unavailable(
                "Predefined playlists are unavailable. Check the configured folder.",
            )
        }
    }
}

fn track_response(track: &Track, fallback_index: usize) -> TrackResponse {
    TrackResponse {
        id: track
            .playback_id
            .clone()
            .unwrap_or_else(|| format!("track-{fallback_index}")),
        title: track.title.clone(),
        url: track.url.clone(),
        requested_by: track.requested_by.map(|id| id.get().to_string()),
    }
}

fn env_id(name: &str) -> Option<u64> {
    let value = std::env::var(name).ok()?;
    match value.trim().parse::<u64>() {
        Ok(id) if id > 0 => Some(id),
        _ => {
            warn!("{name} is not a valid Discord ID; dashboard controls are disabled");
            None
        }
    }
}
