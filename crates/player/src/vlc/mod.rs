//! VLC backend: VLC runs sppy's Lua interface module, which connects back over TCP
//! (the way Syncplay drives VLC).

mod player;
pub mod tracker;
pub mod wire;

pub use player::{MODULE, VlcConfig, VlcLauncher, VlcPlayer, install_module, user_lua_intf_dir};
