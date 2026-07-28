import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import {
  Check,
  ChevronDown,
  ChevronUp,
  CircleAlert,
  ExternalLink,
  GripVertical,
  Headphones,
  ListMusic,
  LoaderCircle,
  Moon,
  Music2,
  Pause,
  Play,
  Plus,
  Radio,
  SkipForward,
  Sparkles,
  Sun,
  Trash2,
  X,
} from "lucide-preact";

type Track = {
  id: string;
  title: string;
  url: string;
  requestedBy: string | null;
};

type QueueData = {
  configured: boolean;
  connected: boolean;
  canAdd: boolean;
  paused: boolean;
  guildId: string | null;
  voiceChannelId: string | null;
  current: Track | null;
  queue: Track[];
};

type PlaylistData = {
  playlists: string[];
};

type Notice = {
  kind: "success" | "error";
  message: string;
};

const EMPTY_QUEUE: QueueData = {
  configured: false,
  connected: false,
  canAdd: false,
  paused: false,
  guildId: null,
  voiceChannelId: null,
  current: null,
  queue: [],
};

function initialTheme(): "light" | "dark" {
  const saved = localStorage.getItem("bobby-theme");
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...init?.headers,
    },
  });
  const payload = (await response.json().catch(() => ({}))) as {
    error?: string;
  };

  if (!response.ok) {
    throw new Error(payload.error || "Something went wrong. Please try again.");
  }
  return payload as T;
}

export function App() {
  const [theme, setTheme] = useState(initialTheme);
  const [queueData, setQueueData] = useState<QueueData>(EMPTY_QUEUE);
  const [query, setQuery] = useState("");
  const [playlists, setPlaylists] = useState<string[]>([]);
  const [selectedPlaylist, setSelectedPlaylist] = useState("");
  const [loading, setLoading] = useState(true);
  const [playlistsLoading, setPlaylistsLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [confirmClear, setConfirmClear] = useState(false);
  const [dragging, setDragging] = useState<number | null>(null);
  const noticeTimer = useRef<number | undefined>(undefined);

  const refresh = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const next = await api<QueueData>("/api/queue");
      setQueueData(next);
    } catch (error) {
      if (!quiet) showNotice("error", errorMessage(error));
    } finally {
      if (!quiet) setLoading(false);
    }
  }, []);

  const refreshPlaylists = useCallback(async (quiet = false) => {
    if (!quiet) setPlaylistsLoading(true);
    try {
      const next = await api<PlaylistData>("/api/playlists");
      setPlaylists(next.playlists);
      setSelectedPlaylist((current) =>
        current && next.playlists.includes(current)
          ? current
          : (next.playlists[0] ?? ""),
      );
    } catch (error) {
      if (!quiet) showNotice("error", errorMessage(error));
    } finally {
      if (!quiet) setPlaylistsLoading(false);
    }
  }, []);

  const showNotice = (kind: Notice["kind"], message: string) => {
    setNotice({ kind, message });
    window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 3800);
  };

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("bobby-theme", theme);
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", theme === "dark" ? "#2b2c3c" : "#f4eee2");
  }, [theme]);

  useEffect(() => {
    void refresh();
    void refreshPlaylists();
    const interval = window.setInterval(() => {
      void refresh(true);
      void refreshPlaylists(true);
    }, 5000);
    return () => window.clearInterval(interval);
  }, [refresh, refreshPlaylists]);

  useEffect(
    () => () => window.clearTimeout(noticeTimer.current),
    [],
  );

  const runAction = async (
    name: string,
    action: () => Promise<{ message: string }>,
    refreshDelay = 0,
  ): Promise<boolean> => {
    setBusy(name);
    try {
      const result = await action();
      showNotice("success", result.message);
      if (refreshDelay) {
        window.setTimeout(() => void refresh(true), refreshDelay);
      } else {
        await refresh(true);
      }
      return true;
    } catch (error) {
      showNotice("error", errorMessage(error));
      await refresh(true);
      return false;
    } finally {
      setBusy(null);
    }
  };

  const addTrack = async (event: SubmitEvent) => {
    event.preventDefault();
    const value = query.trim();
    if (!value) return;

    const added = await runAction("add", () =>
      api("/api/queue", {
        method: "POST",
        body: JSON.stringify({ query: value }),
      }),
    );
    if (added) setQuery("");
  };

  const queuePlaylist = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!selectedPlaylist) return;

    const queued = await runAction("playlist", () =>
      api("/api/playlists/queue", {
        method: "POST",
        body: JSON.stringify({ name: selectedPlaylist }),
      }),
    );
    if (queued) await refreshPlaylists(true);
  };

  const reorder = async (from: number, to: number) => {
    if (from === to || busy) return;
    await runAction(`move-${from}`, () =>
      api("/api/queue/reorder", {
        method: "POST",
        body: JSON.stringify({ from, to }),
      }),
    );
  };

  const remove = (index: number) =>
    runAction(`remove-${index}`, () =>
      api(`/api/queue/${index}`, { method: "DELETE" }),
    );

  const clear = async () => {
    await runAction("clear", () =>
      api("/api/queue", { method: "DELETE" }),
    );
    setConfirmClear(false);
  };

  const playback = (action: "pause" | "resume" | "skip") =>
    runAction(
      action,
      () => api(`/api/playback/${action}`, { method: "POST" }),
      action === "skip" ? 650 : 0,
    );

  const hasMusic = Boolean(queueData.current || queueData.queue.length);

  return (
    <div class="app-shell">
      <AnimatedBackground />

      <header class="topbar">
        <a class="brand" href="/" aria-label="Bobby TaBot dashboard">
          <span class="brand-mark">
            <Music2 size={20} strokeWidth={2.4} />
          </span>
          <span>
            <strong>Bobby TaBot</strong>
            <small>Music control</small>
          </span>
        </a>

        <div class="header-actions">
          <div
            class={`connection-pill ${queueData.connected ? "is-online" : ""}`}
            title={
              queueData.connected
                ? "Connected to a Discord voice channel"
                : "Not connected to voice"
            }
          >
            <span class="status-dot" />
            <span>{queueData.connected ? "Live in voice" : "Standing by"}</span>
          </div>
          <button
            class="icon-button theme-toggle"
            type="button"
            aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
            title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
            onClick={() =>
              setTheme((value) => (value === "dark" ? "light" : "dark"))
            }
          >
            {theme === "dark" ? <Sun size={19} /> : <Moon size={19} />}
          </button>
        </div>
      </header>

      <main>
        <section class="intro">
          <div class="eyebrow">
            <Sparkles size={14} />
            Your server, your soundtrack
          </div>
          <h1>Keep the good songs coming.</h1>
          <p>
            Search for a track, paste a link, and shape the queue without
            interrupting the vibe.
          </p>
        </section>

        <form class="add-track-card" onSubmit={addTrack}>
          <div class="search-icon" aria-hidden="true">
            <Headphones size={21} />
          </div>
          <label class="visually-hidden" for="track-query">
            Song name or URL
          </label>
          <input
            id="track-query"
            value={query}
            onInput={(event) => setQuery(event.currentTarget.value)}
            placeholder="Search a song or paste a YouTube link…"
            autoComplete="off"
            disabled={busy === "add" || !queueData.canAdd}
          />
          <button
            class="primary-button"
            type="submit"
            disabled={!query.trim() || busy === "add" || !queueData.canAdd}
          >
            {busy === "add" ? (
              <LoaderCircle class="spin" size={19} />
            ) : (
              <Plus size={19} />
            )}
            <span>{busy === "add" ? "Finding…" : "Add track"}</span>
          </button>
        </form>

        <form class="playlist-card" onSubmit={queuePlaylist}>
          <div class="playlist-icon" aria-hidden="true">
            <ListMusic size={20} />
          </div>
          <div class="playlist-select-wrap">
            <label for="playlist-name">Predefined playlist</label>
            <select
              id="playlist-name"
              value={selectedPlaylist}
              onChange={(event) => setSelectedPlaylist(event.currentTarget.value)}
              disabled={
                playlistsLoading ||
                busy === "playlist" ||
                !queueData.canAdd ||
                playlists.length === 0
              }
            >
              {!selectedPlaylist && (
                <option value="">
                  {playlistsLoading ? "Loading playlists…" : "No playlists found"}
                </option>
              )}
              {playlists.map((playlist) => (
                <option value={playlist} key={playlist}>
                  {playlist}
                </option>
              ))}
            </select>
          </div>
          <button
            class="secondary-button"
            type="submit"
            disabled={
              !selectedPlaylist ||
              playlistsLoading ||
              busy === "playlist" ||
              !queueData.canAdd
            }
          >
            {busy === "playlist" ? (
              <LoaderCircle class="spin" size={18} />
            ) : (
              <Plus size={18} />
            )}
            <span>{busy === "playlist" ? "Queueing…" : "Queue playlist"}</span>
          </button>
        </form>

        {!queueData.canAdd && !loading && (
          <div class="setup-note" role="status">
            <CircleAlert size={18} />
            <span>
              {!queueData.configured
                ? "Set GUILD_ID to connect this dashboard to your Discord server."
                : "Join the bot to voice once, or set VOICE_CHANNEL_ID so the dashboard can connect it."}
            </span>
          </div>
        )}

        <section class="dashboard-grid" aria-busy={loading}>
          <NowPlaying
            current={queueData.current}
            paused={queueData.paused}
            busy={busy}
            onPause={() =>
              void playback(queueData.paused ? "resume" : "pause")
            }
            onSkip={() => void playback("skip")}
          />

          <section class="queue-card">
            <div class="section-heading">
              <div>
                <div class="title-row">
                  <ListMusic size={20} />
                  <h2>Up next</h2>
                  <span class="count-badge">{queueData.queue.length}</span>
                </div>
                <p>Drag tracks or use the arrows to change the order.</p>
              </div>

              {queueData.queue.length > 0 &&
                (confirmClear ? (
                  <div class="confirm-clear">
                    <span>Clear all?</span>
                    <button
                      class="small-button danger"
                      type="button"
                      onClick={() => void clear()}
                      disabled={busy === "clear"}
                    >
                      {busy === "clear" ? (
                        <LoaderCircle class="spin" size={15} />
                      ) : (
                        <Check size={15} />
                      )}
                      Yes
                    </button>
                    <button
                      class="small-icon-button"
                      type="button"
                      aria-label="Cancel clear"
                      title="Cancel"
                      onClick={() => setConfirmClear(false)}
                    >
                      <X size={16} />
                    </button>
                  </div>
                ) : (
                  <button
                    class="clear-button"
                    type="button"
                    onClick={() => setConfirmClear(true)}
                  >
                    <Trash2 size={16} />
                    Clear
                  </button>
                ))}
            </div>

            <div class="queue-content">
              {loading ? (
                <QueueSkeleton />
              ) : queueData.queue.length ? (
                <ol class="queue-list">
                  {queueData.queue.map((track, index) => (
                    <li
                      class={`queue-item ${dragging === index ? "is-dragging" : ""
                        }`}
                      key={track.id}
                      draggable={!busy}
                      onDragStart={(event) => {
                        const transfer = event.dataTransfer;
                        if (!transfer) return;
                        setDragging(index);
                        transfer.effectAllowed = "move";
                        transfer.setData("text/plain", index.toString());
                      }}
                      onDragEnd={() => setDragging(null)}
                      onDragOver={(event) => {
                        event.preventDefault();
                        if (event.dataTransfer) {
                          event.dataTransfer.dropEffect = "move";
                        }
                      }}
                      onDrop={(event) => {
                        event.preventDefault();
                        const from = Number(
                          event.dataTransfer?.getData("text/plain"),
                        );
                        setDragging(null);
                        if (Number.isInteger(from)) void reorder(from, index);
                      }}
                    >
                      <div class="drag-handle" title="Drag to reorder">
                        <GripVertical size={18} />
                      </div>
                      <span class="track-number">
                        {String(index + 1).padStart(2, "0")}
                      </span>
                      <div class="track-copy">
                        <a
                          href={track.url}
                          target="_blank"
                          rel="noreferrer"
                          title={`Open ${track.title}`}
                        >
                          <span>{track.title}</span>
                          <ExternalLink size={13} />
                        </a>
                        <small>
                          {track.requestedBy
                            ? "Requested on Discord"
                            : "Added from dashboard"}
                        </small>
                      </div>
                      <div class="track-actions">
                        <button
                          class="small-icon-button"
                          type="button"
                          aria-label={`Move ${track.title} up`}
                          title="Move up"
                          disabled={index === 0 || Boolean(busy)}
                          onClick={() => void reorder(index, index - 1)}
                        >
                          <ChevronUp size={17} />
                        </button>
                        <button
                          class="small-icon-button"
                          type="button"
                          aria-label={`Move ${track.title} down`}
                          title="Move down"
                          disabled={
                            index === queueData.queue.length - 1 || Boolean(busy)
                          }
                          onClick={() => void reorder(index, index + 1)}
                        >
                          <ChevronDown size={17} />
                        </button>
                        <button
                          class="small-icon-button remove-button"
                          type="button"
                          aria-label={`Remove ${track.title}`}
                          title="Remove"
                          disabled={Boolean(busy)}
                          onClick={() => void remove(index)}
                        >
                          {busy === `remove-${index}` ? (
                            <LoaderCircle class="spin" size={16} />
                          ) : (
                            <X size={17} />
                          )}
                        </button>
                      </div>
                    </li>
                  ))}
                </ol>
              ) : (
                <div class="empty-queue">
                  <span class="empty-icon">
                    <Radio size={25} />
                  </span>
                  <h3>The queue is wide open</h3>
                  <p>Add a track above and let Bobby handle the rest.</p>
                </div>
              )}
            </div>

            <footer class="queue-footer">
              <span>
                {hasMusic
                  ? `${queueData.queue.length + (queueData.current ? 1 : 0)} ${queueData.queue.length +
                    (queueData.current ? 1 : 0) ===
                    1
                    ? "track"
                    : "tracks"
                  } in this session`
                  : "Ready when you are"}
              </span>
              <span class="auto-refresh">
                <span />
                Syncs automatically
              </span>
            </footer>
          </section>
        </section>
      </main>

      <footer class="page-footer">
        <span>Bobby TaBot</span>
        <span aria-hidden="true">•</span>
        <span>Queue beautifully.</span>
      </footer>

      {notice && (
        <div class={`toast ${notice.kind}`} role="status" aria-live="polite">
          {notice.kind === "success" ? (
            <Check size={18} />
          ) : (
            <CircleAlert size={18} />
          )}
          <span>{notice.message}</span>
          <button
            type="button"
            aria-label="Dismiss notification"
            onClick={() => setNotice(null)}
          >
            <X size={15} />
          </button>
        </div>
      )}
    </div>
  );
}

function NowPlaying({
  current,
  paused,
  busy,
  onPause,
  onSkip,
}: {
  current: Track | null;
  paused: boolean;
  busy: string | null;
  onPause: () => void;
  onSkip: () => void;
}) {
  return (
    <section class="now-playing-card">
      <div class="now-playing-label">
        <span class={`pulse-dot ${current && !paused ? "is-playing" : ""}`} />
        {current ? (paused ? "Paused" : "Now playing") : "Player"}
      </div>

      <div class={`record-wrap ${current && !paused ? "is-spinning" : ""}`}>
        <div class="record">
          <div class="record-groove groove-one" />
          <div class="record-groove groove-two" />
          <div class="record-label">
            <Music2 size={25} />
          </div>
        </div>
        <div class="record-shadow" />
      </div>

      <div class="current-copy">
        {current ? (
          <>
            <a href={current.url} target="_blank" rel="noreferrer">
              <h2>{current.title}</h2>
              <ExternalLink size={15} />
            </a>
            <p>
              {current.requestedBy
                ? "Requested from Discord"
                : "Added from the dashboard"}
            </p>
          </>
        ) : (
          <>
            <h2>Nothing playing yet</h2>
            <p>Your next favorite track can start right here.</p>
          </>
        )}
      </div>

      <div class="player-controls">
        <button
          class="play-button"
          type="button"
          aria-label={paused ? "Resume playback" : "Pause playback"}
          title={paused ? "Resume" : "Pause"}
          disabled={!current || Boolean(busy)}
          onClick={onPause}
        >
          {busy === "pause" || busy === "resume" ? (
            <LoaderCircle class="spin" size={22} />
          ) : paused ? (
            <Play size={22} fill="currentColor" />
          ) : (
            <Pause size={22} fill="currentColor" />
          )}
        </button>
        <button
          class="skip-button"
          type="button"
          aria-label="Skip current track"
          title="Skip"
          disabled={!current || Boolean(busy)}
          onClick={onSkip}
        >
          {busy === "skip" ? (
            <LoaderCircle class="spin" size={20} />
          ) : (
            <SkipForward size={21} fill="currentColor" />
          )}
        </button>
      </div>
    </section>
  );
}

function QueueSkeleton() {
  return (
    <div class="queue-skeleton" aria-label="Loading queue">
      {[0, 1, 2].map((item) => (
        <div class="skeleton-row" key={item}>
          <span />
          <div>
            <i />
            <i />
          </div>
        </div>
      ))}
    </div>
  );
}

function AnimatedBackground() {
  return (
    <div class="animated-background" aria-hidden="true">
      <div class="orb orb-one" />
      <div class="orb orb-two" />
      <div class="wave wave-one" />
      <div class="wave wave-two" />
      <div class="wave wave-three" />
    </div>
  );
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : "Something went wrong.";
}
