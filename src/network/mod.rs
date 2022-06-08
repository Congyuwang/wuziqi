//! Network Infrastructure
//!
//! This module contains basic utility for establishing
//! stable network connection.
pub(crate) mod connection;
pub(crate) mod utility;

pub use connection::{Conn, ConnectionError, Received};
