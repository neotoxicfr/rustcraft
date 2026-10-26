//! Bench of the connection encryption without online services: a fake local server sends an
//! encryption request (no `join` required, like a proxy), checks the decrypted challenge,
//! switches to AES-128 CFB8, finishes the login and waits for the client's encrypted
//! acknowledgement. Then a second pass requires authentication: without a token, the client
//! must refuse cleanly.
//! `cargo run -p rc-client --example online_mock --release`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_client::ids::Ids;
use rc_client::{Config, Event};
use rc_net::{FrameDecoder, FrameEncoder};
use rc_proto::Reader;
use rc_proto::varint::{read_varint, write_varint};

fn read_packet(s: &mut TcpStream, dec: &mut FrameDecoder) -> Result<Vec<u8>, String> {
    let mut packet = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if dec.next_packet(&mut packet).map_err(|e| e.to_string())? {
            return Ok(packet);
        }
        let n = s.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("client gone".into());
        }
        dec.push(&buf[..n]);
    }
}

fn send(s: &mut TcpStream, enc: &mut FrameEncoder, id: i32, body: &[u8]) {
    let mut p = Vec::new();
    write_varint(&mut p, id);
    p.extend_from_slice(body);
    let mut wire = Vec::new();
    enc.encode(&p, &mut wire).expect("encoding");
    s.write_all(&wire).expect("write");
}

fn bytes(r: &mut Reader<'_>) -> Vec<u8> {
    let n = read_varint(r).expect("length") as usize;
    r.take(n).expect("bytes").to_vec()
}

fn server(listener: TcpListener, ids: Ids, authenticate: bool) -> Result<String, String> {
    use rsa::pkcs8::EncodePublicKey;
    use rsa::traits::Decryptor;
    let (mut s, _) = listener.accept().map_err(|e| e.to_string())?;
    let (mut dec, mut enc) = (FrameDecoder::new(), FrameEncoder::new());
    read_packet(&mut s, &mut dec)?; // handshake
    read_packet(&mut s, &mut dec)?; // hello
    let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
    let key = rsa::RsaPrivateKey::new(&mut rng, 1024).map_err(|e| e.to_string())?;
    let der = key.to_public_key().to_public_key_der().map_err(|e| e.to_string())?.as_bytes().to_vec();
    let challenge = [7u8, 1, 2, 9];
    let mut body = Vec::new();
    rc_proto::write_str(&mut body, "");
    write_varint(&mut body, der.len() as i32);
    body.extend_from_slice(&der);
    write_varint(&mut body, 4);
    body.extend_from_slice(&challenge);
    body.push(u8::from(authenticate));
    send(&mut s, &mut enc, ids.login_encryption_request, &body);
    let p = read_packet(&mut s, &mut dec)?;
    let mut r = Reader::new(&p);
    let id = read_varint(&mut r).map_err(|e| e.to_string())?;
    if id != ids.login_key {
        return Err(format!("packet {id} instead of the encryption response"));
    }
    let dk = rsa::pkcs1v15::DecryptingKey::new(key);
    let secret: [u8; 16] =
        dk.decrypt(&bytes(&mut r)).map_err(|e| e.to_string())?.try_into().map_err(|_| "secret ≠ 16")?;
    if dk.decrypt(&bytes(&mut r)).map_err(|e| e.to_string())? != challenge {
        return Err("incorrect challenge".into());
    }
    enc.enable_encryption(&secret);
    dec.enable_encryption(&secret);
    // Login finished (profile without properties), encrypted.
    let mut fin = Vec::new();
    fin.extend_from_slice(&0x1234u128.to_be_bytes());
    rc_proto::write_str(&mut fin, "AgentOnline");
    write_varint(&mut fin, 0);
    send(&mut s, &mut enc, ids.login_finished, &fin);
    let ack = read_packet(&mut s, &mut dec)?;
    let id = read_varint(&mut Reader::new(&ack)).map_err(|e| e.to_string())?;
    if id != ids.login_acknowledged {
        return Err(format!("unreadable encrypted acknowledgement (packet {id})"));
    }
    Ok("challenge verified, encrypted acknowledgement received and decrypted".into())
}

fn run(data: &Arc<rc_data::VersionData>, authenticate: bool) {
    let ids = Ids::resolve(data).expect("identifiers");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().expect("port").port();
    let srv = std::thread::spawn(move || server(listener, ids, authenticate));
    let cfg = Config { host: "127.0.0.1".into(), port, name: "AgentOnline".into(), ..Default::default() };
    let session = rc_client::connect(cfg, data.clone()).expect("connection");
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(ev) = session.events.recv_deadline(deadline) {
        match ev {
            Event::LoginFinished => println!("client: login finished after encryption"),
            Event::Kicked(c) => println!("client: refused: {c:?}"),
            Event::Disconnected(r) => {
                println!("client: end ({r})");
                break;
            }
            _ => {}
        }
    }
    session.link.disconnect();
    println!("server: {:?}", srv.join().expect("server thread"));
}

fn main() {
    let data = Arc::new(rc_data::load_or_prepare("26.3", |s| eprintln!("{s}")).expect("data"));
    // `serve <port> [auth]`: a single external client (the game), to see the refusal screen.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [cmd, port, rest @ ..] = args.as_slice()
        && cmd == "serve"
    {
        let listener = TcpListener::bind(("127.0.0.1", port.parse::<u16>().expect("port"))).expect("listen");
        let ids = Ids::resolve(&data).expect("identifiers");
        println!("server: {:?}", server(listener, ids, rest.first().is_some_and(|a| a == "auth")));
        return;
    }
    println!("== encryption without join required ==");
    run(&data, false);
    println!("== join required, no token ==");
    run(&data, true);
}
