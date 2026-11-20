pub mod auth;
pub mod dm;
pub mod gateway;
pub mod guild;
pub mod message;
pub mod role;
pub mod unimplemented;

pub use auth::AuthSvc;
pub use dm::DmSvc;
pub use gateway::GatewaySvc;
pub use guild::GuildSvc;
pub use message::MessageSvc;
pub use role::RoleSvc;
pub use unimplemented::{AdminSvc, MediaSvc};
