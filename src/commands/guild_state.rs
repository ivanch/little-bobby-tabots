use std::collections::VecDeque;

use serenity::model::id::UserId;

/// State for a single guild: current track + upcoming queue.
/// One of these is created per guild on first use and stored in `Data`.
pub struct GuildMusicState {
    pub queue: VecDeque<Track>,
    pub current: Option<Track>,
    /// Guild-specific pre-play URL. `None` means the feature is disabled.
    pub preplay_url: Option<String>,
    /// Whether an inserted pre-play clip is currently ahead of the next music track.
    pub preplay_active: bool,
}

impl GuildMusicState {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            preplay_url: None,
            preplay_active: false,
        }
    }

    /// Add a track to the end of the queue.
    pub fn enqueue(&mut self, track: Track) {
        self.queue.push_back(track);
    }

    /// Record an ordered group of tracks that was appended to Songbird's queue.
    /// If playback was idle, the first track becomes current and the remainder
    /// become the upcoming queue.
    pub fn enqueue_batch(&mut self, tracks: Vec<Track>, was_idle: bool) {
        let mut tracks = tracks.into_iter();

        if was_idle {
            self.preplay_active = false;
            self.current = tracks.next();
        }

        self.queue.extend(tracks);
    }

    /// Advance to the next track: returns it if the queue is non-empty.
    pub fn advance(&mut self) -> Option<Track> {
        self.preplay_active = false;
        let next = self.queue.pop_front();
        self.current = next.clone();
        next
    }

    /// Mark the inserted pre-play clip as current without consuming the next song.
    pub fn begin_preplay(&mut self) -> bool {
        if self.queue.is_empty() {
            return false;
        }

        self.current = None;
        self.preplay_active = true;
        true
    }

    /// Finish the active pre-play clip and promote the next song to current.
    pub fn finish_preplay(&mut self) -> Option<Track> {
        if !self.preplay_active {
            return None;
        }

        self.advance()
    }

    /// Clear current track and queue (used by /leave).
    pub fn clear(&mut self) {
        self.current = None;
        self.queue.clear();
        self.preplay_active = false;
    }

    /// Clear only upcoming tracks, preserving whatever is currently playing.
    pub fn clear_queue(&mut self) -> usize {
        let cleared = self.queue.len();
        self.queue.clear();
        cleared
    }

    /// Return whether an event belongs to the current music track.
    pub fn is_current_playback(&self, playback_id: &str) -> bool {
        self.current
            .as_ref()
            .and_then(|track| track.playback_id.as_deref())
            == Some(playback_id)
    }

    /// Enable pre-play audio or replace the guild's current URL.
    pub fn enable_preplay(&mut self, url: String) {
        self.preplay_url = Some(url);
    }

    /// Disable future pre-play audio without changing music playback state.
    pub fn disable_preplay(&mut self) -> bool {
        self.preplay_url.take().is_some()
    }
}

/// Metadata for a single track in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    /// The resolved URL that yt-dlp will stream from.
    pub url: String,
    /// Discord requester, or `None` when the track came from the web dashboard.
    pub requested_by: Option<UserId>,
    /// Songbird's unique ID for this playback instance.
    pub playback_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{GuildMusicState, Track};
    use serenity::model::id::UserId;

    fn track(title: &str) -> Track {
        Track {
            title: title.to_string(),
            url: format!("https://example.com/{title}"),
            requested_by: Some(UserId::new(1)),
            playback_id: None,
        }
    }

    #[test]
    fn enqueue_batch_starts_first_track_when_idle() {
        let mut state = GuildMusicState::new();
        state.enqueue_batch(vec![track("one"), track("two"), track("three")], true);

        assert_eq!(
            state.current.as_ref().map(|track| &track.title),
            Some(&"one".to_string())
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
    }

    #[test]
    fn enqueue_batch_appends_when_a_track_is_playing() {
        let mut state = GuildMusicState::new();
        state.current = Some(track("current"));
        state.enqueue(track("existing"));

        state.enqueue_batch(vec![track("one"), track("two")], false);

        assert_eq!(
            state.current.as_ref().map(|track| &track.title),
            Some(&"current".to_string())
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["existing", "one", "two"]
        );
    }

    #[test]
    fn clear_removes_current_and_queued_tracks() {
        let mut state = GuildMusicState::new();
        state.current = Some(track("current"));
        state.enqueue(track("queued"));
        state.enable_preplay("https://youtu.be/preplay".to_string());

        state.clear();

        assert!(state.current.is_none());
        assert!(state.queue.is_empty());
        assert!(!state.preplay_active);
        assert_eq!(
            state.preplay_url.as_deref(),
            Some("https://youtu.be/preplay")
        );
    }

    #[test]
    fn clear_queue_preserves_current_track_and_preplay_setting() {
        let mut state = GuildMusicState::new();
        let mut current = track("current");
        current.playback_id = Some("current-id".to_string());
        let mut queued = track("one");
        queued.playback_id = Some("queued-id".to_string());
        state.current = Some(current);
        state.enqueue(queued);
        state.enqueue(track("two"));
        state.enable_preplay("https://youtu.be/preplay".to_string());

        let cleared = state.clear_queue();

        assert_eq!(cleared, 2);
        assert_eq!(
            state.current.as_ref().map(|track| track.title.as_str()),
            Some("current")
        );
        assert!(state.queue.is_empty());
        assert!(state.is_current_playback("current-id"));
        assert!(!state.is_current_playback("queued-id"));
        assert_eq!(
            state.preplay_url.as_deref(),
            Some("https://youtu.be/preplay")
        );
    }

    #[test]
    fn preplay_can_be_enabled_replaced_and_disabled() {
        let mut state = GuildMusicState::new();
        assert!(state.preplay_url.is_none());

        state.enable_preplay("https://youtu.be/one".to_string());
        state.enable_preplay("https://youtu.be/two".to_string());
        assert_eq!(state.preplay_url.as_deref(), Some("https://youtu.be/two"));

        assert!(state.disable_preplay());
        assert!(!state.disable_preplay());
        assert!(state.preplay_url.is_none());
    }

    #[test]
    fn preplay_phase_keeps_next_music_queued_until_clip_ends() {
        let mut state = GuildMusicState::new();
        state.current = Some(track("current"));
        state.enqueue(track("next"));

        assert!(state.begin_preplay());
        assert!(state.preplay_active);
        assert!(state.current.is_none());
        assert_eq!(
            state.queue.front().map(|track| track.title.as_str()),
            Some("next")
        );

        let next = state.finish_preplay();
        assert!(!state.preplay_active);
        assert_eq!(
            next.as_ref().map(|track| track.title.as_str()),
            Some("next")
        );
        assert_eq!(
            state.current.as_ref().map(|track| track.title.as_str()),
            Some("next")
        );
        assert!(state.queue.is_empty());
    }

    #[test]
    fn preplay_cannot_start_without_an_upcoming_song() {
        let mut state = GuildMusicState::new();
        state.current = Some(track("current"));

        assert!(!state.begin_preplay());
        assert!(!state.preplay_active);
        assert_eq!(
            state.current.as_ref().map(|track| track.title.as_str()),
            Some("current")
        );
    }
}
