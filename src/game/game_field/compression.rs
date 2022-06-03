use crate::game::game_field::State::{self, B, E, W};
use unroll::unroll_for_loops;

const EMPTY_BIT_FLAG: u8 = 0b0000_0010;
const BLACK_BIT_FLAG: u8 = 0b0000_0001;
const WHITE_BIT_FLAG: u8 = 0u8;

#[inline]
#[unroll_for_loops]
pub const fn compress_field(field: &[[State; 15]; 15]) -> [(u8, u8, u8, u8); 15] {
    let mut compress_result = [(0u8, 0u8, 0u8, 0u8); 15];
    for n in 0..15 {
        compress_result[n] = compress_15_states(&field[n]);
    }
    compress_result
}

#[inline]
#[unroll_for_loops]
pub const fn decompress_field(dat: &[(u8, u8, u8, u8); 15]) -> [[State; 15]; 15] {
    let mut result = [[E; 15]; 15];
    for n in 0..15 {
        result[n] = decompress_15_states(dat[n])
    }
    result
}

#[inline]
const fn compress_15_states(row: &[State; 15]) -> (u8, u8, u8, u8) {
    (
        compress_four_states(&row[0], &row[1], &row[2], &row[3]),
        compress_four_states(&row[4], &row[5], &row[6], &row[7]),
        compress_four_states(&row[8], &row[9], &row[10], &row[11]),
        compress_four_states(&row[12], &row[13], &row[14], &E),
    )
}

#[inline]
const fn decompress_15_states((b0, b1, b2, b3): (u8, u8, u8, u8)) -> [State; 15] {
    let (p0, p1, p2, p3) = decompress_four_states(b0);
    let (p4, p5, p6, p7) = decompress_four_states(b1);
    let (p8, p9, p10, p11) = decompress_four_states(b2);
    let (p12, p13, p14, _) = decompress_four_states(b3);
    [
        p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14,
    ]
}

#[inline]
const fn compress_four_states(s1: &State, s2: &State, s3: &State, s4: &State) -> u8 {
    let b1 = state_to_byte(s1);
    let b2 = state_to_byte(s2);
    let b3 = state_to_byte(s3);
    let b4 = state_to_byte(s4);
    b1 ^ (b2 << 2) ^ (b3 << 4) ^ (b4 << 6)
}

#[inline(always)]
const fn decompress_four_states(b: u8) -> (State, State, State, State) {
    (
        decode_with_flag(b, 0),
        decode_with_flag(b, 2),
        decode_with_flag(b, 4),
        decode_with_flag(b, 6),
    )
}

#[inline(always)]
const fn state_to_byte(state: &State) -> u8 {
    match state {
        E => EMPTY_BIT_FLAG,
        B => BLACK_BIT_FLAG,
        W => WHITE_BIT_FLAG,
    }
}

// this function makes assumption about data integrity
#[inline(always)]
const fn decode_with_flag(byte: u8, shift_bit: u8) -> State {
    let is_empty = ((EMPTY_BIT_FLAG << shift_bit) & byte) != 0u8;
    let is_black = ((BLACK_BIT_FLAG << shift_bit) & byte) != 0u8;
    match (is_empty, is_black) {
        (false, true) => B,
        (false, false) => W,
        _ => E,
    }
}

#[cfg(test)]
mod compress {
    use super::*;
    use crate::game::game_field::utility::rotate;

    const FIELD: [[State; 15]; 15] = [
        [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
        [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
        [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
        [E, B, E, E, E, W, W, E, E, E, E, E, E, E, E],
        [E, B, B, E, E, W, W, E, E, E, E, E, E, E, E],
        [E, B, B, E, E, W, W, E, E, E, E, E, E, E, E],
        [E, B, B, E, E, W, W, E, E, E, E, E, E, E, E],
        [E, E, B, E, E, W, E, E, E, E, E, E, E, E, E],
        [E, E, B, B, W, W, E, E, E, E, E, E, E, E, E],
        [E, E, B, B, W, W, E, E, E, E, E, E, E, E, E],
        [E, E, B, B, W, E, E, E, E, E, E, E, E, E, E],
        [E, E, E, B, W, E, E, E, E, E, E, E, E, E, E],
        [E, E, E, B, W, E, E, E, E, E, E, E, E, E, E],
        [E, E, E, B, W, E, E, E, E, E, E, E, E, E, E],
        [E, E, E, B, W, E, E, E, E, E, E, E, E, E, E],
    ];

    #[test]
    fn test_decompress_compress() {
        let data = compress_field(&FIELD);
        let data_t = compress_field(&rotate(&FIELD));
        let field_decompressed = decompress_field(&data);
        let field_decompressed_t = decompress_field(&data_t);
        assert_eq!(field_decompressed, FIELD);
        assert_eq!(field_decompressed_t, rotate(&FIELD))
    }
}
