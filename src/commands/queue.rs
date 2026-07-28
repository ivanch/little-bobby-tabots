use std::sync::Arc;

use serenity::{
    all::{
        CommandInteraction, CreateEmbed, CreateInteractionResponse,
        CreateInteractionResponseMessage,
    },
    client::Context,
};

use crate::{commands::guild_state::GuildMusicState, Data};

/// /queue — show the current track and the upcoming queue.
pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    data: &Arc<Data>,
) -> Result<(), serenity::Error> {
    let guild_id = match command.guild_id {
        Some(id) => id,
        None => {
            return reply_text(
                ctx,
                command,
                "❌ This command can only be used in a server.",
                true,
            )
            .await;
        }
    };

    let states = data.music_states.read().await;
    let state_arc = match states.get(&guild_id.get()) {
        Some(s) => s.clone(),
        None => {
            return reply_text(ctx, command, "📭 The queue is empty.", false).await;
        }
    };

    let state = state_arc.read().await;

    if !state.preplay_active && state.current.is_none() && state.queue.is_empty() {
        return reply_text(ctx, command, "📭 The queue is empty.", false).await;
    }

    let description = build_queue_description(&state);

    let embed = CreateEmbed::new()
        .title("📋 Queue")
        .description(description)
        .colour(0x5865F2)
        .footer(serenity::all::CreateEmbedFooter::new(format!(
            "{} track(s) in queue",
            state.queue.len()
        )));

    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .ephemeral(false),
            ),
        )
        .await
}

fn build_queue_description(state: &GuildMusicState) -> String {
    let mut description = String::new();

    if state.preplay_active {
        description
            .push_str("**🔊 Pre-play Audio**\nPlaying the configured between-track clip.\n\n");
    } else if let Some(current) = &state.current {
        description.push_str(&format!(
            "**🎵 Now Playing**\n[{}]({})\nRequested by {}\n\n",
            current.title,
            current.url,
            requester_label(current.requested_by)
        ));
    }

    if state.queue.is_empty() {
        description.push_str("*No more tracks queued.*");
    } else {
        description.push_str("**Up Next**\n");
        for (i, track) in state.queue.iter().enumerate() {
            description.push_str(&format!(
                "`{}`. [{}]({}) — {}\n",
                i + 1,
                track.title,
                track.url,
                requester_label(track.requested_by)
            ));
            // Discord embed description limit is 4096 chars; stop early if needed
            if description.len() > 3800 {
                description.push_str("*…and more*");
                break;
            }
        }
    }

    description
}

fn requester_label(requested_by: Option<serenity::all::UserId>) -> String {
    requested_by
        .map(|user_id| format!("<@{user_id}>"))
        .unwrap_or_else(|| "Dashboard".to_string())
}

async fn reply_text(
    ctx: &Context,
    command: &CommandInteraction,
    content: &str,
    ephemeral: bool,
) -> Result<(), serenity::Error> {
    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(ephemeral),
            ),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::build_queue_description;
    use crate::commands::guild_state::{GuildMusicState, Track};
    use serenity::all::UserId;

    fn track(title: &str) -> Track {
        Track {
            title: title.to_string(),
            url: format!("https://example.com/{title}"),
            requested_by: Some(UserId::new(1)),
            playback_id: None,
        }
    }

    #[test]
    fn queue_description_reports_active_preplay_before_next_music() {
        let mut state = GuildMusicState::new();
        state.current = Some(track("ended"));
        state.enqueue(track("next"));
        assert!(state.begin_preplay());

        let description = build_queue_description(&state);

        assert!(description.contains("Pre-play Audio"));
        assert!(description.contains("[next](https://example.com/next)"));
        assert!(!description.contains("Now Playing"));
        assert!(!description.contains("[ended](https://example.com/ended)"));
    }
}
