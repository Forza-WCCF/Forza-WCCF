//! The 2017-18 network guard, checked before any of its game programs starts.  The game calls four IP-helper functions
//! that drop this PC's DHCP lease and change its routes; setup (`scripts\setup.py`) makes copies of client_Release.exe and
//! control_Release.exe that import them from the kit's `wccfnet.dll` (four stubs) instead of IPHLPAPI.DLL, and records
//! the hashes of what it made in `<runtime>\setup-hashes.json`.  A run starts only when every recorded file is still
//! the one setup made, and each guarded program's import table names wccfnet.dll and not IPHLPAPI.DLL.
//! (`netguard.dll`, injected into control and both clients by the run, is the second line.)

use std::fs;
use std::path::{Path, PathBuf};

use object::LittleEndian as LE;
use object::pe::ImageNtHeaders32;
use object::read::pe::{Import, PeFile32};
use sha2::{Digest, Sha256};

/// Setup's record, in the runtime folder (the one above the game folder).
pub const HASHES: &str = "setup-hashes.json";
/// The guard DLL the guarded programs import.
pub const WCCFNET: &str = "wccfnet.dll";
/// The stand-in the game loads by name for its pings (they return at once), in the game folder and seat 1.
pub const ICMP: &str = "ICMP.DLL";
/// The calls wccfnet.dll stubs.
pub const BLOCKED: [&str; 4] = [
    "IpReleaseAddress",
    "IpRenewAddress",
    "CreateIpForwardEntry",
    "SetIpForwardEntry",
];

/// The files the guard checks, each with where setup-hashes.json keeps its hash: the game's two guarded programs, seat
/// 1's own guarded client, the two guard DLLs and the ICMP.DLL stand-in (seat 1's wccfnet.dll and ICMP.DLL are links
/// to the game's).
pub fn files(game: &Path, seat: &Path, client: &str, control: &str) -> Vec<(PathBuf, Vec<String>)> {
    let key = |k: &[&str]| k.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    vec![
        (game.join(client), key(&[client, "guarded"])),
        (game.join(control), key(&[control, "guarded"])),
        (seat.join(client), key(&[&format!("seat1/{client}")])),
        (game.join(WCCFNET), key(&[WCCFNET])),
        (seat.join(WCCFNET), key(&[WCCFNET])),
        (game.join("netguard.dll"), key(&["netguard.dll"])),
        (game.join(ICMP), key(&[ICMP])),
        (seat.join(ICMP), key(&[ICMP])),
    ]
}

/// The whole check for a game folder and its seat 1 (`client`, `control`: the programs' names - the game's, or a test's
/// stand-ins): Ok, or what is wrong.
pub fn check(game: &Path, seat: &Path, client: &str, control: &str) -> Result<(), String> {
    let runtime = game.parent().unwrap_or(game);
    hashes(&runtime.join(HASHES), &files(game, seat, client, control))?;
    for exe in [game.join(client), game.join(control), seat.join(client)] {
        let data = fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        imports(&data).map_err(|why| format!("{}: {why}", exe.display()))?;
    }
    Ok(())
}

/// Every file's SHA-256 is the one `record` (setup-hashes.json) holds for it.
pub fn hashes(record: &Path, files: &[(PathBuf, Vec<String>)]) -> Result<(), String> {
    let text = fs::read_to_string(record).map_err(|e| format!("{} cannot be read ({e})", record.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{} is not JSON ({e})", record.display()))?;
    for (path, key) in files {
        let want = key
            .iter()
            .try_fold(&json, |v, k| v.get(k))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{} has no hash for {}", record.display(), key.join(" ")))?;
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(want.trim()) {
            return Err(format!(
                "{} is not the file Setup made (SHA-256 {got}, recorded {want})",
                path.display()
            ));
        }
    }
    Ok(())
}

/// A 32-bit program's import table names wccfnet.dll, not IPHLPAPI.DLL, and takes none of the four calls from
/// anywhere else.
pub fn imports(exe: &[u8]) -> Result<(), String> {
    let list = import_list(exe).map_err(|e| format!("not a 32-bit Windows program ({e})"))?;
    if list.iter().any(|(lib, _)| lib.eq_ignore_ascii_case("iphlpapi.dll")) {
        return Err("it still imports IPHLPAPI.DLL: this is not the guarded copy".into());
    }
    if !list.iter().any(|(lib, _)| lib.eq_ignore_ascii_case(WCCFNET)) {
        return Err(format!("it does not import {WCCFNET}"));
    }
    if let Some((lib, name)) = list
        .iter()
        .find(|(lib, name)| !lib.eq_ignore_ascii_case(WCCFNET) && name.iter().any(|n| BLOCKED.contains(&n.as_str())))
    {
        return Err(format!("it imports {} from {lib}", name.join(", ")));
    }
    Ok(())
}

/// (DLL, the functions imported from it by name) for each import descriptor.
fn import_list(exe: &[u8]) -> object::Result<Vec<(String, Vec<String>)>> {
    let pe = PeFile32::parse(exe)?;
    let mut out = Vec::new();
    let Some(table) = pe.import_table()? else {
        return Ok(out);
    };
    let mut descs = table.descriptors()?;
    while let Some(d) = descs.next()? {
        let lib = String::from_utf8_lossy(table.name(d.name.get(LE))?).into_owned();
        let first = match d.original_first_thunk.get(LE) {
            0 => d.first_thunk.get(LE),
            rva => rva,
        };
        let mut names = Vec::new();
        let mut thunks = table.thunks(first)?;
        while let Some(t) = thunks.next::<ImageNtHeaders32>()? {
            if let Import::Name(_, name) = table.import::<ImageNtHeaders32>(t)? {
                names.push(String::from_utf8_lossy(name).into_owned());
            }
        }
        out.push((lib, names));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(data: &[u8]) -> String {
        Sha256::digest(data).iter().map(|b| format!("{b:02X}")).collect()
    }

    /// A fake setup-hashes.json over two fake files: it holds, a changed file or a missing hash is refused, and so is
    /// no record at all.
    #[test]
    fn the_record_must_match_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.exe"), dir.path().join("b.dll"));
        fs::write(&a, b"guarded copy").unwrap();
        fs::write(&b, b"the guard").unwrap();
        let record = dir.path().join(HASHES);
        let files = vec![
            (a.clone(), vec!["a.exe".to_owned(), "guarded".to_owned()]),
            (b.clone(), vec!["b.dll".to_owned()]),
        ];
        assert!(hashes(&record, &files).unwrap_err().contains("cannot be read"));
        let json = serde_json::json!({"a.exe": {"original": "00", "guarded": sha(b"guarded copy")},
                                      "b.dll": sha(b"the guard").to_lowercase()});
        fs::write(&record, json.to_string()).unwrap();
        hashes(&record, &files).unwrap();

        fs::write(&a, b"the original").unwrap();
        let why = hashes(&record, &files).unwrap_err();
        assert!(why.contains("a.exe is not the file Setup made"), "{why}");
        fs::write(&a, b"guarded copy").unwrap();

        let json = serde_json::json!({"a.exe": {"original": sha(b"guarded copy")}, "b.dll": sha(b"the guard")});
        fs::write(&record, json.to_string()).unwrap();
        assert!(
            hashes(&record, &files)
                .unwrap_err()
                .contains("no hash for a.exe guarded")
        );
        fs::write(&record, "{not json").unwrap();
        assert!(hashes(&record, &files).unwrap_err().contains("is not JSON"));
        fs::remove_file(&b).unwrap();
        let json = serde_json::json!({"a.exe": {"guarded": sha(b"guarded copy")}, "b.dll": sha(b"the guard")});
        fs::write(&record, json.to_string()).unwrap();
        assert!(hashes(&record, &files).unwrap_err().contains("b.dll"));
    }

    #[test]
    fn not_a_program_is_refused() {
        assert!(
            imports(b"MZ nothing")
                .unwrap_err()
                .contains("not a 32-bit Windows program")
        );
    }
}
