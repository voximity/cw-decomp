//! Starts the server binary and performs the client handshake against it: the Join and Seed
//! packets, then the first frame (UpdateFinished, CurrentTime, ServerUpdate). Needs
//! `CW_GAME_DIR` for the model table; skipped otherwise.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn read_exact(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).expect("read");
    b
}

fn u32_of(s: &mut TcpStream) -> u32 {
    u32::from_le_bytes(read_exact(s, 4).try_into().unwrap())
}

#[test]
fn a_client_joins_and_receives_frames() {
    let Some(game) = std::env::var_os("CW_GAME_DIR") else {
        eprintln!("CW_GAME_DIR not set; skipping");
        return;
    };
    let port = 12390 + (std::process::id() % 100) as u16;
    let mut child = Command::new(env!("CARGO_BIN_EXE_cw-server"))
        .args(["--seed", "26879", "--port", &port.to_string(), "--no-save", "--game-dir"])
        .arg(&game)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    let result = std::panic::catch_unwind(|| {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(e) if Instant::now() < deadline => {
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => panic!("connect: {e}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        // A wrong version gets the bare u32 3.
        stream.write_all(&[0x11, 0, 0, 0, 2, 0, 0, 0]).unwrap();
        assert_eq!(u32_of(&mut stream), 3);
        drop(stream);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(&[0x11, 0, 0, 0, 3, 0, 0, 0]).unwrap();
        assert_eq!(u32_of(&mut stream), 0x10, "join id");
        assert_eq!(u32_of(&mut stream), 0);
        let id = i64::from_le_bytes(read_exact(&mut stream, 8).try_into().unwrap());
        assert_eq!(id, 1);
        let entity = read_exact(&mut stream, 0x1168);
        assert_eq!(&entity[0x54..0x58], &0x2cu32.to_le_bytes(), "player type");
        assert_eq!(&entity[0x15c..0x160], &100.0f32.to_le_bytes(), "max HP at level 1");
        assert_eq!(u32_of(&mut stream), 0x0f);
        assert_eq!(u32_of(&mut stream), 26879);
        // First frame: no other entities, so UpdateFinished, CurrentTime, ServerUpdate.
        assert_eq!(u32_of(&mut stream), 2);
        assert_eq!(u32_of(&mut stream), 5);
        let _day = u32_of(&mut stream);
        let _time = u32_of(&mut stream);
        assert_eq!(u32_of(&mut stream), 4);
        let len = u32_of(&mut stream) as usize;
        let body = cw_net::compress::decompress(&read_exact(&mut stream, len)).unwrap();
        assert_eq!(body, vec![0u8; 52], "an empty ServerUpdate has thirteen zero counts");
    });
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
