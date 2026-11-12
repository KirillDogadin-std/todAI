//! What every todai frontend shares: the board event log (`store`), the text
//! agents receive (`inbox`), the HTTP API's wire types (`api`), and XDG paths
//! (`dirs`). No GUI and no network code lives here.

pub mod api;
pub mod boards;
pub mod dirs;
pub mod inbox;
pub mod store;
