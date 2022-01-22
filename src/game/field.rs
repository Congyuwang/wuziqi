use crate::game::field::GameState::{BlackWins, Draw, Impossible, UnFinished, WhiteWins};
use crate::game::field::State::{B, E, W};
use crate::game::field_compression::compress_field;
use crate::game::field_utility::{diagonal_b_w_max, reduce_tuple_max, rotate, rows_b_w_max};
use anyhow::{Error, Result};

#[derive(Clone, PartialEq, Copy, Debug)]
#[repr(u8)]
pub enum Color {
    Black = 1,
    White = 2,
}

impl Color {
    pub fn switch(&self) -> Self {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }
}

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

impl From<Color> for State {
    #[inline(always)]
    fn from(c: Color) -> Self {
        match c {
            Color::Black => B,
            Color::White => W,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
#[repr(u8)]
pub(crate) enum GameState {
    BlackWins,
    WhiteWins,
    UnFinished,
    Draw,
    Impossible,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Field {
    inner: [[State; 15]; 15],
    field_state: GameState,
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

    /// play black and white
    pub fn play(&mut self, x: usize, y: usize, color: Color) -> Result<()> {
        match self.inner.get_mut(x) {
            None => unlikely_error(Err(Error::msg("field range exceeded"))),
            Some(row) => match row.get_mut(y) {
                None => unlikely_error(Err(Error::msg("field range exceeded"))),
                Some(s) => {
                    if *s != E {
                        unlikely_error(Err(Error::msg("already occupied")))
                    } else {
                        self.e_count -= 1;
                        *s = color.into();
                        Ok(self.update_field_state())
                    }
                }
            },
        }
    }

    /// clear a piece using State::E.
    pub fn clear(&mut self, x: usize, y: usize) -> Result<()> {
        match self.inner.get_mut(x) {
            None => unlikely_error(Err(Error::msg("field range exceeded"))),
            Some(row) => match row.get_mut(y) {
                None => unlikely_error(Err(Error::msg("field range exceeded"))),
                Some(s) => {
                    if *s != E {
                        self.e_count += 1;
                        *s = E;
                        self.update_field_state();
                        Ok(())
                    } else {
                        unlikely_error(Err(Error::msg("already empty")))
                    }
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
    pub fn get_field_state(&self) -> &GameState {
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

#[cold]
fn unlikely_error<T>(e: T) -> T {
    e
}

#[cfg(test)]
mod test_field {
    use super::Color::{Black, White};
    use super::*;

    #[test]
    fn test_field_1() {
        // 测试各种棋盘状态
        let mut f = Field::new();
        f.play(0, 0, Black).unwrap();
        f.play(14, 14, White).unwrap();
        f.play(1, 1, Black).unwrap();
        f.play(13, 13, White).unwrap();
        f.play(10, 10, Black).unwrap();
        f.play(5, 5, White).unwrap();
        f.play(10, 5, Black).unwrap();
        f.play(5, 10, White).unwrap();
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
            f.play(i, i, Black).unwrap();
        }
        assert_eq!(f.get_field_state(), &BlackWins);
    }

    #[test]
    fn test_play_w_win() {
        // test play method for WhiteWins
        let mut f = Field::new();
        for i in 0..5 {
            f.play(i, i, White).unwrap();
        }
        assert_eq!(f.get_field_state(), &WhiteWins);
    }

    #[test]
    fn test_play_unfinished() {
        let mut f = Field::new();
        for i in 0..4 {
            f.play(i, i, White).unwrap();
        }
        for i in (5..9).rev() {
            f.play(i, i, Black).unwrap();
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
                        f.play(i, j, White).unwrap();
                    } else {
                        f.play(i, j, Black).unwrap();
                    }
                } else {
                    if col_color_switcher {
                        f.play(i, j, Black).unwrap();
                    } else {
                        f.play(i, j, White).unwrap();
                    }
                }
                if i != 14 || j != 14 {
                    assert_eq!(f.get_field_state(), &UnFinished);
                }
            }
        }
        assert_eq!(f.get_field_state(), &Draw);
    }

    #[test]
    fn test_play_impossible() {
        let mut f = Field::new();
        for i in 0..6 {
            f.play(i, i, Black).unwrap();
        }
        assert_eq!(f.get_field_state(), &Impossible);
    }

    #[test]
    fn test_play_out_of_range() {
        let mut f = Field::new();
        if let Ok(_) = f.play(17, 21, Black) {
            panic!("error not thrown")
        }
    }
}
