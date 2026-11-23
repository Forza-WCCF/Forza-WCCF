use super::*;

/// A `club_out.json` written before the arcade rename still loads, and the keys on disk stay the same.
#[test]
fn club_out_keeps_its_old_keys() {
    let old = r#"{"shop":"Sala Roma","since":1,"seat":2,"home_aime":3,"shop_aime":4,
        "link":{"coaches":[],"cards":[]},"home_stamp":"a","host_stamp":"b","back_stamp":"c","back":0}"#;
    let out: Out = serde_json::from_str(old).unwrap();
    assert_eq!((out.arcade.as_str(), out.arcade_aime), ("Sala Roma", 4));
    let json = serde_json::to_value(&out).unwrap();
    assert!(
        json.get("shop").is_some() && json.get("shop_aime").is_some(),
        "{json}"
    );
}

#[test]
fn ask_lines() {
    let t = "ab".repeat(32);
    assert_eq!(
        Ask::parse(&format!("CLUB PUT 2 952833657 {t} 100")),
        Some(Ask::Put {
            seat: 2,
            aime: 952833657,
            token: t.clone(),
            size: 100
        })
    );
    assert_eq!(
        Ask::parse(&format!("CLUB GET 7 {t} -")),
        Some(Ask::Get {
            aime: 7,
            token: t.clone(),
            stamp: "-".into()
        })
    );
    for bad in [
        format!("CLUB PUT 9 5 {t} 100"),
        format!("CLUB PUT 2 0 {t} 100"),
        format!("CLUB PUT 2 5 {t} 0"),
        format!("CLUB PUT 2 5 {t} {}", MAX_BYTES + 1),
        format!("CLUB PUT 2 5 {} 100", "AB".repeat(32)),
        format!("CLUB PUT 2 5 {} 100", "ab".repeat(31)),
        "CLUB DONE 5".into(),
        "CLUB".into(),
    ] {
        assert_eq!(Ask::parse(&bad), None, "{bad}");
    }
}

use std::net::TcpListener;
use std::sync::Mutex;

const CODE: &str = "50123456789012345678";
/// The Aime id of the test desk's own card ([`desk`]): its host's club.
const OWN: u32 = 9;

fn sum32(b: &[u8]) -> u32 {
    b.as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |s, w| s.wrapping_add(u32::from_le_bytes(*w)))
}

/// A SAVE_DATA_SET 277 as the client sends it: ids, kind 2, `block`, the revision and the block's sum.
fn save_277(aime: u32, coach: u32, card: u32, block: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; SAVE_SET_LEN];
    for (i, b) in p.iter_mut().enumerate().skip(TAIL) {
        *b = (i * 13 + 5) as u8;
    }
    put(&mut p, 4, aime);
    put(&mut p, 8, coach);
    put(&mut p, 12, card);
    p[24] = 2;
    p[28..28 + BLOCK].copy_from_slice(block);
    put(&mut p, 3100, 12);
    p[3102..3104].copy_from_slice(&(BLOCK as u16).to_le_bytes());
    put(&mut p, 3224, 0);
    put(&mut p, 5236, sum32(block));
    put(&mut p, 5940, 47565);
    p
}

/// A saved club as the store keeps it: the ids inside its block too.
fn coach(aime: u32, coach: u32, card: u32, seed: u8) -> Coach {
    let mut block: Vec<u8> = (0..BLOCK)
        .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
        .collect();
    for bit in CARD_BITS {
        put_bits(&mut block, bit, 32, card);
    }
    put_bits(&mut block, COACH_BIT, 32, coach);
    let req = save_277(aime, coach, card, &block);
    Coach {
        coach,
        card,
        name1: b"MANAGER".to_vec(),
        name2: b"CLUB".to_vec(),
        rec122: 90,
        play_count: 7,
        titles: 16,
        created: 100,
        updated: 200,
        save: Some(Save {
            block,
            version: 12,
            size: BLOCK as u16,
            kind: 2,
            updated: 200,
            tail: req[TAIL..].to_vec(),
        }),
        injustice: Some(1),
        requests: vec![Request {
            cmd: SAVE_SET,
            payload: req,
            updated: 200,
        }],
    }
}

fn purchase(aime: u32) -> Purchase {
    let mut payload = vec![0u8; 20];
    put(&mut payload, 4, aime);
    put(&mut payload, 8, 1);
    put(&mut payload, 12, 2);
    Purchase { at: 150, payload }
}

fn club(aime: u32, ids: &[(u32, u32)]) -> Club {
    Club {
        aime,
        coaches: ids
            .iter()
            .enumerate()
            .map(|(i, &(k, c))| coach(aime, k, c, i as u8 + 3))
            .collect(),
        purchases: vec![purchase(aime)],
    }
}

/// A store at `path` holding `c`, its counters past `c`'s ids.
fn store_with(path: &Path, c: &Club) -> Store {
    let st = Store::open(path).unwrap();
    insert(&st.0, c).unwrap();
    let top = c
        .coaches
        .iter()
        .map(|k| k.coach.max(k.card))
        .max()
        .unwrap_or(0);
    st.0.execute("INSERT INTO ids VALUES ('coach', ?1), ('card', ?1)", [top])
        .unwrap();
    st
}

/// CLEAR BAD ENDINGS (2017-18): home counters to 0 after a copy of the store; a guest's counter stays; a second
/// clear has nothing to do; a club out in an arcade is left as it is.
#[test]
fn clear_bad_endings_zeroes_home_clubs_after_a_backup() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("gamesvr.db");
    let s = Store::open(&db).unwrap();
    s.0.execute_batch(
        "INSERT INTO injustice VALUES (7, 1, 3), (7, 2, 0), (9, 1, 2);
         INSERT INTO guests VALUES (9, 't', 2, 0);",
    )
    .unwrap();
    drop(s);
    let said = clear_bad_endings(d.path()).unwrap();
    assert!(said.starts_with("bad endings cleared (1 club(s)"), "{said}");
    let count = |c: &Connection, a: u32, k: u32| -> u32 {
        c.query_row(
            "SELECT count FROM injustice WHERE aime_id = ?1 AND coach_id = ?2",
            [a, k],
            |r| r.get(0),
        )
        .unwrap()
    };
    let now = Connection::open(&db).unwrap();
    assert_eq!((count(&now, 7, 1), count(&now, 9, 1)), (0, 2));
    let copies: Vec<_> = std::fs::read_dir(d.path().join("backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(count(&Connection::open(copies[0].path()).unwrap(), 7, 1), 3);
    assert_eq!(
        clear_bad_endings(d.path()).unwrap(),
        "no bad endings to clear"
    );
    now.execute(
        "UPDATE injustice SET count = 4 WHERE aime_id = 7 AND coach_id = 1",
        [],
    )
    .unwrap();
    std::fs::write(out_file(d.path()), b"{}").unwrap();
    assert!(
        clear_bad_endings(d.path())
            .unwrap()
            .contains("out in an arcade")
    );
    assert_eq!(count(&now, 7, 1), 4);
}

/// An old card at home (integer Aime id 1, from before the hash) brings two clubs to a host whose own club holds
/// the same coach and card ids; a match is saved there through the stand-in; the copy back restores the home
/// ids and every other byte, with the match's changes.
#[test]
fn round_trip_restores_the_ids_and_every_byte() {
    let d = tempfile::tempdir().unwrap();
    let (home_aime, arcade_aime) = (1, crate::standins::aimedb::hash_id(CODE) as u32);
    let sent = club(home_aime, &[(1, 1), (2, 3)]);
    let mut home = store_with(&d.path().join("home.db"), &sent);
    let host = store_with(
        &d.path().join("host.db"),
        &club(9, &[(1, 1), (2, 2), (3, 3)]),
    );
    let host_own = host.club(9).unwrap();
    let out = home.club(home_aime).unwrap();
    assert_eq!(out, sent);
    let host = Mutex::new(host);
    let link = host
        .lock()
        .unwrap()
        .import_guest(&out, arcade_aime, 2, &token(CODE))
        .unwrap();
    assert_eq!(link.coaches, [(1, 4), (2, 5)]);
    assert_eq!(link.cards, [(1, 4), (3, 5)]);

    // in the arcade: host ids everywhere the store and the client read them
    let there = host
        .lock()
        .unwrap()
        .guest_club(arcade_aime, &token(CODE))
        .unwrap();
    assert_eq!(there.aime, arcade_aime);
    for (k, (coach, card)) in there.coaches.iter().zip([(4, 4), (5, 5)]) {
        assert_eq!((k.coach, k.card), (coach, card));
        let b = &k.save.as_ref().unwrap().block;
        assert_eq!(
            [
                get_bits(b, 185, 32),
                get_bits(b, 263, 32),
                get_bits(b, 217, 32)
            ],
            [card, card, coach]
        );
        let p = &k.requests[0].payload;
        assert_eq!(
            [u32_at(p, 4), u32_at(p, 8), u32_at(p, 12)],
            [arcade_aime, coach, card]
        );
    }
    assert_eq!(u32_at(&there.purchases[0].payload, 4), arcade_aime);
    assert_eq!(
        host.lock().unwrap().club(9).unwrap(),
        host_own,
        "the host's own club is untouched"
    );

    // nothing played: the copy back is the club sent, byte for byte
    let mut back = link.clone();
    let stamp = home
        .write_home(&there, &mut back, home_aime, &out.stamp())
        .unwrap()
        .unwrap();
    assert_eq!(home.club(home_aime).unwrap(), sent);
    assert_eq!(back, link);

    // a match saved in the arcade by the stand-in (277 with the arcade's ids, one byte of the block changed)
    let k = &there.coaches[1];
    let mut block = k.save.as_ref().unwrap().block.clone();
    block[1000] ^= 0x5A;
    let (_, refused) = super::super::gamesvr::ask_for_test(
        &host,
        277,
        &save_277(arcade_aime, 5, 5, &block),
        &(Arc::new(|_: &str| {}) as Log),
    );
    assert_eq!(refused, None);
    let after = host
        .lock()
        .unwrap()
        .guest_club(arcade_aime, &token(CODE))
        .unwrap();
    home.write_home(&after, &mut back, home_aime, &stamp)
        .unwrap()
        .unwrap();
    let got = home.club(home_aime).unwrap();
    assert_eq!(
        got.coaches[0], sent.coaches[0],
        "the club that did not play comes back as it went"
    );
    let (g, s) = (&got.coaches[1], &sent.coaches[1]);
    assert_eq!((g.coach, g.card, g.play_count), (2, 3, s.play_count + 1));
    let mut want = s.save.as_ref().unwrap().block.clone();
    want[1000] ^= 0x5A;
    assert_eq!(
        g.save.as_ref().unwrap().block,
        want,
        "the home ids back in the block, the match's byte kept"
    );
    let p = &g.requests[0].payload;
    assert_eq!(
        [u32_at(p, 4), u32_at(p, 8), u32_at(p, 12)],
        [home_aime, 2, 3]
    );
    assert_eq!(p[28..28 + BLOCK], want[..]);
    assert_eq!(got.purchases, sent.purchases);
    assert_eq!(host.lock().unwrap().club(9).unwrap(), host_own);
}

/// A club made in the arcade gets fresh home ids; the next copy back keeps them.
#[test]
fn a_club_made_in_the_arcade_gets_home_ids() {
    let d = tempfile::tempdir().unwrap();
    let mut home = store_with(&d.path().join("home.db"), &club(1, &[(1, 1)]));
    let mut host = store_with(&d.path().join("host.db"), &club(9, &[(1, 1)]));
    let link = host
        .import_guest(&home.club(1).unwrap(), 50, 3, &token(CODE))
        .unwrap();
    // made in the arcade: coach 7 / card 8, as COACH_LIST_ADD would
    insert(
        &host.0,
        &Club {
            aime: 50,
            coaches: vec![coach(50, 7, 8, 9)],
            purchases: vec![],
        },
    )
    .unwrap();
    let there = host.guest_club(50, &token(CODE)).unwrap();
    let mut back = link;
    let stamp = home
        .write_home(&there, &mut back, 1, &home.club(1).unwrap().stamp())
        .unwrap()
        .unwrap();
    let got = home.club(1).unwrap();
    let new = &got.coaches[1];
    assert_eq!((new.coach, new.card), (2, 2), "the home store's next ids");
    assert_eq!(get_bits(&new.save.as_ref().unwrap().block, 217, 32), 2);
    let again = back.clone();
    home.write_home(&there, &mut back, 1, &stamp)
        .unwrap()
        .unwrap();
    assert_eq!(back, again);
    assert_eq!(home.club(1).unwrap(), got);
}

/// A home club that changed since the copy is never overwritten.
#[test]
fn a_newer_home_club_is_kept() {
    let d = tempfile::tempdir().unwrap();
    let mut home = store_with(&d.path().join("home.db"), &club(1, &[(1, 1)]));
    let mut host = Store::open(&d.path().join("host.db")).unwrap();
    let stamp = home.club(1).unwrap().stamp();
    let mut link = host
        .import_guest(&home.club(1).unwrap(), 50, 3, &token(CODE))
        .unwrap();
    home.0
        .execute("UPDATE coaches SET play_count = 99", [])
        .unwrap();
    let mine = home.club(1).unwrap();
    let there = host.guest_club(50, &token(CODE)).unwrap();
    assert_eq!(home.write_home(&there, &mut link, 1, &stamp).unwrap(), None);
    assert_eq!(home.club(1).unwrap(), mine);
}

#[test]
fn a_guest_cannot_touch_another_guests_club() {
    let d = tempfile::tempdir().unwrap();
    let mut host = store_with(&d.path().join("host.db"), &club(9, &[(1, 1)]));
    let own = host.club(9).unwrap();
    let (a, b) = (token(CODE), token("50123456789012345679"));
    let mine = club(1, &[(1, 1)]);
    // a club no one brought is not readable or droppable without its token
    assert!(host.guest_club(9, &a).is_err());
    assert!(host.drop_guest(9, &a).is_err());
    assert_eq!(host.club(9).unwrap(), own);
    // the first guest owns its Aime id here; another token is refused
    host.import_guest(&mine, 100, 2, &a).unwrap();
    assert!(
        host.import_guest(&mine, 100, 3, &b)
            .unwrap_err()
            .contains("another player")
    );
    assert!(host.guest_club(100, &b).is_err());
    assert!(host.drop_guest(100, &b).is_err());
    assert_eq!(host.guest_club(100, &a).unwrap().coaches.len(), 1);
    // the same guest again replaces its copy
    host.import_guest(&club(1, &[(1, 1), (2, 2)]), 100, 2, &a)
        .unwrap();
    assert_eq!(host.guest_club(100, &a).unwrap().coaches.len(), 2);
    // malformed or foreign records
    let bad = |f: &dyn Fn(&mut Club)| {
        let mut c = club(1, &[(1, 1), (2, 2)]);
        f(&mut c);
        c
    };
    for (what, c) in [
        (
            "short block",
            bad(&|c| {
                c.coaches[0].save.as_mut().unwrap().block.pop();
            }),
        ),
        (
            "odd tail",
            bad(&|c| c.coaches[0].save.as_mut().unwrap().tail.push(0)),
        ),
        (
            "other command",
            bad(&|c| c.coaches[0].requests[0].cmd = 309),
        ),
        (
            "short request",
            bad(&|c| c.coaches[0].requests[0].payload.truncate(100)),
        ),
        (
            "another card's save",
            bad(&|c| put(&mut c.coaches[0].requests[0].payload, 4, 77)),
        ),
        (
            "another card's purchase",
            bad(&|c| put(&mut c.purchases[0].payload, 4, 77)),
        ),
        ("repeated coach", bad(&|c| c.coaches[1].coach = 1)),
        ("zero card", bad(&|c| c.coaches[1].card = 0)),
        ("long name", bad(&|c| c.coaches[0].name1 = vec![b'x'; 57])),
        ("Aime id 0", bad(&|c| c.aime = 0)),
    ] {
        assert!(host.import_guest(&c, 200, 2, &a).is_err(), "{what}");
    }
    assert!(host.guest_club(200, &a).is_err(), "nothing imported");
    // one club per seat at most: a guest cannot fill the host's store
    for aime in 101..108 {
        host.import_guest(&club(1, &[(1, 1)]), aime, 2, &a).unwrap();
    }
    assert!(
        host.import_guest(&club(1, &[(1, 1)]), 108, 2, &a)
            .unwrap_err()
            .contains("8 guests")
    );
    host.import_guest(&club(1, &[(1, 1)]), 107, 2, &a).unwrap();
    // the purge at the host's next start drops guests only
    assert_eq!(host.purge_guests().unwrap(), 8);
    assert!(!has_rows(&host.0, 100).unwrap());
    assert_eq!(host.club(9).unwrap(), own);
    // a club left under a card by an earlier session (its token gone) does not block the next bring: the club
    // brought lands and the one left behind goes to backups (before 2026-11-02 the bring was refused for good)
    host.import_guest(&club(1, &[(1, 1)]), 300, 2, &a).unwrap();
    host.0
        .execute("DELETE FROM guests WHERE aime_id = 300", [])
        .unwrap();
    host.import_guest(&club(1, &[(1, 1), (2, 2)]), 300, 2, &b)
        .unwrap();
    assert_eq!(host.guest_club(300, &b).unwrap().coaches.len(), 2);
    let kept = |kind: &str| {
        std::fs::read_dir(d.path().join("backups"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(kind))
            .count()
    };
    assert_eq!(
        kept("replaced-300-"),
        1,
        "the club left behind is in backups"
    );
    assert_eq!(kept("rejoined-"), 2, "and the copies the rejoins replaced");
}

/// A public server: the brought club replaces what the server kept for the card (a club made there, a copy
/// that never went home), another token is still refused, and the oldest copy makes room past the cap.
#[test]
fn a_public_server_keeps_the_brought_club() {
    let d = tempfile::tempdir().unwrap();
    let mut srv = store_with(&d.path().join("srv.db"), &club(50, &[(1, 1), (2, 2)]));
    let (a, b) = (token(CODE), token("50123456789012345679"));
    let mine = club(1, &[(1, 1)]);
    srv.import_public(&mine, 50, 1, &a, &|_| false).unwrap();
    assert_eq!(
        srv.guest_club(50, &a).unwrap().coaches.len(),
        1,
        "the made-there club is gone"
    );
    let kept = std::fs::read_dir(d.path().join("backups")).unwrap().count();
    assert_eq!(kept, 1, "the made-there club is in backups");
    assert!(
        srv.import_public(&mine, 50, 1, &b, &|_| false)
            .unwrap_err()
            .contains("another player")
    );
    // a newer home club replaces the copy left from an earlier session
    srv.import_public(&club(1, &[(1, 1), (2, 2), (3, 3)]), 50, 1, &a, &|_| false)
        .unwrap();
    assert_eq!(srv.guest_club(50, &a).unwrap().coaches.len(), 3);
    srv.0
        .execute("UPDATE guests SET since = 0 WHERE aime_id = 50", [])
        .unwrap();
    for aime in 1000..1000 + MAX_PUBLIC_GUESTS as u32 {
        srv.import_public(&club(1, &[(1, 1)]), aime, 1, &a, &|_| false)
            .unwrap();
    }
    assert!(srv.guest_club(50, &a).is_err(), "the oldest copy made room");
    assert!(!has_rows(&srv.0, 50).unwrap());
    assert!(srv.guest_club(1000, &a).is_ok());
}

/// A public server never makes room from a copy in play: past the cap a bring is refused while every card is
/// on the game server; otherwise the oldest copy not in play makes room, set aside in `backups` first.
#[test]
fn a_public_server_never_evicts_a_club_in_play() {
    let d = tempfile::tempdir().unwrap();
    let mut srv = store_with(&d.path().join("srv.db"), &club(7, &[(1, 1)]));
    let a = token(CODE);
    for aime in 1000..1000 + MAX_PUBLIC_GUESTS as u32 {
        srv.import_public(&club(1, &[(1, 1)]), aime, 1, &a, &|_| true)
            .unwrap();
    }
    let more = |srv: &mut Store, playing: &dyn Fn(u32) -> bool| {
        srv.import_public(&club(1, &[(1, 1)]), 5000, 1, &a, playing)
    };
    assert!(more(&mut srv, &|_| true).unwrap_err().contains("in play"));
    assert!(srv.guest_club(1000, &a).is_ok(), "nobody evicted");
    // the two oldest copies; only the older one's card is on the game server
    srv.0
        .execute(
            "UPDATE guests SET since = 0 WHERE aime_id IN (1000, 1001)",
            [],
        )
        .unwrap();
    more(&mut srv, &|a| a == 1000).unwrap();
    assert!(srv.guest_club(1000, &a).is_ok(), "in play: kept");
    assert!(srv.guest_club(1001, &a).is_err(), "not in play: made room");
    let set_aside = std::fs::read_dir(d.path().join("backups"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("evicted-1001-"));
    assert!(set_aside, "the evicted copy is in backups");
}

/// Strangers that fill a public server with fresh copies and keep asking for them (256 PUTs, then GETs) lock
/// no player out: a copy whose card is not on the game server always makes room for a real bring.
#[test]
fn strangers_asking_for_their_copies_cannot_lock_players_out() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("srv.db");
    let mut srv = store_with(&db, &club(7, &[(1, 1)]));
    let a = token(CODE);
    for aime in 1000..1000 + MAX_PUBLIC_GUESTS as u32 {
        srv.import_public(&club(1, &[(1, 1)]), aime, 1, &a, &|_| false)
            .unwrap();
    }
    let since = |srv: &Store| -> i64 {
        srv.0
            .query_row("SELECT sum(since) FROM guests", [], |r| r.get(0))
            .unwrap()
    };
    let before = since(&srv);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = l.local_addr().unwrap();
    thread::spawn(move || {
        for c in l.incoming() {
            let mut c = c.unwrap();
            let mut line = String::new();
            let _ = io::BufRead::read_line(&mut io::BufReader::new(&c), &mut line);
            let _ = serve(
                &db,
                line.trim(),
                Vec::new(),
                &mut c,
                &|_| false,
                &|_| false,
                Some(&|_| false),
            );
        }
    });
    thread::sleep(Duration::from_millis(1100)); // so a refreshed since would differ
    for aime in [1000, 1100, 1255] {
        assert!(fetch(at, aime, CODE, "").unwrap().is_some());
    }
    assert_eq!(since(&srv), before, "asking keeps no copy");
    let player = token("50123456789012345679");
    srv.import_public(&club(1, &[(1, 1)]), 6000, 1, &player, &|_| false)
        .unwrap();
    assert_eq!(srv.guest_club(6000, &player).unwrap().coaches.len(), 1);
}

/// A same-token rejoin (a Launcher killed mid-run brings its older home club again) replaces the arcade's copy
/// only once that copy is in `backups`.
#[test]
fn a_rejoin_keeps_the_arcades_copy_in_backups() {
    let d = tempfile::tempdir().unwrap();
    let mut host = store_with(&d.path().join("host.db"), &club(7, &[(1, 1)]));
    let a = token(CODE);
    host.import_guest(&club(1, &[(1, 1), (2, 2)]), 50, 2, &a)
        .unwrap();
    assert!(!d.path().join("backups").exists());
    host.import_guest(&club(1, &[(1, 1)]), 50, 2, &a).unwrap();
    let kept: Vec<_> = std::fs::read_dir(d.path().join("backups"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rejoined-50-")
        })
        .collect();
    assert_eq!(kept.len(), 1, "the arcade's copy is in backups");
    let copy: Club = serde_json::from_slice(&std::fs::read(&kept[0]).unwrap()).unwrap();
    assert_eq!(copy.coaches.len(), 2, "with both clubs");
}

/// Clubs set aside for many Aime ids (evictions a stranger drives) stop at [`KEEP_SET_ASIDE`] of each kind in
/// all; the newest stays, and other kinds are left alone.
#[test]
fn set_aside_clubs_stop_at_a_cap_across_aime_ids() {
    let d = tempfile::tempdir().unwrap();
    let srv = store_with(&d.path().join("srv.db"), &club(7, &[(1, 1)]));
    let b = d.path().join("backups");
    std::fs::create_dir_all(&b).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    for n in 0..KEEP_SET_ASIDE + 5 {
        let f = std::fs::File::create(b.join(format!("evicted-{}-x.json", 10_000 + n))).unwrap();
        f.set_modified(old).unwrap();
    }
    std::fs::write(b.join("club-1-x.json"), b"{}").unwrap();
    set_aside(&srv.0, 7, "evicted").unwrap();
    let names: Vec<String> = std::fs::read_dir(&b)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let evicted = names.iter().filter(|n| n.starts_with("evicted-")).count();
    assert_eq!(evicted, KEEP_SET_ASIDE);
    assert!(
        names.iter().any(|n| n.starts_with("evicted-7-")),
        "the newest stays"
    );
    assert!(
        names.iter().any(|n| n == "club-1-x.json"),
        "another kind stays"
    );
}

/// A PUT the desk can refuse from its line (a seat not taken, the host's own card, another token's copy) is
/// answered at once, its upload unread: a stranger cannot hold the connection for the upload's 60 s.
#[test]
fn a_put_is_refused_from_its_line_before_the_upload() {
    let d = tempfile::tempdir().unwrap();
    let host_db = d.path().join("host.db");
    let mut host = store_with(&host_db, &club(OWN, &[(1, 1)]));
    host.import_guest(&club(1, &[(1, 1)]), 50, 2, &token(CODE))
        .unwrap();
    drop(host);
    let (at, _down, _) = desk(host_db);
    let other = token("50123456789012345679");
    for (head, why) in [
        (format!("CLUB PUT 3 60 {other} {MAX_BYTES}\n"), "not taken"),
        (
            format!("CLUB PUT 2 {OWN} {other} {MAX_BYTES}\n"),
            "own club",
        ),
        (
            format!("CLUB PUT 2 50 {other} {MAX_BYTES}\n"),
            "another player",
        ),
    ] {
        let t = Instant::now();
        let got = ask(at, &head, &[]).unwrap_err();
        assert!(got.contains(why), "{got}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }
}

/// The host's desk for tests: `serve` on every connection, seat 2 taken; `down` closes connections unanswered;
/// `closed`: the next connection closes the desk itself (later ones are refused, as after the host's Stop).
fn desk(db: PathBuf) -> (SocketAddr, Arc<AtomicBool>, Arc<AtomicBool>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let (at, down, closed) = (
        l.local_addr().unwrap(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (off, gone) = (down.clone(), closed.clone());
    thread::spawn(move || {
        for c in l.incoming() {
            let mut c = c.unwrap();
            if gone.load(Ordering::Relaxed) {
                return;
            }
            if off.load(Ordering::Relaxed) {
                continue;
            }
            let mut buf = Vec::new();
            while !buf.contains(&b'\n') {
                let mut b = [0u8; 256];
                match c.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&b[..n]),
                }
            }
            let Some(at) = buf.iter().position(|&b| b == b'\n') else {
                continue;
            };
            let rest = buf.split_off(at + 1);
            let line = String::from_utf8_lossy(&buf).trim().to_owned();
            let _ = serve(&db, &line, rest, &mut c, &|s| s == 2, &|a| a == OWN, None);
            // like the desk's hang-up: read off an upload refused unread, so no reset loses the answer
            let _ = c.shutdown(std::net::Shutdown::Write);
            let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = io::copy(&mut (&c).take(MAX_BYTES as u64), &mut io::sink());
        }
    });
    (at, down, closed)
}

fn guest(desk: SocketAddr, dir: &Path, seat: u8) -> Guest {
    Guest {
        desk,
        seat,
        code: CODE.into(),
        home_aime: 1,
        arcade_aime: 50,
        dir: dir.to_path_buf(),
        arcade: "test arcade".into(),
        poll: Duration::from_millis(200),
    }
}

fn wait(what: &str, ok: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < end, "{what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// The whole trip over the desk words: backup, copy in, a save copied home while playing, home at the end and
/// the host's copy dropped; a seat not taken is refused.
#[test]
fn a_trip_over_the_desk() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    let sent = club(1, &[(1, 1)]);
    drop(store_with(&dir.join("gamesvr.db"), &sent));
    let host_db = d.path().join("host.db");
    drop(store_with(&host_db, &club(9, &[(1, 1)])));
    let (at, _down, _) = desk(host_db.clone());
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let keep = said.clone();
    let log: Log = Arc::new(move |l: &str| keep.lock().unwrap().push(l.to_owned()));
    assert!(
        guest(at, &dir, 3)
            .bring(log.clone())
            .err()
            .unwrap()
            .contains("seat 3 is not taken")
    );
    assert!(!out_file(&dir).exists());

    let trip = guest(at, &dir, 2).bring(log.clone()).unwrap();
    assert!(out_file(&dir).exists(), "marked out");
    let backups = std::fs::read_dir(dir.join("backups")).unwrap().count();
    assert_eq!(
        backups, 2,
        "the home club backed up first (the refused try made one too)"
    );
    // a save in the arcade comes home within a poll
    let host = Mutex::new(Store::open(&host_db).unwrap());
    let there = host.lock().unwrap().guest_club(50, &token(CODE)).unwrap();
    let mut block = there.coaches[0].save.as_ref().unwrap().block.clone();
    block[2000] ^= 1;
    let (k, c) = (there.coaches[0].coach, there.coaches[0].card);
    assert_eq!(
        super::super::gamesvr::ask_for_test(&host, 277, &save_277(50, k, c, &block), &log).1,
        None
    );
    let home = Store::open(&dir.join("gamesvr.db")).unwrap();
    wait("the save copied home", || {
        home.club(1).unwrap().coaches[0].play_count == 8
    });
    drop(trip);
    assert!(
        !out_file(&dir).exists() && !last_file(&dir).exists(),
        "home: marker gone"
    );
    assert!(
        !has_rows(&host.lock().unwrap().0, 50).unwrap(),
        "the host dropped its copy"
    );
    let got = home.club(1).unwrap();
    assert_eq!(
        get_bits(&got.coaches[0].save.as_ref().unwrap().block, 217, 32),
        1
    );
    assert_eq!(
        got.coaches[0].save.as_ref().unwrap().block[2000],
        sent.coaches[0].save.as_ref().unwrap().block[2000] ^ 1
    );
    assert_eq!(leftover(&dir).unwrap(), None);
    // the club's cards and team, logged on copy-in, at the result save and around each copy-back
    let said = said.lock().unwrap().join("\n");
    let (old, new) = (
        team_tag(&sent.coaches[0].save.as_ref().unwrap().block),
        team_tag(&block),
    );
    for want in [
        format!("club copy-in, home: aime 1: 1 card(s) [card 1 coach 1 plays 7 team {old}]"),
        format!("club save: aime 50 coach {k} card {c} team {old} -> {new}"),
        format!(
            "club copy-back, home before: aime 1: 1 card(s) [card 1 coach 1 plays 7 team {old}]"
        ),
        format!(
            "club copy-back, home after: aime 1: 1 card(s) [card 1 coach 1 plays 8 team {new}]"
        ),
    ] {
        assert!(said.contains(&want), "{want:?} in:\n{said}");
    }
    assert!(said.contains("match end: aime 50 "), "{said}");
}

/// The logs' team tag reads the same at home and in the arcade (the swapped ids are left out of it).
#[test]
fn the_team_tag_ignores_the_swapped_ids() {
    let home = club(1, &[(1, 1)]);
    let link = Link {
        coaches: vec![(1, 70)],
        cards: vec![(1, 71)],
    };
    let host = home.remap(&Swap::out(&link, 1, 50));
    let tag = |c: &Club| team_tag(&c.coaches[0].save.as_ref().unwrap().block);
    assert_eq!(tag(&home), tag(&host));
    assert_eq!(tag(&home).len(), 8);
    assert_eq!(
        host.summary(),
        format!(
            "aime 50: 1 card(s) [card 71 coach 70 plays 7 team {}]",
            tag(&home)
        )
    );
}

/// The host dies mid-session: the club stays marked out; the next start keeps the home copy, puts the last
/// pulled copy in backups and says so once.
#[test]
fn a_host_crash_keeps_the_home_copy() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
    let host_db = d.path().join("host.db");
    let (at, down, _) = desk(host_db.clone());
    let trip = guest(at, &dir, 2).bring(Arc::new(|_: &str| {})).unwrap();
    wait("a first copy pulled", || last_file(&dir).exists());
    let home = Store::open(&dir.join("gamesvr.db")).unwrap();
    let before = home.club(1).unwrap();
    down.store(true, Ordering::Relaxed);
    let progress = trip.progress();
    drop(trip);
    let p = *progress.lock().unwrap();
    assert!(!p.home && p.last_ok.is_some(), "{p:?}");
    assert!(out_file(&dir).exists(), "not home: still marked out");
    assert!(
        has_rows(&Store::open(&host_db).unwrap().0, 50).unwrap(),
        "the host still holds it (purged at its start)"
    );
    // the last copy is home already (the desk's stamp is the parsed copy's): nothing to offer, a start says so
    assert!(
        matches!(left(&dir), Ok(Some(Left::Same { .. }))),
        "{:?}",
        left(&dir)
    );
    assert!(take(&dir).is_err(), "nothing to take");
    let (kept, home_at) = leftover(&dir).unwrap().expect("told");
    assert!(home_at.is_some_and(|t| t > 0), "the copy was home");
    assert!(
        kept.starts_with(dir.join("backups")) && kept.is_file(),
        "{kept:?}"
    );
    let (link, host): (Link, Club) =
        serde_json::from_slice(&std::fs::read(&kept).unwrap()).unwrap();
    assert_eq!((link.coaches.len(), host.aime), (1, 50));
    assert!(!out_file(&dir).exists());
    assert_eq!(home.club(1).unwrap(), before, "the home copy is kept");
    assert_eq!(leftover(&dir).unwrap(), None, "said once");
}

/// The host closes its arcade (its desk gone, the PC still up): the last copy-back says so, and the marker stays
/// (a save after the last good copy-back may be missing at home), so the next start tells it.
#[test]
fn a_closed_arcade_is_told_and_the_marker_stays() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
    let (addr, _down, closed) = desk(d.path().join("host.db"));
    let trip = guest(addr, &dir, 2).bring(Arc::new(|_: &str| {})).unwrap();
    wait("a first copy pulled", || last_file(&dir).exists());
    let progress = trip.progress();
    wait("a good copy-back", || {
        progress.lock().unwrap().last_ok.is_some()
    });
    closed.store(true, Ordering::Relaxed);
    let _ = TcpStream::connect(addr); // wakes the desk, which then closes its listener
    wait("a refused copy-back", || progress.lock().unwrap().gone_once);
    drop(trip);
    let p = *progress.lock().unwrap();
    assert!(!p.home && p.gone, "{p:?}");
    assert!(out_file(&dir).exists(), "not home: still marked out");
    assert!(leftover(&dir).unwrap().is_some(), "the next start tells it");
}

#[test]
fn an_old_card_is_found_under_its_home_id() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("aimedb.json");
    std::fs::write(&path, format!("{{\"{CODE}\": 1}}")).unwrap();
    let db = crate::standins::aimedb::Db::open(path).unwrap();
    assert_eq!(db.home_id(CODE), 1);
    let other = "50123456789012345679";
    assert_eq!(db.home_id(other), crate::standins::aimedb::hash_id(other));
    assert_eq!(db.len(), 1, "read only");
    // this PC's card under its old id and under its hash; another card under neither
    let hash = |c: &str| crate::standins::aimedb::hash_id(c) as u32;
    assert!(db.holds(1) && db.holds(hash(CODE)));
    assert!(!db.holds(hash(other)) && !db.holds(2));
}

/// A guest that names one of the host's cards in its PUT (an old card's id is easy to guess) is refused, and
/// the host's club stays as it was.
#[test]
fn a_guest_cannot_replace_the_hosts_own_club() {
    let d = tempfile::tempdir().unwrap();
    let host_db = d.path().join("host.db");
    drop(store_with(&host_db, &club(OWN, &[(1, 1), (2, 2)])));
    let own = Store::open(&host_db).unwrap().club(OWN).unwrap();
    let (at, _down, _) = desk(host_db.clone());
    let body = serde_json::to_vec(&club(1, &[(1, 1)])).unwrap();
    let other = token("50123456789012345679");
    let head = format!("CLUB PUT 2 {OWN} {other} {}\n", body.len());
    let why = ask(at, &head, &body).unwrap_err();
    assert!(why.contains("own club"), "{why}");
    assert_eq!(Store::open(&host_db).unwrap().club(OWN).unwrap(), own);
    assert!(!d.path().join("backups").exists(), "nothing replaced");
}

/// The host's own card on a second PC joins the host's arcade: the bring is refused (that PC plays the
/// arcade's club as it is), and the arcade's club survives the next start's purge.
#[test]
fn the_arcades_own_club_survives_a_bring_and_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
    let host_db = d.path().join("host.db");
    drop(store_with(&host_db, &club(OWN, &[(1, 1), (2, 2)])));
    let own = Store::open(&host_db).unwrap().club(OWN).unwrap();
    let (at, _down, _) = desk(host_db.clone());
    let mut second = guest(at, &dir, 2);
    second.arcade_aime = OWN;
    let why = second.bring(Arc::new(|_: &str| {})).err().unwrap();
    assert!(why.contains("own club"), "{why}");
    assert!(!out_file(&dir).exists(), "not marked out");
    let mut host = Store::open(&host_db).unwrap();
    assert_eq!(host.purge_guests().unwrap(), 0);
    assert_eq!(host.club(OWN).unwrap(), own);
}

/// A host's copy that lost a club the guest brought (or fails the check, or holds another Aime id) is set aside
/// in `backups`, never written home; the marker stays at the end.
#[test]
fn a_bad_copy_back_is_never_written_home() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
    let host_db = d.path().join("host.db");
    let (at, _down, _) = desk(host_db.clone());
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let keep = said.clone();
    let log: Log = Arc::new(move |l: &str| keep.lock().unwrap().push(l.to_owned()));
    let g = guest(at, &dir, 2);
    let trip = guest(at, &dir, 2).bring(log).unwrap();
    let progress = trip.progress();
    wait("a good copy-back", || {
        progress.lock().unwrap().last_ok.is_some()
    });
    let home = Store::open(&dir.join("gamesvr.db")).unwrap();
    let before = home.club(1).unwrap();
    let link = Link {
        coaches: vec![(1, 70)],
        cards: vec![(1, 71)],
    };
    let there = club(50, &[(70, 71)]);
    assert!(sound(g.arcade_aime, &link, &there).is_ok());
    assert!(
        sound(g.arcade_aime, &link, &club(51, &[(70, 71)])).is_err(),
        "another Aime id"
    );
    assert!(
        sound(g.arcade_aime, &link, &club(50, &[(72, 71)])).is_err(),
        "a club lost"
    );
    let mut bad = there.clone();
    bad.coaches[0].save.as_mut().unwrap().block.pop();
    assert!(sound(g.arcade_aime, &link, &bad).is_err(), "a short block");
    // the host's copy loses the club (an empty answer from a broken or modified host)
    delete(&Store::open(&host_db).unwrap().0, 50).unwrap();
    wait("the copy refused", || {
        said.lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("club copy-back refused ("))
    });
    drop(trip);
    assert_eq!(home.club(1).unwrap(), before, "the home club stays");
    assert!(out_file(&dir).exists(), "not home: still marked out");
    let refused = std::fs::read_dir(dir.join("backups"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("club-refused-"));
    assert!(refused, "the arcade's copy is in backups");
}

/// After a refused copy-back the host's copy stays the same, and the desk answers `SAME` from then on: that is
/// not a copy-back that brought the club home, so the last good one keeps its time.
#[test]
fn a_refused_copy_back_is_not_counted_as_home() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    std::fs::create_dir_all(&dir).unwrap();
    drop(store_with(&dir.join("gamesvr.db"), &club(1, &[(1, 1)])));
    let host_db = d.path().join("host.db");
    let (at, _down, _) = desk(host_db.clone());
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let keep = said.clone();
    let log: Log = Arc::new(move |l: &str| keep.lock().unwrap().push(l.to_owned()));
    let trip = guest(at, &dir, 2).bring(log).unwrap();
    let progress = trip.progress();
    wait("a good copy-back", || {
        progress.lock().unwrap().last_ok.is_some()
    });
    delete(&Store::open(&host_db).unwrap().0, 50).unwrap();
    wait("the copy refused", || {
        said.lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("club copy-back refused ("))
    });
    let ok = progress.lock().unwrap().last_ok;
    thread::sleep(Duration::from_secs(1)); // five more polls, each answered SAME
    assert_eq!(progress.lock().unwrap().last_ok, ok, "SAME is not home");
    drop(trip);
    assert!(!progress.lock().unwrap().home);
}

#[test]
fn token_hides_the_code() {
    let code = "50123456789012345678";
    assert_eq!(token(code).len(), 64);
    assert!(!token(code).contains(code));
    assert_ne!(token(code), token("50123456789012345679"));
}

/// The link of [`left_out`]'s visit: home coach and card 1 are 70 and 71 in the arcade.
fn visit_link() -> Link {
    Link {
        coaches: vec![(1, 70)],
        cards: vec![(1, 71)],
    }
}

/// A visit that ended before the last copy came home (the home write failed, so the marker's stamps never moved):
/// the home club in the store, the marker and a sound `club_last.json` holding a match more. -> (home, the copy).
fn left_out(dir: &Path) -> (Club, Club) {
    std::fs::create_dir_all(dir).unwrap();
    let home = club(1, &[(1, 1)]);
    drop(store_with(&dir.join("gamesvr.db"), &home));
    let link = visit_link();
    let mut host = home.remap(&Swap::out(&link, 1, 50));
    host.coaches[0].play_count += 1;
    host.coaches[0].save.as_mut().unwrap().block[2000] ^= 1;
    let out = Out {
        arcade: "test arcade".into(),
        since: 1000,
        seat: 2,
        home_aime: 1,
        arcade_aime: 50,
        link: link.clone(),
        home_stamp: home.stamp(),
        host_stamp: String::new(),
        back_stamp: String::new(),
        back: 0,
    };
    std::fs::write(out_file(dir), serde_json::to_vec(&out).unwrap()).unwrap();
    std::fs::write(last_file(dir), serde_json::to_vec(&(&link, &host)).unwrap()).unwrap();
    (home, host)
}

fn backups(dir: &Path, kind: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("backups"))
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .strip_prefix(kind)
                        .is_some_and(|r| r.starts_with('-'))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// #434: the arcade's last copy never came home; USE THE ARCADE COPY puts it home with the home ids, the home club
/// from before in `backups`, the marker gone and the undo kept.
#[test]
fn an_arcade_copy_that_never_came_home_can_be_taken() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    let (home, host) = left_out(&dir);
    let Ok(Some(Left::Differs(o))) = left(&dir) else {
        panic!("{:?}", left(&dir))
    };
    assert_eq!(
        (o.arcade.as_str(), o.home_at, o.plays),
        ("test arcade", 1000, Some((7, 8)))
    );
    assert!(o.pulled > 0);
    let saved = take(&dir).unwrap();
    assert_eq!(saved.after, None);
    let saved = saved.backup;
    let store = Store::open(&dir.join("gamesvr.db")).unwrap();
    let want = host.remap(&Swap::out(&visit_link(), 1, 50).inverse());
    assert_eq!(store.club(1).unwrap(), want, "the copy is home, home ids");
    assert_eq!(backups(&dir, "club-1"), std::slice::from_ref(&saved));
    let old: Club = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(old, home, "the home club from before is backed up");
    assert!(!out_file(&dir).exists() && !last_file(&dir).exists());
    assert!(can_undo(&dir));
    assert_eq!(
        backups(&dir, "club-shop").len(),
        1,
        "the copy kept as at a start"
    );
    assert_eq!(left(&dir), Ok(None), "offered once");
}

/// A step after the copy went home fails (here the undo file cannot be written): the take still counts, with the
/// home club's backup to tell, and the rest (marker, copy) still goes.
#[test]
fn a_take_that_fails_after_the_write_still_says_taken() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    let (home, host) = left_out(&dir);
    // a folder where the undo file goes: its write cannot land
    std::fs::create_dir(undo_file(&dir)).unwrap();
    let t = take(&dir).expect("taken: the copy is home");
    assert!(
        t.after
            .as_deref()
            .is_some_and(|e| e.contains("club_undo.json")),
        "{:?}",
        t.after
    );
    let old: Club = serde_json::from_slice(&std::fs::read(&t.backup).unwrap()).unwrap();
    assert_eq!(old, home, "the home club from before is backed up");
    let store = Store::open(&dir.join("gamesvr.db")).unwrap();
    let want = host.remap(&Swap::out(&visit_link(), 1, 50).inverse());
    assert_eq!(store.club(1).unwrap(), want, "the copy is home");
    assert!(!out_file(&dir).exists(), "settled all the same");
}

/// Declining (a start without the offer taken): the home club stays and the copy goes to `backups`, as before #434.
#[test]
fn a_declined_copy_leaves_the_home_club() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    let (home, _) = left_out(&dir);
    let (kept, home_at) = leftover(&dir).unwrap().expect("told");
    assert_eq!(home_at, None, "the copy differed");
    assert_eq!(backups(&dir, "club-shop"), [kept]);
    let store = Store::open(&dir.join("gamesvr.db")).unwrap();
    assert_eq!(store.club(1).unwrap(), home);
    assert!(!out_file(&dir).exists() && !last_file(&dir).exists());
}

/// A copy a copy-back would refuse is never offered nor taken, and nothing at home moves.
#[test]
fn a_bad_arcade_copy_is_never_offered() {
    type Bad = fn(&mut Club) -> Option<Vec<u8>>;
    let cases: [(&str, Bad); 7] = [
        ("Aime id 0", |c| {
            c.aime = 0;
            None
        }),
        ("a save of the wrong size", |c| {
            c.coaches[0].save.as_mut().unwrap().block.truncate(10);
            None
        }),
        ("another Aime id", |c| {
            *c = c.remap(&Swap {
                aimes: HashMap::from([(50, 51)]),
                ..Swap::default()
            });
            None
        }),
        ("a brought club missing", |c| {
            c.coaches.clear();
            None
        }),
        ("not JSON", |_| Some(b"{not json".to_vec())),
        ("over the size cap", |c| {
            let mut b = serde_json::to_vec(&(visit_link(), &*c)).unwrap();
            b.resize(MAX_BYTES + (64 << 10) + 1, b' ');
            Some(b)
        }),
        ("a marker that does not parse", |_| None),
    ];
    for (what, bad) in cases {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("gamesvr");
        let (home, mut host) = left_out(&dir);
        let bytes =
            bad(&mut host).unwrap_or_else(|| serde_json::to_vec(&(visit_link(), &host)).unwrap());
        std::fs::write(last_file(&dir), bytes).unwrap();
        if what.starts_with("a marker") {
            std::fs::write(out_file(&dir), b"{").unwrap();
        }
        assert!(left(&dir).is_err(), "{what}: {:?}", left(&dir));
        assert!(matches!(take(&dir), Err(NotReplaced::Failed(_))), "{what}");
        let store = Store::open(&dir.join("gamesvr.db")).unwrap();
        assert_eq!(store.club(1).unwrap(), home, "{what}");
        assert!(!dir.join("backups").exists(), "{what}: no backups");
    }
}

/// PUT THE HOME CLUB BACK restores the club from before the take; a club changed since stays; a start ends the undo.
#[test]
fn a_taken_copy_can_be_undone_until_the_next_start() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    let (home, _) = left_out(&dir);
    take(&dir).unwrap();
    undo(&dir).unwrap();
    let store = Store::open(&dir.join("gamesvr.db")).unwrap();
    assert_eq!(store.club(1).unwrap(), home, "the home club is back");
    assert!(!can_undo(&dir));
    assert!(undo(&dir).is_err(), "once");

    // the home club changes after the take: the undo refuses and goes, the club stays
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    left_out(&dir);
    take(&dir).unwrap();
    let mut store = Store::open(&dir.join("gamesvr.db")).unwrap();
    let mut changed = store.club(1).unwrap();
    changed.coaches[0].play_count += 1;
    let now = changed.stamp();
    assert!(
        store
            .put_home(&changed, &store.club(1).unwrap().stamp())
            .unwrap()
    );
    assert_eq!(store.club(1).unwrap().stamp(), now);
    assert_eq!(undo(&dir), Err(NotReplaced::Changed(dir.join("backups"))));
    assert_eq!(store.club(1).unwrap(), changed);
    assert!(!can_undo(&dir));

    // a start (leftover) ends the undo
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("gamesvr");
    left_out(&dir);
    take(&dir).unwrap();
    assert!(can_undo(&dir));
    assert_eq!(leftover(&dir).unwrap(), None, "nothing more to tell");
    assert!(!can_undo(&dir));
}
