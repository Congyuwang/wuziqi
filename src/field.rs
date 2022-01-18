use crate::field::FieldState::{BlackWins, Draw, Impossible, UnFinished, WhiteWins};
use crate::field::State::{B, E, W};
use crate::field_utility::{diagonal_b_w_max, reduce_tuple_max, rotate, rows_b_w_max};
use anyhow::{Error, Result};

// ban 0 for protocol message EOF
#[derive(Clone, PartialEq, Copy, Debug)]
#[repr(u8)]
pub enum State {
    // black
    B = 1,
    // white
    W = 2,
    // empty
    E = 3,
}

#[derive(Clone, PartialEq, Debug)]
#[repr(u8)]
pub enum FieldState {
    BlackWins,
    WhiteWins,
    UnFinished,
    Draw,
    Impossible,
}

#[derive(Debug, PartialEq)]
pub struct Field {
    inner: [[State; 15]; 15],
    field_state: FieldState,
    e_count: u8,
}

impl Field {
    #[inline(always)]
    pub fn new() -> Self {
        Field {
            inner: [[E; 15]; 15],
            field_state: UnFinished,
            e_count: 225,
        }
    }

    /// put a piece on the field, or clear a piece using State::E.
    pub fn play(&mut self, x: usize, y: usize, state: State) -> Result<()> {
        match self.inner.get_mut(x) {
            None => Err(Error::msg("field range exceeded")),
            Some(row) => match row.get_mut(y) {
                None => Err(Error::msg("field range exceeded")),
                Some(s) => {
                    if let E = s {
                        self.e_count -= 1;
                    }
                    if let E = state {
                        self.e_count += 1;
                    }
                    *s = state;
                    Ok(self.update_field_state())
                }
            },
        }
    }

    /// read the internal representation of field
    pub fn get_field(&self) -> &[[State; 15]; 15] {
        &self.inner
    }

    /// read the field state
    ///
    /// this method is inert, it reads from cached field state
    pub fn get_field_state(&self) -> &FieldState {
        &self.field_state
    }

    /// called in play()
    fn update_field_state(&mut self) {
        let rotated = rotate(&self.inner);
        let rows_max = rows_b_w_max(&self.inner);
        let cols_max = rows_b_w_max(&rotated);
        let diag_max = diagonal_b_w_max(&self.inner);
        let diag_max_t = diagonal_b_w_max(&rotated);
        let (black_max, white_max) =
            reduce_tuple_max([rows_max, cols_max, diag_max, diag_max_t].into_iter());
        self.field_state = match (black_max, white_max, self.e_count) {
            (0..=4, 0..=4, 0) => Draw,
            (0..=4, 0..=4, 1..=225) => UnFinished,
            (5, 0..=4, _) => BlackWins,
            (0..=4, 5, _) => WhiteWins,
            _ => Impossible,
        }
    }
}

#[cfg(test)]
mod test_field {
    use super::*;
    #[test]
    fn test_empty_field() {
        let mut field = Field::new();
        field.play(7, 8, B).unwrap();
        // and play some
        // check state
        assert_eq!(*field.get_field_state(), UnFinished);
    }

    #[test]
    fn test_field_1() {
        // 测试各种棋盘状态
        let mut f = Field::new();
        f.play(0, 0, B).unwrap();
        f.play(14, 14, W).unwrap();
        f.play(1, 1, B).unwrap();
        f.play(13, 13, W).unwrap();
        f.play(10, 10, B).unwrap();
        f.play(5, 5, W).unwrap();
        f.play(10, 5, B).unwrap();
        f.play(5, 10, W).unwrap();
        assert_eq!(
            f.get_field(),
            &[
                [B, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, B, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, W, E, E, E, E, W, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, B, E, E, E, E, B, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, W, E],
                [E, E, E, E, E, E, E, E, E, E, E, E, E, E, W],
            ]
        )
    }

    #[test]
    fn test_play_b_win() {
        // test play method for BlackWins
        let mut f = Field::new();
        for i in 0..5 {
            f.play(i, i, B).unwrap();
        }
        assert_eq!(f.get_field_state(), &BlackWins);
    }

    #[test]
    fn test_play_w_win() {
        // test play method for WhiteWins
        let mut f = Field::new();
        for i in 0..5 {
            f.play(i, i, W).unwrap();
        }
        assert_eq!(f.get_field_state(), &WhiteWins);
    }

    #[test]
    fn test_play_unfinished() {
        let mut f = Field::new();
        for i in 0..4 {
            f.play(i, i, W).unwrap();
        }
        for i in (5..9).rev() {
            f.play(i, i, B).unwrap();
        }
        assert_eq!(f.get_field_state(), &UnFinished);
    }

    #[test]
    fn test_play_draw() {
        let mut f = Field::new();
        for i in 0..15 {
            let row_order_switcher = (i / 3) % 2 == 0;
            for j in 0..15 {
                let col_color_switcher = ((j / 3) + i) % 2 == 0;
                if row_order_switcher {
                    if col_color_switcher {
                        f.play(i, j, W);
                    } else {
                        f.play(i, j, B);
                    }
                } else {
                    if col_color_switcher {
                        f.play(i, j, B);
                    } else {
                        f.play(i, j, W);
                    }
                }
            }
        }
        assert_eq!(f.get_field_state(), &Draw);
    }

    #[test]
    fn test_play_impossible() {
        let mut f = Field::new();
        for i in 0..6 {
            f.play(i, i, B).unwrap();
        }
        assert_eq!(f.get_field_state(), &Impossible);
    }

    #[test]
    fn test_play_out_of_range() {
        let mut f = Field::new();
        if let Ok(_) = f.play(17, 21, B) {
            panic!("error not thrown")
        }
    }
}
