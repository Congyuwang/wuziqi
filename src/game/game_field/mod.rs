mod api;
mod compression;
mod field;
mod utility;
use bincode::{Decode, Encode};

/// Represents player action (black or white)
#[derive(Clone, PartialEq, Copy, Debug, Encode, Decode)]
#[repr(u8)]
pub enum Color {
    Black = 1,
    White = 2,
}

/// represents field State: Black, White, Empty
#[derive(Clone, PartialEq, Copy, Debug, Encode, Decode)]
#[repr(u8)]
pub enum State {
    // black
    B = 1,
    // white
    W = 2,
    // empty
    E = 3,
}

pub(crate) use api::{new_field, GameCommand, GameResponse};
pub use compression::{compress_field, decompress_field};
