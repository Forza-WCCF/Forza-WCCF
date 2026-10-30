//! The tablet formation board's server (issue #94) on loopback: the token and network gate, board moves as the
//! panel makes them, a table change pushed to the tablet, the buttons in the key driver's tablet set.

use forza::cards::{self, Placed, TableFile};
use launcher::tablet::{Board, serve};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn sample() -> Vec<Placed> {
    let p = |slot, no, x, y| Placed { slot, no, x, y };
    vec![
        p(1, 101, 0.0, 0.9),
        p(2, 102, -0.6, 0.55),
        p(3, 103, -0.2, 0.6),
        p(4, 104, 0.2, 0.6),
        p(5, 105, 0.6, 0.55),
        p(6, 106, -0.25, -0.45),
        p(7, 201, cards::BENCH_X, cards::BENCH_Y[0]),
        p(8, 202, cards::BENCH_X, cards::BENCH_Y[1]),
    ]
}

struct Server {
    port: u16,
    keys: Arc<AtomicU16>,
    dir: tempfile::TempDir,
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// A board on 127.0.0.1 for peers on `net`, over the sample table.
async fn start(net: (Ipv4Addr, u8)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    TableFile::in_dir(dir.path()).write(&sample()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let keys = Arc::new(AtomicU16::new(0));
    let board = Board {
        dir: dir.path().to_owned(),
        catalogue: Arc::from(vec![]),
        pictures: dir.path().to_owned(),
        keys: keys.clone(),
        log: Arc::new(|_: &str| {}),
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(listener, TOKEN.into(), net, board, async move {
        let _ = rx.await;
    }));
    Server {
        port,
        keys,
        dir,
        _stop: tx,
    }
}

/// The status code of a plain GET (on a thread: the server runs on this test's runtime).
async fn status(port: u16, path: &str) -> u16 {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut r = String::new();
        let _ = s.read_to_string(&mut r);
        r.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
    })
    .await
    .unwrap()
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn tablet(port: u16) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws?t={TOKEN}"))
        .await
        .unwrap()
        .0
}

/// The next state message's cards as (slot, number).
async fn next_state(ws: &mut Ws) -> Vec<(u64, u64)> {
    use futures_util::StreamExt;
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a message within 5 s")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = m {
            let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap();
            if v["t"] == "state" {
                return v["cards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| (c["slot"].as_u64().unwrap(), c["no"].as_u64().unwrap()))
                    .collect();
            }
        }
    }
}

async fn say(ws: &mut Ws, text: &str) {
    use futures_util::SinkExt;
    ws.send(Message::Text(text.into())).await.unwrap();
}

/// (a) No token or a wrong one: 403, on every path; the right one: the page. A peer outside the network: 403 even with
/// the token.
#[tokio::test]
async fn the_token_and_the_network_gate_every_request() {
    let s = start((Ipv4Addr::LOCALHOST, 8)).await;
    for path in [
        "/",
        "/?t=",
        "/?t=0123456789abcdef0123456789abcdee",
        "/ws",
        "/c/101",
        "/?t=0123",
    ] {
        assert_eq!(status(s.port, path).await, 403, "{path}");
    }
    assert_eq!(status(s.port, &format!("/?t={TOKEN}")).await, 200);
    // pictures: digits only, never a path
    assert_eq!(status(s.port, &format!("/c/..%2Fx?t={TOKEN}")).await, 404);
    let wrong = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/ws?t=bad", s.port)).await;
    assert!(wrong.is_err(), "the socket without the token");

    let other = start((Ipv4Addr::new(192, 168, 1, 10), 24)).await;
    assert_eq!(status(other.port, &format!("/?t={TOKEN}")).await, 403);
}

/// (b) Tablet moves give the table file the panel's own board functions give for the same moves.
#[tokio::test]
async fn tablet_moves_write_the_panels_table() {
    let s = start((Ipv4Addr::LOCALHOST, 8)).await;
    let mut ws = tablet(s.port).await;
    next_state(&mut ws).await;
    for m in [
        r#"{"t":"swap","from":1,"to":11}"#, // a pitch card and a bench card trade places
        r#"{"t":"move","from":0,"x":0.1,"y":0.8}"#, // the keeper moves
        r#"{"t":"move","from":12,"x":0.5,"y":-0.5}"#, // a bench card onto open pitch
        r#"{"t":"off","slot":3}"#,          // a card taken off
        r#"{"t":"swap","from":2,"to":14}"#, // onto an empty bench box
    ] {
        say(&mut ws, m).await;
        next_state(&mut ws).await;
    }
    // the same moves as the panel makes them: board, move, table_after, a new board from the table
    let mut placed = sample();
    let step = |placed: &mut Vec<Placed>, f: &dyn Fn(&mut forza::board::Board)| {
        let mut b = cards::board(placed, &[]);
        f(&mut b);
        *placed = cards::table_after(placed, &b);
    };
    step(&mut placed, &|b| b.swap(1, 11));
    step(&mut placed, &|b| assert!(b.move_to(0, eframe::egui::pos2(0.55, 0.9))));
    step(&mut placed, &|b| assert!(b.move_to(12, eframe::egui::pos2(0.75, 0.25))));
    step(&mut placed, &|b| b.slots[3] = None);
    step(&mut placed, &|b| b.swap(2, 14));
    let want = tempfile::tempdir().unwrap();
    TableFile::in_dir(want.path()).write(&placed).unwrap();
    let read = |d: &std::path::Path| std::fs::read_to_string(d.join("fpr_table0.txt")).unwrap();
    assert_eq!(read(s.dir.path()), read(want.path()));
    assert_eq!(cards::parse_table(read(s.dir.path()).as_bytes()).len(), 7);
}

/// (c) A table written by someone else (the panel) reaches the tablet within 100 ms.
#[tokio::test]
async fn a_table_change_is_pushed_within_100_ms() {
    let s = start((Ipv4Addr::LOCALHOST, 8)).await;
    let mut ws = tablet(s.port).await;
    assert_eq!(next_state(&mut ws).await.len(), 8);
    let mut fewer = sample();
    fewer.pop();
    let t0 = Instant::now();
    TableFile::in_dir(s.dir.path()).write(&fewer).unwrap();
    let got = next_state(&mut ws).await;
    let took = t0.elapsed();
    assert_eq!(got.len(), 7);
    assert!(took < Duration::from_millis(100), "pushed after {took:?}");
}

/// (d) A button held on the tablet is in the key driver's tablet set (which counts with any window in front), held at
/// least 140 ms, and let go when the tablet goes; a second tablet takes over from the first.
#[tokio::test]
async fn buttons_reach_the_key_driver() {
    let s = start((Ipv4Addr::LOCALHOST, 8)).await;
    let shoot = 1u16 << forza::keys::action_index("SHOOT").unwrap();
    let mut ws = tablet(s.port).await;
    next_state(&mut ws).await;
    say(&mut ws, r#"{"t":"key","a":"SHOOT","down":true}"#).await;
    let wait = |want: u16| {
        let keys = s.keys.clone();
        async move {
            let t0 = Instant::now();
            while keys.load(Ordering::Relaxed) != want {
                assert!(t0.elapsed() < Duration::from_secs(2), "keys never {want:#x}");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            t0
        }
    };
    let down = wait(shoot).await;
    say(&mut ws, r#"{"t":"key","a":"SHOOT","down":false}"#).await;
    wait(0).await;
    assert!(
        down.elapsed() >= Duration::from_millis(140),
        "held {:?}",
        down.elapsed()
    );
    // not a button of the tablet's: nothing held
    say(&mut ws, r#"{"t":"key","a":"CARD","down":true}"#).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(s.keys.load(Ordering::Relaxed), 0);

    say(&mut ws, r#"{"t":"key","a":"SHOOT","down":true}"#).await;
    wait(shoot).await;
    let mut second = tablet(s.port).await;
    next_state(&mut second).await;
    assert_eq!(s.keys.load(Ordering::Relaxed), 0, "the first tablet's buttons let go");
    use futures_util::StreamExt;
    let bye = loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
            Some(Ok(Message::Text(t))) if t.as_str().contains("bye") => break t.as_str().to_owned(),
            Some(Ok(_)) => {}
            other => panic!("no bye: {other:?}"),
        }
    };
    assert!(bye.contains("other"), "{bye}");
}
