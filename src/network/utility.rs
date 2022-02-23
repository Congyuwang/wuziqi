use async_std::io::BufReader;
use async_std::net::TcpStream;
use futures::AsyncReadExt;

pub async fn read_n_bytes(reader: &mut BufReader<TcpStream>, n: u32) -> Option<Vec<u8>> {
    let n = n as usize;
    let mut pay_load = Vec::with_capacity(n);
    for _ in 0..n {
        pay_load.push(read_one_byte(reader).await?);
    }
    Some(pay_load)
}

pub async fn read_be_u32(reader: &mut BufReader<TcpStream>) -> Option<u32> {
    let mut bytes = [0u8; 4];
    if reader.read_exact(&mut bytes).await.is_err() {
        None
    } else {
        Some(u32::from_be_bytes(bytes))
    }
}

pub async fn read_one_byte(reader: &mut BufReader<TcpStream>) -> Option<u8> {
    let mut packet_type = [0u8; 1];
    if reader.read_exact(&mut packet_type).await.is_err() {
        None
    } else {
        Some(packet_type[0])
    }
}
