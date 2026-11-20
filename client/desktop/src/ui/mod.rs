//! One module per screen or region. Each builds elements from `&App` and the design kit and
//! wires handlers that mutate `AppState` through `App::act` (which also hands the queued commands
//! to the backend). Nothing here owns state.

pub mod auth;
pub mod composer;
pub mod dialogs;
pub mod members;
pub mod rail;
pub mod settings;
pub mod shell;
pub mod sidebar;
pub mod toasts;
pub mod transcript;
