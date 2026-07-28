use std::sync::Arc;

use serenity::{
    all::{CommandInteraction, ResolvedValue},
    client::Context,
};
use tracing::{error, warn};

use crate::{commands::play, playlists::PlaylistError, Data};

/// `/playlist <name>` — queue every entry from a configured text playlist.
pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    data: &Arc<Data>,
) -> Result<(), serenity::Error> {
    let name = match command.data.options().first() {
        Some(option) => match &option.value {
            ResolvedValue::String(value) => value.to_string(),
            _ => return play::reply_ephemeral(ctx, command, "❌ Expected a playlist name.").await,
        },
        None => {
            return play::reply_ephemeral(ctx, command, "❌ Please provide a playlist name.").await
        }
    };

    let guild_id = match command.guild_id {
        Some(id) => id,
        None => {
            return play::reply_ephemeral(
                ctx,
                command,
                "❌ This command can only be used in a server.",
            )
            .await
        }
    };

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
            return play::reply_ephemeral(
                ctx,
                command,
                "❌ You need to be in a voice channel first.",
            )
            .await
        }
    };

    command.defer(&ctx.http).await?;

    let playlist = match data.playlist_library.load(&name).await {
        Ok(playlist) => playlist,
        Err(error) => {
            warn!(playlist = %name, "Could not load predefined playlist: {error}");
            return play::edit_reply(ctx, command, &playlist_error_message(&error)).await;
        }
    };
    let request = match play::resolve_predefined_playlist(&playlist, Some(command.user.id)).await {
        Ok(request) => request,
        Err(message) => return play::edit_reply(ctx, command, &message).await,
    };

    let operation_lock = data.music_operation_lock(guild_id.get()).await;
    let operation_guard = operation_lock.lock().await;
    let songbird = songbird::get(ctx)
        .await
        .expect("Songbird must be registered");
    let handler_lock = match songbird.join(guild_id, channel_id).await {
        Ok(handler) => handler,
        Err(join_error) => {
            error!("Failed to join voice channel for predefined playlist: {join_error}");
            drop(operation_guard);
            return play::edit_reply(
                ctx,
                command,
                "❌ Failed to join your voice channel. Do I have permission to connect?",
            )
            .await;
        }
    };

    let was_idle =
        play::enqueue_resolved_request(&handler_lock, guild_id.get(), data, &songbird, &request)
            .await;
    drop(operation_guard);

    play::reply_for_request(ctx, command, &request, was_idle).await
}

fn playlist_error_message(error: &PlaylistError) -> String {
    match error {
        PlaylistError::NotFound { name } => {
            format!("❌ The predefined playlist `{name}` was not found.")
        }
        PlaylistError::Empty { name } => {
            format!("❌ The predefined playlist `{name}` has no tracks.")
        }
        PlaylistError::TooManyTracks { name, maximum } => {
            format!("❌ The predefined playlist `{name}` has more than {maximum} tracks.")
        }
        PlaylistError::LineTooLong {
            name,
            line_number,
            maximum,
        } => {
            format!("❌ Playlist `{name}` line {line_number} is longer than {maximum} characters.")
        }
        PlaylistError::Directory { .. } | PlaylistError::Read { .. } => {
            "❌ Predefined playlists are unavailable. Check the configured playlist folder."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::playlist_error_message;
    use crate::playlists::PlaylistError;

    #[test]
    fn hides_server_paths_for_directory_errors() {
        let error = PlaylistError::Directory {
            path: "private/playlists".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };

        assert!(!playlist_error_message(&error).contains("private/playlists"));
    }
}
