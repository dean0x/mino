//! CLI command implementations

pub mod cache;
pub mod completions;
pub mod config;
pub mod exec;
pub mod init;
pub mod list;
pub mod logs;
pub mod run;
pub mod setup;
pub mod status;
pub mod stop;

pub use cache::execute as cache;
pub use completions::execute as completions;
pub use config::execute as config;
pub use exec::execute as exec;
pub use init::execute as init;
pub use list::execute as list;
pub use logs::execute as logs;
pub use run::execute as run;
pub use setup::execute as setup;
pub use status::execute as status;
pub use stop::execute as stop;
