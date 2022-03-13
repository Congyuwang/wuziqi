use anyhow::{Error, Result};
use rand::prelude::*;
use std::hash::Hash;
use unicode_segmentation::UnicodeSegmentation;
use serde::{Serialize, Deserialize};

const TOKEN_LENGTH: usize = 10;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct RoomToken(pub(crate) [u8; TOKEN_LENGTH]);

impl RoomToken {
    pub(crate) fn random<R: Rng>(rng: &mut R) -> Self {
        let rand_range = rand::distributions::Uniform::new(0u8, 116u8);
        let mut inner = [0u8; TOKEN_LENGTH];
        for (idx, rd) in rand_range.sample_iter(rng).take(TOKEN_LENGTH).enumerate() {
            inner[idx] = rd;
        }
        RoomToken(inner)
    }

    pub fn from_code(code: &str) -> Result<Self> {
        decode_token(code)
    }

    pub fn as_code(&self) -> String {
        encode_token(self)
    }
}

macro_rules! encode_decode_char {
    ([$($chars:literal),+] <=> [$($codes:literal),+]) => {
        fn char_to_code(c: &str) -> Result<u8> {
            match c {
                $($chars => Ok($codes)),*,
                _ => Err(Error::msg("invalid token char")),
            }
        }

        fn code_to_char(c: &u8) -> Result<&str> {
            match c {
                $($codes => Ok($chars)),*,
                _ => Err(Error::msg("invalid token code")),
            }
        }
    }
}

encode_decode_char!([
    "观", "自", "在", "菩", "萨", "行", "深", "般", "若", "波", "罗", "蜜",
    "多", "时", "照", "见", "五", "蕴", "皆", "空", "度", "一", "切", "苦",
    "厄", "舍", "利", "子", "色", "不", "异", "即", "是", "受", "想", "识",
    "亦", "复", "如", "诸", "法", "相", "生", "灭", "垢", "净", "增", "减",
    "故", "中", "无", "眼", "耳", "鼻", "舌", "身", "意", "声", "香", "味",
    "触", "界", "乃", "至", "明", "尽", "老", "死", "集", "道", "智", "得",
    "以", "所", "提", "埵", "依", "心", "罣", "碍", "有", "恐", "怖", "远",
    "离", "颠", "倒", "梦", "究", "竟", "涅", "磐", "三", "世", "佛", "阿",
    "耨", "藐", "知", "大", "神", "咒", "上", "等", "能", "除", "真", "实",
    "虚", "说", "曰", "揭", "谛", "僧", "婆", "诃"
] <=> [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
    56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73,
    74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91,
    92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107,
    108, 109, 110, 111, 112, 113, 114, 115
]);

#[inline]
fn decode_token(code: &str) -> Result<RoomToken> {
    let mut out = [0u8; TOKEN_LENGTH];
    let mut count = 0usize;
    for (i, ch) in code.graphemes(true).enumerate() {
        match out.get_mut(i) {
            None => Err(Error::msg("incorrect token length"))?,
            Some(t) => {
                *t = char_to_code(ch)?;
            }
        }
        count += 1;
    }
    if count < TOKEN_LENGTH {
        Err(Error::msg("short token length"))
    } else {
        Ok(RoomToken(out))
    }
}

#[inline]
fn encode_token(token: &RoomToken) -> String {
    let mut out = String::with_capacity(TOKEN_LENGTH);
    for t in &token.0 {
        out.push_str(code_to_char(t).unwrap())
    }
    out
}

#[cfg(test)]
mod test_room_manager {
    use super::*;

    #[test]
    fn test_decode() {
        let token = "观自在菩萨行深般若波";
        assert_eq!(
            RoomToken::from_code(token).unwrap().as_code(),
            token.to_string()
        );
    }

    #[test]
    fn test_error_length() {
        let token = "观自在菩萨行深般若波罗蜜多";
        assert!(matches!(RoomToken::from_code(token), Err(_)))
    }

    #[test]
    fn test_random_gen() {
        let mut rng = thread_rng();
        for _ in 0..1000 {
            let token = RoomToken::random(&mut rng);
            assert_eq!(RoomToken::from_code(&token.as_code()).unwrap(), token)
        }
    }
}
