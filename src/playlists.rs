use std::{fmt, path::PathBuf};

use tokio::fs;

/// Maximum number of music entries a predefined playlist may contain.
pub const MAX_PREDEFINED_PLAYLIST_TRACKS: usize = 100;
/// Maximum size of one search query or URL in a playlist file.
pub const MAX_PLAYLIST_LINE_LENGTH: usize = 500;

/// A configured on-disk library of predefined playlists.
#[derive(Clone, Debug)]
pub struct PlaylistLibrary {
    root: PathBuf,
}

impl PlaylistLibrary {
    pub fn from_env() -> Self {
        Self {
            root: PathBuf::from(
                std::env::var("PLAYLISTS_DIR").unwrap_or_else(|_| "playlists".to_string()),
            ),
        }
    }

    /// Return the available direct-child `.txt` playlists in filename order.
    pub async fn list(&self) -> Result<Vec<String>, PlaylistError> {
        Ok(self
            .files()
            .await?
            .into_iter()
            .map(|file| file.name)
            .collect())
    }

    /// Read and validate one named playlist without allowing path traversal.
    pub async fn load(&self, requested_name: &str) -> Result<Playlist, PlaylistError> {
        let name = requested_name.trim();
        if name.is_empty() {
            return Err(PlaylistError::NotFound {
                name: requested_name.to_string(),
            });
        }

        let file = self
            .files()
            .await?
            .into_iter()
            .find(|file| file.name == name)
            .ok_or_else(|| PlaylistError::NotFound {
                name: name.to_string(),
            })?;
        let contents =
            fs::read_to_string(&file.path)
                .await
                .map_err(|source| PlaylistError::Read {
                    name: file.name.clone(),
                    source,
                })?;

        parse_playlist(&file.name, &contents)
    }

    async fn files(&self) -> Result<Vec<PlaylistFile>, PlaylistError> {
        let mut directory =
            fs::read_dir(&self.root)
                .await
                .map_err(|source| PlaylistError::Directory {
                    path: self.root.clone(),
                    source,
                })?;
        let mut files = Vec::new();

        while let Some(entry) =
            directory
                .next_entry()
                .await
                .map_err(|source| PlaylistError::Directory {
                    path: self.root.clone(),
                    source,
                })?
        {
            let path = entry.path();
            let is_playlist_file = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"));
            if !is_playlist_file {
                continue;
            }

            let metadata =
                fs::metadata(&path)
                    .await
                    .map_err(|source| PlaylistError::Directory {
                        path: self.root.clone(),
                        source,
                    })?;
            if !metadata.is_file() {
                continue;
            }

            let Some(name) = path
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
            else {
                continue;
            };

            files.push(PlaylistFile { name, path });
        }

        files.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(files)
    }
}

impl Default for PlaylistLibrary {
    fn default() -> Self {
        Self::from_env()
    }
}

#[derive(Debug)]
struct PlaylistFile {
    name: String,
    path: PathBuf,
}

/// Parsed entries from one predefined playlist file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub line_number: usize,
    pub query: String,
}

#[derive(Debug)]
pub enum PlaylistError {
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    Read {
        name: String,
        source: std::io::Error,
    },
    NotFound {
        name: String,
    },
    Empty {
        name: String,
    },
    TooManyTracks {
        name: String,
        maximum: usize,
    },
    LineTooLong {
        name: String,
        line_number: usize,
        maximum: usize,
    },
}

impl fmt::Display for PlaylistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory { path, source } => {
                write!(
                    formatter,
                    "could not read playlist directory {}: {source}",
                    path.display()
                )
            }
            Self::Read { name, source } => {
                write!(formatter, "could not read playlist '{name}': {source}")
            }
            Self::NotFound { name } => write!(formatter, "playlist '{name}' was not found"),
            Self::Empty { name } => write!(formatter, "playlist '{name}' has no tracks"),
            Self::TooManyTracks { name, maximum } => {
                write!(
                    formatter,
                    "playlist '{name}' contains more than {maximum} tracks"
                )
            }
            Self::LineTooLong {
                name,
                line_number,
                maximum,
            } => write!(
                formatter,
                "playlist '{name}' line {line_number} is longer than {maximum} characters"
            ),
        }
    }
}

impl std::error::Error for PlaylistError {}

fn parse_playlist(name: &str, contents: &str) -> Result<Playlist, PlaylistError> {
    let mut entries = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let query = line.trim();
        if query.is_empty() {
            continue;
        }
        if query.chars().count() > MAX_PLAYLIST_LINE_LENGTH {
            return Err(PlaylistError::LineTooLong {
                name: name.to_string(),
                line_number: index + 1,
                maximum: MAX_PLAYLIST_LINE_LENGTH,
            });
        }

        entries.push(PlaylistEntry {
            line_number: index + 1,
            query: query.to_string(),
        });
        if entries.len() > MAX_PREDEFINED_PLAYLIST_TRACKS {
            return Err(PlaylistError::TooManyTracks {
                name: name.to_string(),
                maximum: MAX_PREDEFINED_PLAYLIST_TRACKS,
            });
        }
    }

    if entries.is_empty() {
        return Err(PlaylistError::Empty {
            name: name.to_string(),
        });
    }

    Ok(Playlist {
        name: name.to_string(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        parse_playlist, PlaylistError, PlaylistLibrary, MAX_PLAYLIST_LINE_LENGTH,
        MAX_PREDEFINED_PLAYLIST_TRACKS,
    };

    #[test]
    fn parses_trimmed_entries_in_file_order() {
        let playlist = parse_playlist("focus", "\n  First song  \r\n\r\nhttps://example.com/two\n")
            .expect("playlist should parse");

        assert_eq!(playlist.name, "focus");
        assert_eq!(playlist.entries.len(), 2);
        assert_eq!(playlist.entries[0].line_number, 2);
        assert_eq!(playlist.entries[0].query, "First song");
        assert_eq!(playlist.entries[1].line_number, 4);
        assert_eq!(playlist.entries[1].query, "https://example.com/two");
    }

    #[test]
    fn rejects_empty_playlist() {
        let error = parse_playlist("empty", " \n\r\n").expect_err("playlist should be empty");

        assert!(matches!(error, PlaylistError::Empty { .. }));
    }

    #[test]
    fn rejects_a_line_that_is_too_long() {
        let long_line = "x".repeat(MAX_PLAYLIST_LINE_LENGTH + 1);
        let error = parse_playlist("long", &long_line).expect_err("line should be rejected");

        assert!(matches!(
            error,
            PlaylistError::LineTooLong { line_number: 1, .. }
        ));
    }

    #[test]
    fn rejects_too_many_tracks() {
        let contents = std::iter::repeat_n("song", MAX_PREDEFINED_PLAYLIST_TRACKS + 1)
            .collect::<Vec<_>>()
            .join("\n");
        let error = parse_playlist("large", &contents).expect_err("playlist should be capped");

        assert!(matches!(error, PlaylistError::TooManyTracks { .. }));
    }

    #[tokio::test]
    async fn lists_text_files_case_insensitively() {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "little-bobby-tabots-playlists-{}-{unique_suffix}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("temporary playlist directory should be created");
        tokio::fs::write(root.join("Focus.TXT"), "First song")
            .await
            .expect("playlist should be written");
        tokio::fs::write(root.join("notes.md"), "not a playlist")
            .await
            .expect("non-playlist should be written");

        let library = PlaylistLibrary { root: root.clone() };
        let playlists = library.list().await.expect("playlists should list");

        tokio::fs::remove_dir_all(&root)
            .await
            .expect("temporary playlist directory should be removed");
        assert_eq!(playlists, vec!["Focus"]);
    }
}
