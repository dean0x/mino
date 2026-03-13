//! Session management module

pub mod manager;
pub mod state;

pub use manager::SessionManager;
pub use state::{validate_session_name, Session, SessionStatus};
