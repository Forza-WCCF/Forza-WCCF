//! Replays tests/rec/conf.jsonl (legacy/2010-11/tests/rec_a_conf.py: play.py's set_control_ip, set_satellite_no,
//! set_conf_value).  Bytes are stored as Latin-1 text.

use std::fs;
use std::io::ErrorKind;

use launcher::conf::{set_conf_value, set_control_ip, set_satellite_no};
use serde_json::Value;

fn bytes(v: &Value) -> Vec<u8> {
    v.as_str()
        .unwrap()
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap())
        .collect()
}

#[test]
fn conf_edits_as_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("client_user_option.conf");
    let mut n = 0;
    for line in include_str!("../rec/conf.jsonl").lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        let _ = fs::remove_file(&conf);
        if !rec["before"].is_null() {
            fs::write(&conf, bytes(&rec["before"])).unwrap();
        }
        let a = &rec["args"];
        let got = match rec["fn"].as_str().unwrap() {
            "set_control_ip" => set_control_ip(&conf, a[0].as_str().unwrap()),
            "set_satellite_no" => set_satellite_no(&conf, u32::try_from(a[0].as_u64().unwrap()).unwrap()),
            "set_conf_value" => set_conf_value(&conf, a[0].as_str().unwrap(), a[1].as_str().unwrap()),
            f => panic!("{f}"),
        };
        match (got, rec["error"].as_str()) {
            (Ok(changed), None) => assert_eq!(Some(changed), rec["changed"].as_bool(), "{line}"),
            (Err(e), Some("os")) => assert_eq!(e.kind(), ErrorKind::NotFound, "{line}"),
            (Err(e), Some(msg)) => {
                assert_eq!(
                    e.to_string(),
                    msg.replace("{conf}", &conf.display().to_string()),
                    "{line}"
                );
            }
            (got, _) => panic!("{got:?} for {line}"),
        }
        if !rec["before"].is_null() {
            assert_eq!(fs::read(&conf).unwrap(), bytes(&rec["after"]), "{line}");
        }
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(Value::from(left), rec["left"], "{line}");
        n += 1;
    }
    assert_eq!(n, 99);
}
