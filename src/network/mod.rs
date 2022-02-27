//! Network Infrastructure
//!
//! This module contains basic utility for establishing
//! stable network connection.
pub(crate) mod connection;
pub(crate) mod utility;
pub use connection::{handle_connection, Conn, ConnectionError, Received};
