use crate::game::field::State::{self, B, E, W};
use anyhow::Result;
use unroll::unroll_for_loops;

/// rotate a field
#[inline(always)]
#[unroll_for_loops]
pub(crate) const fn rotate(field: &[[State; 15]; 15]) -> [[State; 15]; 15] {
    let mut new_field = [[E; 15]; 15];
    for i in 0..15 {
        for j in 0..15 {
            new_field[i][j] = field[14 - j][i];
        }
    }
    new_field
}

/// compute max consecutive for each rows
#[inline]
pub(crate) fn rows_b_w_max(field: &[[State; 15]; 15]) -> (u8, u8) {
    reduce_tuple_max(field.iter().map(|x| max_consecutive_black_white(x.iter())))
}

/// compute max consecutive for diagonals
#[inline]
pub(crate) fn diagonal_b_w_max(field: &[[State; 15]; 15]) -> (u8, u8) {
    reduce_tuple_max(
        (4usize..15)
            .map(|x| {
                max_consecutive_black_white((0..=x).rev().zip(0..=x).map(|(i, j)| &field[i][j]))
            })
            .chain((1usize..=10).map(|x| {
                max_consecutive_black_white((x..=14).rev().zip(x..=14).map(|(i, j)| &field[i][j]))
            })),
    )
}

/// compute max for two streams of zipped integers
#[inline(always)]
pub(crate) fn reduce_tuple_max(iter: impl Iterator<Item = (u8, u8)>) -> (u8, u8) {
    iter.reduce(|(mx_b, mx_w), (b, w)| (mx_b.max(b), mx_w.max(w)))
        .unwrap()
}

/// compute max number of consecutive black and white pieces
#[inline(always)]
fn max_consecutive_black_white<'a>(iter: impl Iterator<Item = &'a State>) -> (u8, u8) {
    let (b_max, w_max, b_max_cur, w_max_cur) = iter.fold(
        (0u8, 0u8, 0u8, 0u8),
        |(b_max, w_max, b_max_cur, w_max_cur), s| match s {
            B => (b_max, w_max.max(w_max_cur), b_max_cur + 1, 0),
            W => (b_max.max(b_max_cur), w_max, 0, w_max_cur + 1),
            E => (b_max.max(b_max_cur), w_max.max(w_max_cur), 0, 0),
        },
    );
    (b_max.max(b_max_cur), w_max.max(w_max_cur))
}

#[cfg(test)]
mod test_field_utility {
    use super::*;

    const FIELD_2_3: [[State; 15]; 15] = [
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
    fn test_rotate() {
        let field_rotated = [
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, B, B, B, B, B, B, B],
            [E, E, E, E, B, B, B, B, B, B, B, E, E, E, E],
            [B, B, B, B, B, B, B, E, E, E, E, E, E, E, E],
            [W, W, W, W, W, W, W, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, W, W, W, W, W, W, W, E, E, E],
            [E, E, E, E, E, E, E, E, W, W, W, W, W, W, W],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
            [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
        ];

        assert_eq!(rotate(&FIELD_2_3), field_rotated);
    }

    #[test]
    fn test_max_consecutive_bw() {
        assert_eq!(
            max_consecutive_black_white([E, E, E, E, E, E, E, E, E].iter()),
            (0, 0)
        );
        assert_eq!(
            max_consecutive_black_white([W, E, B, W, E, B, W, E, B].iter()),
            (1, 1)
        );
        assert_eq!(
            max_consecutive_black_white([W, W, B, W, E, B, W, E, B].iter()),
            (1, 2)
        );
        assert_eq!(
            max_consecutive_black_white([W, E, B, W, E, B, B, B, B].iter()),
            (4, 1)
        );
        assert_eq!(
            max_consecutive_black_white([B, B, B, B, W, B, B, B, B].iter()),
            (4, 1)
        );
        assert_eq!(
            max_consecutive_black_white([B, B, B, B, E, B, B, B, B].iter()),
            (4, 0)
        );
        assert_eq!(
            max_consecutive_black_white([B, B, B, B, B, B, B, B, B].iter()),
            (9, 0)
        );
        assert_eq!(
            max_consecutive_black_white([B, E, E, B, B, B, W, W, W].iter()),
            (3, 3)
        );
    }

    #[test]
    fn test_rows_b_w_max() {
        assert_eq!(rows_b_w_max(&FIELD_2_3), (2, 2));
        assert_eq!(rows_b_w_max(&rotate(&FIELD_2_3)), (7, 7));
    }

    #[test]
    fn test_diag_b_w_max() {
        let field_5_3 = [
            [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
            [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
            [E, B, E, E, E, E, W, E, E, E, E, E, E, E, E],
            [E, B, E, E, E, W, W, E, E, E, E, E, E, E, E],
            [E, B, B, E, E, W, W, E, E, E, E, E, E, E, E],
            [E, B, B, E, E, W, W, B, E, E, E, E, E, E, E],
            [E, B, B, E, E, W, W, E, B, E, E, E, E, E, E],
            [E, E, B, E, E, W, E, E, E, B, E, E, E, E, E],
            [E, E, B, B, W, W, E, W, E, E, B, E, E, E, E],
            [E, E, B, B, W, W, E, E, W, E, E, B, E, E, E],
            [E, E, B, B, W, E, E, E, E, W, E, E, B, E, B],
            [E, E, E, B, W, E, E, E, E, E, W, E, E, B, E],
            [E, E, E, B, W, E, E, E, E, E, E, W, B, E, E],
            [E, E, E, B, W, E, E, E, E, E, E, B, W, E, E],
            [E, E, E, B, W, E, E, E, E, E, B, E, E, W, E],
        ];
        let field_6_4 = [
            [E, B, E, E, E, E, W, E, E, E, E, E, E, E, B],
            [E, B, E, E, E, E, W, E, E, E, E, E, E, B, E],
            [E, B, E, E, E, E, W, E, E, E, E, E, B, E, E],
            [E, B, E, E, E, W, W, E, E, E, E, B, E, E, E],
            [E, B, B, E, E, W, W, E, E, E, B, E, E, E, E],
            [E, B, B, E, E, W, W, E, E, B, E, E, E, E, E],
            [E, B, B, E, E, W, W, E, E, E, E, E, E, E, E],
            [E, E, B, E, E, W, E, W, E, E, E, E, E, E, E],
            [E, E, B, B, W, W, E, E, W, E, E, E, E, E, E],
            [E, E, B, B, W, W, E, E, E, W, E, E, E, E, E],
            [E, E, B, B, W, E, E, E, E, E, W, E, E, E, B],
            [E, E, E, B, W, E, E, E, E, E, E, W, E, B, E],
            [E, E, E, W, W, E, E, E, E, E, E, E, B, E, E],
            [E, E, W, B, W, E, E, E, E, E, E, B, E, E, E],
            [E, W, E, B, W, E, E, E, E, E, B, E, E, E, E],
        ];
        // left-lower to right-upper diag
        assert_eq!(diagonal_b_w_max(&FIELD_2_3), (2, 3));
        assert_eq!(diagonal_b_w_max(&field_5_3), (5, 3));
        assert_eq!(diagonal_b_w_max(&field_6_4), (6, 4));
        // left-upper to right-lower diag
        assert_eq!(diagonal_b_w_max(&rotate(&FIELD_2_3)), (3, 2));
        assert_eq!(diagonal_b_w_max(&rotate(&field_5_3)), (7, 7));
        assert_eq!(diagonal_b_w_max(&rotate(&field_6_4)), (3, 7));
    }
}
