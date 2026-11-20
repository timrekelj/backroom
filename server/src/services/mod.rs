pub mod auth;
pub mod dm;
pub mod gateway;
pub mod guild;
pub mod message;

pub use auth::AuthSvc;
pub use dm::DmSvc;
pub use gateway::GatewaySvc;
pub use guild::GuildSvc;
pub use message::MessageSvc;
