//! Language-server process lifecycle, protocol transport, and request/notification handling.

mod client;
mod protocol;

pub use client::{Client, Event, Request, ServerConfig};
pub use protocol::{
    Capabilities, Encoding, Position, SyncMode, byte_range, completion_position, completions, diagnostics, hover, position, read_frame, uri,
    write_frame,
};
