//! `sphere-host`: a tiny, stable process that owns every child process.
//!
//! Agents run on pipes and terminals on PTYs; each child gets its own process
//! group, and everything it prints is appended to a file, so whoever restarts
//! (sphered, an update, the app) can read on from the offset it saved. The
//! host speaks newline-delimited JSON on a private Unix socket.

pub mod client;
pub mod proto;
pub mod server;

pub use client::{ensure_running, DecodedRead, Follow, FollowItem, HostClient};
pub use proto::*;
pub use server::{serve, ServeOptions};
