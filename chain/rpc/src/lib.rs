//! Node API: [`core::Core`] (chain + mempool + templates + subscribers), the JSON-RPC
//! dispatcher ([`dispatch`], docs/RPC.md) and the HTTP / WebSocket servers ([`server`]).

pub mod core;
pub mod dispatch;
mod reject;
mod social;
pub mod server;

pub use core::{Core, Outbound, Subscriber};
pub use dispatch::RpcError;
pub use server::{ApiRequest, Mailbox};
