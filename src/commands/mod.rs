pub mod clear;
pub mod guild_state;
pub mod leave;
pub mod pause;
pub mod ping;
pub mod play;
pub mod playlist;
pub mod preplay;
pub mod queue;
pub mod resume;
pub mod skip;
pub mod youtube_playlist;

use std::sync::Arc;

use serenity::prelude::TypeMapKey;

use crate::Data;

/// TypeMap key for the shared bot data.
pub struct DataKey;

impl TypeMapKey for DataKey {
    type Value = Arc<Data>;
}
