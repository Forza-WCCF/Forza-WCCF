//! Replays tests/rec/debug.jsonl (legacy/2010-11/tests/rec_f_debug.py: debugger.py's recovery plan and note, return
//! detection, escaping, text check, image end, export lookup, address naming, SBWG_moSuppl) against launcher::debug.
//! Memory is a list of made-up regions [address, hex without its trailing zeros, length].

use std::collections::BTreeMap;
use std::fs;

use launcher::debug::maps::mosuppl_bytes;
use launcher::debug::mem::{
    Index, esc, export_va, image_end, looks_like_return, name_of, no_plan_note, plan_412860_recovery, text_at,
};
use serde_json::{Value, json};

fn hex(v: &Value) -> Vec<u8> {
    let s = v.as_str().unwrap();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

/// ReadProcessMemory over the regions: all n bytes from one region, or None (address 0 never reads).
fn model(v: &Value) -> impl Fn(u64, usize) -> Option<Vec<u8>> {
    let regions: Vec<(u64, Vec<u8>)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut b = hex(&r[1]);
            b.resize(usize::try_from(n(&r[2])).unwrap(), 0);
            (n(&r[0]), b)
        })
        .collect();
    move |addr, len| {
        if addr == 0 {
            return None;
        }
        regions.iter().find_map(|(start, data)| {
            (*start <= addr && addr + len as u64 <= start + data.len() as u64)
                .then(|| data[(addr - start) as usize..(addr - start) as usize + len].to_vec())
        })
    }
}

#[test]
fn debugger_parts_as_recorded() {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    let dir = tempfile::tempdir().unwrap();
    for line in include_str!("rec/debug.jsonl").lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        let f = rec["fn"].as_str().unwrap();
        let got = match f {
            "plan" => {
                let rd = model(&rec["mem"]);
                let esp = n(&rec["esp"]) as u32;
                plan_412860_recovery(esp, &rd, n(&rec["base"]) as u32, n(&rec["ret_off"]) as u32).map_or(
                    Value::Null,
                    |p| {
                        json!({"regs": {"Edi": p.edi, "Esi": p.esi, "Ebp": p.ebp, "Ebx": p.ebx, "Eip": p.eip,
                                        "Esp": p.esp, "Eax": p.eax},
                               "zero": [p.p_len, p.p_off], "retaddr": p.eip, "p_len": p.p_len, "p_off": p.p_off,
                               "at": p.at})
                    },
                )
            }
            "no_plan" => {
                let esp = n(&rec["esp"]);
                let stack = model(&rec["mem"])(esp, 0x120).unwrap_or_default();
                json!(no_plan_note(esp as u32, &stack, n(&rec["ret"]) as u32))
            }
            "looks_like_return" => json!(looks_like_return(&hex(&rec["bytes"]))),
            "esc" => json!(esc(&hex(&rec["bytes"]))),
            "text_at" => {
                let rd = model(&rec["mem"]);
                text_at(&rd, n(&rec["va"])).map_or(Value::Null, |s| json!(to_hex(&s)))
            }
            "image_end" => json!(image_end(&model(&rec["mem"]), n(&rec["base"]))),
            "export_va" => {
                let rd = model(&rec["mem"]);
                export_va(&rd, n(&rec["base"]), rec["want"].as_str().unwrap()).map_or(Value::Null, |v| json!(v))
            }
            "name_of" => {
                let i = &rec["index"];
                let index = Index {
                    starts: i[0].as_array().unwrap().iter().map(n).collect(),
                    names: i[1]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_owned())
                        .collect(),
                    sizes: i[2].as_array().unwrap().iter().map(n).collect(),
                };
                let dlls: Vec<(u64, u64, String)> = rec["dlls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| (n(&d[0]), n(&d[1]), d[2].as_str().unwrap().to_owned()))
                    .collect();
                let (base, end) = (n(&rec["base"]), n(&rec["end"]));
                json!(name_of(
                    n(&rec["va"]),
                    &index,
                    base,
                    end,
                    rec["name"].as_str().unwrap(),
                    &dlls
                ))
            }
            "mosuppl" => {
                let md = dir.path().join("AI/motion_data");
                fs::create_dir_all(&md).unwrap();
                for (name, data) in rec["files"].as_object().unwrap() {
                    fs::write(md.join(name), hex(data)).unwrap();
                }
                json!(to_hex(&mosuppl_bytes(dir.path()).unwrap()))
            }
            other => panic!("unknown record {other}"),
        };
        assert_eq!(got, rec["out"], "{f}: {line}");
        *count.entry(f.to_owned()).or_default() += 1;
    }
    assert_eq!(count.values().sum::<usize>(), 835, "{count:?}");
}
