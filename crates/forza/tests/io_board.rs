//! Replays tests/rec/io_board.jsonl (legacy/2010-11/tests/rec_e_io_board.py: io_board.py on a scratch pipe) against
//! `forza::standins::io_board` and compares each reply and the outputs the board keeps byte for byte.
#![cfg(windows)]

use forza::standins::io_board::{self, Inputs, Shared};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn pair(v: &Value) -> [u8; 2] {
    [v[0].as_u64().unwrap() as u8, v[1].as_u64().unwrap() as u8]
}

#[test]
fn board_as_recorded() {
    let pipe = format!(r"\\.\pipe\forza_test_io_board_{}", std::process::id());
    let shared = Shared::new();
    let svc = io_board::spawn(&pipe, shared.clone(), Arc::new(|_| {})).unwrap();
    let mut f = (0..100)
        .find_map(|_| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&pipe)
                .ok()
                .or_else(|| {
                    sleep(Duration::from_millis(50));
                    None
                })
        })
        .expect("pipe never opened");
    let (mut replies, mut steps) = (0, 0);
    for (i, line) in include_str!("rec/io_board.jsonl").lines().enumerate() {
        let r: Value = serde_json::from_str(line).unwrap();
        if let Some(s) = r.get("state") {
            shared.set_inputs(Inputs {
                sys: s["sys"].as_u64().unwrap() as u8,
                p: [pair(&s["p1"]), pair(&s["p2"])],
                coin: [
                    s["coin"][0].as_i64().unwrap(),
                    s["coin"][1].as_i64().unwrap(),
                ],
            });
            continue;
        }
        steps += 1;
        let req = unhex(r["send"].as_str().unwrap());
        let split = r["split"].as_u64().unwrap() as usize;
        if split > 0 {
            f.write_all(&req[..split]).unwrap();
            sleep(Duration::from_millis(100));
            f.write_all(&req[split..]).unwrap();
        } else {
            f.write_all(&req).unwrap();
        }
        sleep(Duration::from_millis(150));
        let mut resp = Vec::new();
        loop {
            let mut avail = 0u32;
            // SAFETY: the handle is the open pipe client; only the bytes-available out-pointer is given.
            let ok = unsafe {
                PeekNamedPipe(
                    f.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut avail,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || avail == 0 {
                break;
            }
            let mut buf = vec![0u8; avail as usize];
            f.read_exact(&mut buf).unwrap();
            resp.extend(buf);
            sleep(Duration::from_millis(50));
        }
        assert_eq!(
            hex(&resp),
            r["reply"].as_str().unwrap(),
            "record {i}: reply"
        );
        replies += usize::from(!resp.is_empty());
        let outs = shared.outputs().map(|o| hex(&o));
        assert_eq!(
            outs.as_deref(),
            r["outputs"].as_str(),
            "record {i}: outputs"
        );
    }
    assert!(
        steps > 50 && replies > 40,
        "{steps} steps, {replies} replies"
    );
    drop(f);
    drop(svc); // stops cleanly
}

#[test]
fn card_in_follows_the_second_players_sensor() {
    let s = Shared::new();
    assert!(!s.card_in());
    s.set_switches(0, [0, 0], [io_board::CARD_BIT, 0x80]);
    assert!(s.card_in());
    s.set_switches(0, [0x80, 0], [0, 0x80]);
    assert!(!s.card_in());
}
