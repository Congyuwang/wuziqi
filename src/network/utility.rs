use async_std::io::BufReader;
use futures::{AsyncRead, AsyncReadExt};

pub async fn read_n_bytes<S>(reader: &mut BufReader<S>, n: u32) -> Option<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let n = n as usize;
    let mut pay_load = Vec::with_capacity(n);
    for _ in 0..n {
        pay_load.push(read_one_byte(reader).await?);
    }
    Some(pay_load)
}

pub async fn read_be_u32<S>(reader: &mut BufReader<S>) -> Option<u32>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = [0u8; 4];
    if reader.read_exact(&mut bytes).await.is_err() {
        None
    } else {
        Some(u32::from_be_bytes(bytes))
    }
}

pub async fn read_one_byte<S>(reader: &mut BufReader<S>) -> Option<u8>
where
    S: AsyncRead + Unpin,
{
    let mut packet_type = [0u8; 1];
    if reader.read_exact(&mut packet_type).await.is_err() {
        None
    } else {
        Some(packet_type[0])
    }
}
