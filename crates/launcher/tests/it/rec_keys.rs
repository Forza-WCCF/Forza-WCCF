//! Replays tests/rec/keys.jsonl (legacy/2010-11/tests/rec_e_keys.py: keyboard.py's key, controller, free play and eject
//! logic) against launcher::keys. Nothing here reads a real key, controller or window.

use forza::standins::io_board::{Inputs, Shared};
use launcher::keys::*;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

fn records() -> Vec<Value> {
    include_str!("rec/keys.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn of_kind(kind: &str) -> Vec<Value> {
    records().into_iter().filter(|r| r["kind"] == kind).collect()
}

fn set(v: &Value) -> BTreeSet<u32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u32)
        .collect()
}

fn idx(name: &str) -> usize {
    ACTIONS.iter().position(|a| a.name == name).unwrap()
}

fn bindings(b: &Value) -> Bindings {
    let (mut keys, mut pads) = (default_keys(), [0u32; 16]);
    for (n, v) in b["keys"].as_object().unwrap() {
        keys[idx(n)] = v.as_u64().unwrap() as u32;
    }
    for (n, v) in b["pads"].as_object().unwrap() {
        pads[idx(n)] = v.as_u64().unwrap() as u32;
    }
    Bindings::new(keys, pads)
}

fn bytes2(v: &Value) -> [u8; 2] {
    [v[0].as_u64().unwrap() as u8, v[1].as_u64().unwrap() as u8]
}

fn opt_f(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn axes(v: &Value) -> [Option<f64>; 6] {
    std::array::from_fn(|i| opt_f(&v[i]))
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

#[test]
fn tables() {
    let r = &of_kind("tables")[0];
    for (i, a) in r["actions"].as_array().unwrap().iter().enumerate() {
        let act = &ACTIONS[i];
        assert_eq!(a[0], act.name);
        assert_eq!(a[1].as_u64().unwrap() as u32, act.vk, "{}", act.name);
        let bits = act.bits.map(|(l, b, bit)| {
            let line = match l {
                Line::Sys => "sys",
                Line::P1 => "p1",
                Line::P2 => "p2",
            };
            (line.to_string(), b as u64, u64::from(bit))
        });
        let want = a[2].as_array().map(|b| {
            (
                b[0].as_str().unwrap().to_string(),
                b[1].as_u64().unwrap(),
                b[2].as_u64().unwrap(),
            )
        });
        assert_eq!(bits, want, "{}", act.name);
    }
    let allowed: Vec<i128> = r["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| i128::from(x.as_i64().unwrap()))
        .collect();
    for vk in -2..0x400i128 {
        assert_eq!(is_allowed(vk), allowed.contains(&vk), "key {vk:#x}");
    }
}

#[test]
fn labels_and_words() {
    for (vk, l) in of_kind("key_labels")[0]["labels"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(key_label(vk as u32), l.as_str().unwrap(), "key {vk:#x}");
    }
    let r = &of_kind("pad_codes")[0];
    for (code, c) in r["codes"].as_array().unwrap().iter().enumerate() {
        let code = code as u32;
        assert_eq!(pad_token(code).as_deref(), c[0].as_str(), "token {code:#x}");
        assert_eq!(pad_label(code), c[1].as_str().unwrap(), "label {code:#x}");
        if let Some(t) = c[0].as_str() {
            assert_eq!(pad_code(t), code, "round trip {code:#x}");
        }
    }
    for w in r["words"].as_array().unwrap() {
        assert_eq!(
            pad_code(w[0].as_str().unwrap_or("")),
            w[1].as_u64().unwrap() as u32,
            "word {:?}",
            w[0]
        );
    }
}

#[test]
fn keys_txt_as_recorded() {
    let loads = of_kind("load");
    assert!(loads.len() > 400);
    for (i, r) in loads.iter().enumerate() {
        let raw = unhex(r["file"].as_str().unwrap());
        let (keys, notes) = load_keys(&raw);
        let want: Vec<u32> = r["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(
            keys.to_vec(),
            want,
            "load {i}: keys of {:?}",
            String::from_utf8_lossy(&raw)
        );
        let want_notes: Vec<&str> = r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(
            notes,
            want_notes,
            "load {i}: notes of {:?}",
            String::from_utf8_lossy(&raw)
        );
        let (pads, pnotes) = load_pads(&raw);
        let mut want_pads = [0u32; 16];
        for p in r["pads"].as_array().unwrap() {
            want_pads[idx(p[0].as_str().unwrap())] = p[1].as_u64().unwrap() as u32;
        }
        assert_eq!(pads, want_pads, "load {i}: pads of {:?}", String::from_utf8_lossy(&raw));
        let want_pn: Vec<&str> = r["pad_notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(
            pnotes,
            want_pn,
            "load {i}: pad notes of {:?}",
            String::from_utf8_lossy(&raw)
        );
    }
}

#[test]
fn inputs_as_recorded() {
    let inputs = of_kind("input");
    assert!(inputs.len() >= 500);
    for (i, r) in inputs.iter().enumerate() {
        let b = bindings(&r["bind"]);
        let (down, prev) = (set(&r["down"]), set(&r["prev"]));
        let (card, coin) = (r["card"].as_bool().unwrap(), r["coin"].as_i64().unwrap());
        let held: Vec<&str> = (0..16)
            .filter(|k| b.held_actions(&down) >> k & 1 == 1)
            .map(|k| ACTIONS[k].name)
            .collect();
        let mut want: Vec<&str> = r["held"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        want.sort_by_key(|n| idx(n));
        assert_eq!(held, want, "input {i}: held");
        let rd = &r["render"];
        let s = b.render(&down, card, r["start"].as_bool());
        assert_eq!(
            (s.sys, s.p1, s.p2),
            (rd["sys"].as_u64().unwrap() as u8, bytes2(&rd["p1"]), bytes2(&rd["p2"])),
            "input {i}: render"
        );
        let (c2, coin2, ev) = b.step(&down, &prev, card, coin);
        let st = &r["step"];
        assert_eq!(c2, st[0].as_bool().unwrap(), "input {i}: step card");
        assert_eq!(coin2, st[1].as_i64().unwrap(), "input {i}: step coin");
        let want_ev: Vec<&str> = st[2].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        assert_eq!(ev, want_ev, "input {i}: events");
    }
}

#[test]
fn free_play_and_eject_as_recorded() {
    for r in of_kind("start_held") {
        let armed = r["armed"]
            .as_array()
            .map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
        assert_eq!(
            start_held(r["now"].as_f64().unwrap(), r["enter"].as_bool().unwrap(), armed),
            r["held"].as_bool().unwrap(),
            "{r}"
        );
    }
    let ejects = of_kind("eject");
    assert!(ejects.len() > 400);
    for r in ejects {
        let outs: Option<Vec<u8>> = r["outs"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_u64().unwrap() as u8).collect());
        let got = should_eject(
            r["now"].as_f64().unwrap(),
            r["card"].as_bool().unwrap(),
            opt_f(&r["since"]),
            outs.as_deref(),
            opt_f(&r["waiting"]),
        );
        assert_eq!(
            (got.0, got.1),
            (r["out"][0].as_bool().unwrap(), opt_f(&r["out"][1])),
            "{r}"
        );
    }
}

#[test]
fn controllers_as_recorded() {
    for r in of_kind("axis_values")[0]["cases"].as_array().unwrap() {
        let g = |i: usize| r[i].as_u64().unwrap() as u32;
        assert_eq!(pad_axis_value(g(0), g(1), g(2)), r[3].as_f64().unwrap(), "{r}");
    }
    for r in of_kind("hat_dirs")[0]["cases"].as_array().unwrap() {
        assert_eq!(pad_hat_dirs(r[0].as_u64().unwrap() as u32), set(&r[1]), "{r}");
    }
    for d in of_kind("pad_device") {
        let i = &d["init"];
        let mut dev = PadDevice::new(
            i[0].as_u64().unwrap() as u32,
            &axes(&i[1]),
            i[2].as_u64().unwrap() as u32,
        );
        for (n, rd) in d["reads"].as_array().unwrap().iter().enumerate() {
            let got = dev.read(
                rd[0].as_u64().unwrap() as u32,
                &axes(&rd[1]),
                rd[2].as_u64().unwrap() as u32,
            );
            assert_eq!(got, set(&rd[3]), "read {n} of {i}");
        }
    }
    for c in of_kind("pad_readings")[0]["cases"].as_array().unwrap() {
        let (caps, info) = (caps(&c[0]), info(&c[1]));
        let (b, ax, pov) = pad_reading(&caps, &info);
        assert_eq!(
            (b, ax, pov),
            (
                c[2][0].as_u64().unwrap() as u32,
                axes(&c[2][1]),
                c[2][2].as_u64().unwrap() as u32
            )
        );
    }
}

fn caps(c: &Value) -> Caps {
    Caps {
        num_buttons: c["buttons"].as_u64().unwrap() as u32,
        num_axes: c["axes"].as_u64().unwrap() as u32,
        flags: c["caps"].as_u64().unwrap() as u32,
        mid: 0x0079,
        pid: c["pid"].as_u64().unwrap() as u16,
        lim: std::array::from_fn(|i| {
            (
                c["lim"][i][0].as_u64().unwrap() as u32,
                c["lim"][i][1].as_u64().unwrap() as u32,
            )
        }),
    }
}

fn info(i: &Value) -> Info {
    Info {
        buttons: i["buttons"].as_u64().unwrap() as u32,
        pos: std::array::from_fn(|k| i["pos"][k].as_u64().unwrap() as u32),
        pov: i["pov"].as_u64().unwrap() as u32,
    }
}

#[derive(Default)]
struct Fake {
    devs: Mutex<HashMap<u32, (Caps, Info)>>,
    changed: Mutex<u32>,
}

struct FakeApi(Arc<Fake>);

impl PadApi for FakeApi {
    fn caps(&self, jid: u32) -> Option<Caps> {
        self.0.devs.lock().unwrap().get(&jid).map(|d| d.0.clone())
    }
    fn pos(&self, jid: u32) -> Option<Info> {
        self.0.devs.lock().unwrap().get(&jid).map(|d| d.1.clone())
    }
    fn config_changed(&self) {
        *self.0.changed.lock().unwrap() += 1;
    }
}

#[test]
fn pads_on_a_winmm_stand_in_as_recorded() {
    let (fake, said) = (Arc::new(Fake::default()), Arc::new(Mutex::new(Vec::<String>::new())));
    let log = said.clone();
    let pads = Pads::new(
        Box::new(FakeApi(fake.clone())),
        Arc::new(move |m| log.lock().unwrap().push(m.to_string())),
        Box::new(|c| format!("pad {:04X}", c.pid)),
    );
    for (n, step) in of_kind("pads")[0]["script"].as_array().unwrap().iter().enumerate() {
        let slot = step["slot"].as_u64().map(|s| s as u32);
        match step["op"].as_str().unwrap() {
            "set" => {
                fake.devs
                    .lock()
                    .unwrap()
                    .insert(slot.unwrap(), (caps(&step["caps"]), info(&step["info"])));
                continue;
            }
            "del" => {
                fake.devs.lock().unwrap().remove(&slot.unwrap());
                continue;
            }
            "rescan" => pads.rescan(),
            _ => assert_eq!(pads.poll(), set(&step["held"]), "step {n}: held"),
        }
        // `said` since the last query is what the recording keeps
        let want: Vec<&str> = step["said"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        let got: Vec<String> = std::mem::take(&mut *said.lock().unwrap());
        assert_eq!(got, want, "step {n}: said");
        assert_eq!(
            *fake.changed.lock().unwrap() as u64,
            step["changed"].as_u64().unwrap(),
            "step {n}: list read again"
        );
        let names: Vec<&str> = step["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(pads.names(), names, "step {n}: names");
    }
}

#[test]
fn driver_ticks_as_recorded() {
    let runs = of_kind("ticks");
    assert_eq!(runs.len(), 8);
    for run in runs {
        let shared = Shared::new();
        let mut card_start = Inputs::default();
        if run["init_card"].as_bool().unwrap() {
            card_start.p[1][0] = 0x20;
        }
        shared.set_inputs(card_start);
        let t0 = run["t0"].as_f64().unwrap();
        let mut d = Driver::new(bindings(&run["bind"]), shared.clone(), t0);
        for (n, t) in run["ticks"].as_array().unwrap().iter().enumerate() {
            shared.set_outputs(
                t["outs"]
                    .as_array()
                    .map(|a| a.iter().map(|x| x.as_u64().unwrap() as u8).collect()),
            );
            let events = d.tick(t["now"].as_f64().unwrap(), &set(&t["down"]));
            let want: Vec<&str> = t["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect();
            assert_eq!(events, want, "tick {n}: events");
            let (i, s) = (shared.inputs(), &t["state"]);
            assert_eq!(i.sys, s["sys"].as_u64().unwrap() as u8, "tick {n}: sys");
            assert_eq!(i.p, [bytes2(&s["p1"]), bytes2(&s["p2"])], "tick {n}: switches");
            assert_eq!(i.coin[0], s["coin"].as_i64().unwrap(), "tick {n}: coin");
            assert_eq!(shared.card_in(), t["card"].as_bool().unwrap(), "tick {n}: card");
        }
    }
}
