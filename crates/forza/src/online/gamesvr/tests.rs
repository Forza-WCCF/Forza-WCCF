use super::*;
use proptest::prelude::*;

// The framing rule on any header: a length between the 40-byte header and the cap, or the oversized length it
// refused (`fill` sizes its buffer from the first, closes on the second). And a header `header` writes is one the
// reader reads back: the size it claims, its fields and its checksum.
proptest! {
    #[test]
    fn frame_len_stays_within_40_and_the_cap(
        raw in prop::collection::vec(any::<u8>(), 40),
        // the sizes that matter: the 40-byte floor, the cap, and whatever a peer sends
        size in prop_oneof![
            0u32..64,
            (MAX_FRAME as u32 - 32)..(MAX_FRAME as u32 + 32),
            any::<u32>(),
        ],
    ) {
        let mut h = raw;
        h[16..20].copy_from_slice(&size.to_le_bytes());
        match frame_len(&h) {
            Ok(n) => prop_assert!((40..=MAX_FRAME).contains(&n), "{} of {h:02x?}", n),
            Err(n) => prop_assert!(n > MAX_FRAME, "{} of {h:02x?}", n),
        }
        // and the rule the reader follows, from the field alone
        let claim = (size as usize).max(40);
        let want = if claim > MAX_FRAME { Err(claim) } else { Ok(claim) };
        prop_assert_eq!(frame_len(&h), want);
    }

    #[test]
    fn a_written_header_reads_back_whole(
        cmd in any::<u16>(),
        seq in any::<u16>(),
        status in any::<u32>(),
        size in 0..=(MAX_FRAME - 40),
        id in any::<u32>(),
    ) {
        let h = header(cmd, seq, status, size, id);
        prop_assert_eq!(frame_len(&h), Ok(40 + size));
        prop_assert_eq!(&h[..8], &MAGIC);
        prop_assert_eq!(u16::from_le_bytes([h[8], h[9]]), cmd);
        prop_assert_eq!(u16::from_le_bytes([h[10], h[11]]), seq);
        prop_assert_eq!(u32_at(&h, 12), status);
        prop_assert_eq!(u32_at(&h, 20), id);
        prop_assert_eq!(checksum(&h), h[24]);
    }
}

#[test]
fn live_download_chunks_fill_the_buffer_exactly() {
    assert_eq!(
        (
            live_chunk_size(0),
            live_chunk_size(124),
            live_chunk_size(125)
        ),
        (2032, 504, 0)
    );
    assert_eq!((0..125).map(live_chunk_size).sum::<u32>(), 0x3DA38);
    let mut p = vec![0u8; 8];
    put(&mut p, 4, 124);
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let (cmd, body, _) = answer(261, &p, &store, None, &[], None, &log).unwrap();
    assert_eq!((cmd, body.len(), u32_at(&body, 4)), (262, 2040, 504));
}

/// Where round `round` (0..8) of competition `compe` (0..2) keeps its 6,480-byte match in the control's 261 buffer
/// (the answers' bytes from +8, joined): the buffer's own 8-byte head (week id, weekday), then 68,816 bytes per
/// competition, 8,602 per round, the match at +58 (control sub_47AFE0, sub_47A530).
fn live_game_at(compe: usize, round: usize) -> usize {
    8 + 68_816 * compe + 8_602 * round + 58
}

#[test]
fn the_live_download_holds_no_data_match_to_replay() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let mut buf = Vec::new();
    for chunk in 0..125 {
        let mut p = vec![0u8; 8];
        put(&mut p, 4, chunk);
        let (_, body, _) = answer(261, &p, &store, None, &[], None, &log).unwrap();
        let n = u32_at(&body, 4) as usize;
        buf.extend_from_slice(&body[8..8 + n]); // what sub_443090 copies: answer +8, size at +4
    }
    assert_eq!(buf.len(), 0x3DA38);
    // the layout: 8 rounds fill a competition, 2 competitions end where the team names start (+137,640)
    assert_eq!(live_game_at(0, 1) - live_game_at(0, 0), 8_602);
    assert_eq!(live_game_at(1, 0) - live_game_at(0, 0), 8 * 8_602);
    assert_eq!(live_game_at(2, 0) - 58, 137_640);
    // every match slot is zero ("LiveGameInit bin is empty"), as is the rest: names and emblems of nobody
    for (c, r) in (0..2).flat_map(|c| (0..8).map(move |r| (c, r))) {
        let at = live_game_at(c, r);
        assert!(
            buf[at..at + 6480].iter().all(|&b| b == 0),
            "competition {c} round {r}: a non-zero match would play on the projector"
        );
    }
    assert!(buf.iter().all(|&b| b == 0));
}

#[test]
fn c1_c2_of_hand_built_blocks() {
    // 768 words of 1: C1 = 768; 0xFFFF_FFFF words wrap to -768
    assert_eq!(sum32(&[1, 0, 0, 0].repeat(768)), 768);
    assert_eq!(sum32(&[0xFF; BLOCK]), 0u32.wrapping_sub(768));
    let mut body = vec![0u8; 6176];
    put(&mut body, 0, 1); // slot
    fill_save_data(&mut body, &[1, 0, 0, 0].repeat(768), 12, 3072);
    assert_eq!(u32_at(&body, 8 + BLOCK - 4), 1);
    assert_eq!(u32_at(&body, 3080), 12 | (3072 << 16));
    assert_eq!(u32_at(&body, 3084), 768);
    // C2 = slot + block + version/size word + C1, with its own field as 0
    assert_eq!(u32_at(&body, 3088), 1 + 768 + (12 | (3072 << 16)) + 768);
    assert!(body[3092..].iter().all(|b| *b == 0));
}

#[test]
fn put_bits_is_msb_first() {
    let mut b = [0u8; 2780];
    put_bits(&mut b, 0, 32, 12);
    assert_eq!(b[..4], [0, 0, 0, 12]);
    put_bits(&mut b, 22221, 6, 13); // 001101 at bits 22221..22227: byte 2777 bits 5..7, byte 2778 bits 0..2
    assert_eq!((b[2777], b[2778]), (0b0000_0001, 0b1010_0000));
    put_bits(&mut b, 22221, 6, 0);
    assert_eq!((b[2777], b[2778]), (0, 0));
    put_bits(&mut b, 185, 32, u32::MAX); // 1 + 31 bits across bytes 23..27
    assert_eq!(b[23..28], [0x7F, 0xFF, 0xFF, 0xFF, 0x80]);
}

#[test]
fn the_secretaries_are_unlocked() {
    let dir = tempfile::tempdir().unwrap();
    let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
    let (cmd, body, _) = answer(
        237,
        &[0; 20],
        &store,
        None,
        &[],
        None,
        &(Arc::new(|_: &str| {}) as Log),
    )
    .unwrap();
    assert_eq!((cmd, body.len()), (238, 0x5A54));
    assert!(body[SECRETARIES].iter().all(|b| *b == 1));
    // the four secretaries with an id (26..29) and the date and time left 0
    assert!((26..30).all(|n| body[16 + 1176 + n] == 1));
    assert!(body[8..16].iter().all(|b| *b == 0));
}

#[test]
fn tail_spans_stay_inside_and_apart() {
    let mut s = TAIL_SPANS.to_vec();
    s.sort();
    assert!(
        s.iter()
            .all(|&(to, from, len)| to + len <= 6176 && from + len <= 8572)
    );
    assert!(s.windows(2).all(|w| w[0].0 + w[0].2 <= w[1].0));
    let mut s: Vec<_> = TAIL_SPANS
        .iter()
        .map(|&(_, from, len)| (from, len))
        .collect();
    s.sort();
    assert!(s.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0));
}

#[test]
fn an_old_store_gets_its_277_once_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gamesvr.db");
    let (block, tail): (Vec<u8>, Vec<u8>) = (
        (0..BLOCK).map(|i| (i % 251) as u8 + 1).collect(),
        (0..8572 - TAIL).map(|i| (i % 241) as u8 + 3).collect(),
    );
    {
        // a store from before `requests`: user_version 0, a club with a block and a tail
        let db = Connection::open(&path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("DROP TABLE requests").unwrap();
        db.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO saves VALUES (5, 1, ?1, 12, 3072, 2, 1, ?2)",
            params![block, tail],
        )
        .unwrap();
    }
    let log: Log = Arc::new(|_: &str| {});
    let card_in = |store: &Mutex<Store>| {
        answer(275, &req(275, [5, 1, 9]), store, None, &[], None, &log)
            .unwrap()
            .1
    };
    let store = Mutex::new(Store::open(&path).unwrap());
    let a = card_in(&store);
    assert_eq!(u32_at(&a, 4), 0);
    for (to, from, len) in TAIL_SPANS {
        assert_eq!(
            a[to..to + len],
            tail[from - TAIL..from - TAIL + len],
            "span {to}"
        );
    }
    {
        let db = &store.lock().unwrap().0;
        let n = |t: &str| -> i64 {
            db.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!((n("coaches"), n("saves"), n("requests")), (1, 1, 1));
        let kept: Vec<u8> = db
            .query_row("SELECT tail FROM saves", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, tail, "saves.tail untouched");
        let req: Vec<u8> = db
            .query_row("SELECT payload FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!((req.len(), u32_at(&req, 8), req[24]), (8572, 1, 2));
        assert_eq!(&req[28..28 + BLOCK], &block[..]);
        // a later save replaces the rebuilt request; a second open must not bring the old one back
        db.execute("UPDATE requests SET payload = zeroblob(8572)", [])
            .unwrap();
    }
    drop(store);
    let store = Mutex::new(Store::open(&path).unwrap());
    let v: i64 = store
        .lock()
        .unwrap()
        .0
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    let a = card_in(&store);
    assert!(
        a[3100..3556].iter().all(|b| *b == 0),
        "not backfilled twice"
    );
}

#[test]
fn game_ctrl_carries_the_clients_defaults() {
    let mut body = vec![0u8; 0x5A54];
    fill_game_ctrl(&mut body, 0);
    fn at(body: &[u8], off: usize, w: usize) -> u32 {
        let mut b = [0u8; 4];
        b[..w].copy_from_slice(&body[16 + off..16 + off + w]);
        u32::from_le_bytes(b)
    }
    assert!(
        GAME_CTRL_DEFAULTS
            .iter()
            .all(|&(o, w, v)| at(&body, o, w) == v)
    );
    assert_eq!(
        (
            at(&body, 4, 4),
            at(&body, 8, 4),
            at(&body, 19_740, 1),
            at(&body, 1_176, 1)
        ),
        (5_000, 250_000, 26, 1)
    );
    assert!(body[8..16].iter().all(|b| *b == 0), "date and time stay 0");
    // the Universal Cup: scheduling on (payload +1,188 = GAME_CTRL 1172; without it control 0x462820 never counts a
    // cup held and the Universal Championship never follows), 9-minute slots, 20 minutes, and the four open times
    assert_eq!(
        (body[1_188], body[1_189], body[1_190]),
        (1, 9, 20),
        "Universal Cup info"
    );
    assert_eq!(
        [1_092, 1_096, 1_100, 1_104].map(|o| at(&body, o, 4)),
        [1_200, 1_500, 1_800, 2_100]
    );
    // VS overlays the polls and keeps the Grand Trophy block
    vs::fill_game_ctrl(&mut body);
    assert_eq!(
        (
            at(&body, 19_740, 1),
            at(&body, 7_624, 2),
            at(&body, 7_670, 1),
            at(&body, 1_211, 1)
        ),
        (u32::from(vs::POLLS), 2400, 7, 1)
    );
    // no two defaults overlap each other, the secretaries, the week id or the matches per class
    let mut spans: Vec<_> = GAME_CTRL_DEFAULTS
        .iter()
        .map(|&(o, w, _)| (o, o + w))
        .collect();
    spans.extend([(1_176, 1_212), (7_620, 7_622), (7_666, 7_671)]);
    spans.sort();
    assert!(spans.windows(2).all(|w| w[0].1 <= w[1].0));
}

#[test]
fn grand_trophy_block_without_vs() {
    // the arcade's stand-in: no VS switch, so the 238 is the default filler alone
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let mut req = vec![0u8; 16];
    put(&mut req, 0, 1);
    let (body, _) = ask_for_test(&store, 237, &req, &log);
    let at = |off: usize| u16_at(&body, 16 + off);
    assert_eq!((at(7_622), at(7_624)), (0, 2_400), "open all day");
    assert_eq!(body[16 + 7_666..16 + 7_671], [3, 5, 5, 7, 7], "N per class");
    assert_eq!(
        [
            7_626, 7_628, 7_630, 7_632, 7_634, 7_636, 7_638, 7_640, 7_642
        ]
        .map(at),
        [300, 0, 100, 100, 50, 10, 100, 100, 0],
        "win, lose, draw, goal, clean sheet, conceded, hat-trick, streak, streak step"
    );
    assert_eq!((at(7_660), at(7_662), at(7_664)), (4_500, 2_000, 0));
    assert!(
        body[16 + 7_671..16 + 7_736].iter().all(|b| *b == 0),
        "playoff CPU level and the class thresholds"
    );
    assert_eq!(at(7_620), gt_week(now()));
}

#[test]
fn gt_week_rolls_on_sunday_23_italian_time() {
    // Unix seconds of a Rome wall-clock time: winter UTC+1, summer UTC+2
    let utc = |y, mo, d, h, mi| {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    };
    // winter: Sunday 2026-11-08 22:59 / 23:00 CET = 21:59 / 22:00 UTC
    let w = gt_week(utc(2026, 11, 8, 21, 59));
    assert_eq!(gt_week(utc(2026, 11, 8, 22, 0)), w + 1);
    assert_eq!(
        gt_week(utc(2026, 11, 2, 0, 0)),
        w,
        "Monday 01:00 is the same week"
    );
    // summer: Sunday 2026-07-05 22:59 / 23:00 CEST = 20:59 / 21:00 UTC
    let s = gt_week(utc(2026, 7, 5, 20, 59));
    assert_eq!(gt_week(utc(2026, 7, 5, 21, 0)), s + 1);
    assert_eq!(
        w - s,
        18,
        "18 weeks from Monday 2026-06-29 to Monday 2026-11-02"
    );
    // week 0 starts on Sunday 2000-01-02 23:00 CET
    assert_eq!(gt_week(utc(2000, 1, 2, 22, 0)), 0);
    assert_eq!(
        gt_week(utc(2000, 1, 2, 21, 59)),
        0,
        "clamped below the epoch"
    );
    assert_eq!(gt_week(utc(2000, 1, 9, 22, 0)), 1);
}

/// Unix seconds of a UTC wall-clock time.
fn utc_secs(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

#[test]
fn gt_week_rolls_at_23_on_the_dst_sundays() {
    // DST ends on Sunday 2026-10-25 (03:00 CEST -> 02:00 CET): the roll that evening is 23:00 CET = 22:00 UTC
    let w = gt_week(utc_secs(2026, 10, 25, 21, 59));
    assert_eq!(gt_week(utc_secs(2026, 10, 25, 22, 0)), w + 1);
    // the week before rolled at 23:00 CEST = 21:00 UTC, one week earlier
    assert_eq!(gt_week(utc_secs(2026, 10, 18, 20, 59)), w - 1);
    assert_eq!(gt_week(utc_secs(2026, 10, 18, 21, 0)), w);
    // DST starts on Sunday 2027-03-28 (02:00 CET -> 03:00 CEST): the roll is 23:00 CEST = 21:00 UTC
    let (before, after) = (utc_secs(2027, 3, 28, 20, 59), utc_secs(2027, 3, 28, 21, 0));
    assert_eq!(gt_week(after), gt_week(before) + 1);
    assert_eq!(
        gt_week(utc_secs(2027, 3, 21, 22, 0)),
        gt_week(before),
        "rolled at 23:00 CET the week before"
    );
    // a 277 saved a minute before that roll is reset at a card-in a minute after it; one saved at the roll is not
    let mut body = vec![0xAAu8; 6176];
    fill_gt_week(&mut body, gt_week(after), gt_week(after), 0);
    assert!(body[3192..3200].iter().all(|b| *b == 0xAA));
    fill_gt_week(&mut body, gt_week(before), gt_week(after), 0);
    assert!(body[3192..3200].iter().all(|b| *b == 0));
    // a save stamped in a later week resets too
    body.fill(0xAA);
    fill_gt_week(&mut body, gt_week(after), gt_week(before), 0);
    assert!(body[3192..3200].iter().all(|b| *b == 0));
}

/// A play carded in before the Sunday 23:00 roll and saved after it files its record under the card-in week and
/// the club's next card-in starts from zero; a store error on `gt_points` neither refuses the save nor the
/// card-in.
#[test]
fn a_play_across_the_roll_keeps_its_card_in_week() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
    let ask = |cmd: u16, p: &[u8]| {
        let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
        assert!(why.is_none(), "{cmd}: {why:?}");
        body
    };
    let b = ask(279, &fixture(279, [5, 0, 0]));
    let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
    ask(277, &fixture(277, [5, club, card]));
    ask(275, &req(275, [5, club, card]));
    let week = gt_week(now());
    // the card-in notes its week, unfiled
    assert_eq!(
        store.lock().unwrap().1.get(&(5, club)),
        Some(&(week, false))
    );
    store.lock().unwrap().1.insert((5, club), (week - 1, false));
    let mut r = fixture(277, [5, club, card]);
    r[5564..5708].fill(0x11);
    put(&mut r, GT_RECORD_277, 800);
    // a save across the roll whose record is not written stays unfiled, and the next save files it
    store
        .lock()
        .unwrap()
        .0
        .execute_batch("ALTER TABLE gt_points RENAME TO gone")
        .unwrap();
    ask(277, &r);
    assert_eq!(
        store.lock().unwrap().1.get(&(5, club)),
        Some(&(week - 1, false))
    );
    store
        .lock()
        .unwrap()
        .0
        .execute_batch("ALTER TABLE gone RENAME TO gt_points")
        .unwrap();
    ask(277, &r);
    assert_eq!(
        store.lock().unwrap().1.get(&(5, club)),
        Some(&(week - 1, true))
    );
    assert_eq!(
        store.lock().unwrap().gt_ranking(week - 1).unwrap(),
        [(5, 800)]
    );
    assert!(store.lock().unwrap().gt_ranking(week).unwrap().is_empty());
    // a later save of the same session leaves the closed week's row and the live week alone
    let at_w = |s: &Store| -> i64 {
        s.0.query_row(
            "SELECT at FROM gt_points WHERE week = ?1",
            [week - 1],
            |r| r.get(0),
        )
        .unwrap()
    };
    let first_at = at_w(&store.lock().unwrap());
    put(&mut r, GT_RECORD_277, 900);
    ask(277, &r);
    assert_eq!(
        store.lock().unwrap().gt_ranking(week - 1).unwrap(),
        [(5, 800)]
    );
    assert_eq!(at_w(&store.lock().unwrap()), first_at);
    assert!(store.lock().unwrap().gt_ranking(week).unwrap().is_empty());
    let a = ask(275, &req(275, [5, club, card]));
    for (at, len) in GT_WEEKLY_276 {
        assert!(a[at..at + len].iter().all(|b| *b == 0), "+{at} reset");
    }

    store
        .lock()
        .unwrap()
        .0
        .execute_batch("DROP TABLE gt_points")
        .unwrap();
    ask(277, &r);
    assert_eq!(
        u32_at(&ask(275, &req(275, [5, club, card])), GT_RANK_276),
        0
    );
}

/// The shared ranking on a server with VS: two cards' records in one week come back as ranks 1 and 2 at
/// card-in, in the exact 276 bytes the client reads; a stored save from an earlier week loses its weekly
/// fields; ties are deterministic; an arcade's stand-in (no VS) keeps none of it.
#[test]
fn gt_ranking_orders_records_and_fills_the_276() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let ip = IpAddr::from([127, 0, 0, 1]);
    let ask = |cmd: u16, p: &[u8], vs: Option<(&Vs, IpAddr)>| {
        let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
        assert!(why.is_none(), "{cmd}: {why:?}");
        body
    };
    // the 277's Grand Trophy fields, +5,564..+5,708, every byte non-zero; the record (+5,572) set per club
    let save = |aime: u32, record: u32| {
        let b = ask(279, &fixture(279, [aime, 0, 0]), Some((&vs, ip)));
        let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
        let mut r = fixture(277, [aime, club, card]);
        for (i, b) in r[5564..5708].iter_mut().enumerate() {
            *b = i as u8 | 0x80;
        }
        put(&mut r, GT_RECORD_277, record);
        ask(277, &r, Some((&vs, ip)));
        (club, card, r)
    };
    let (c5, k5, r5) = save(5, 900);
    let (c6, k6, _) = save(6, 1_200);
    let week = gt_week(now());
    assert_eq!(
        store.lock().unwrap().gt_ranking(week).unwrap(),
        [(6, 1_200), (5, 900)]
    );
    // another club of card 5 (or the same club, imported again under a new coachId) with a lower record: the
    // card keeps its best
    save(5, 300);
    assert_eq!(
        store.lock().unwrap().gt_ranking(week).unwrap(),
        [(6, 1_200), (5, 900)]
    );
    let card_in =
        |aime: u32, club: u32, card: u32| ask(275, &req(275, [aime, club, card]), Some((&vs, ip)));
    assert_eq!(u32_at(&card_in(6, c6, k6), GT_RANK_276), 1);
    let a = card_in(5, c5, k5);
    // exact bytes: 276 +3,184..+3,328 = the 277's +5,564..+5,708, but the rank (s32 +3,212) is this server's
    let mut want = r5[5564..5708].to_vec();
    want[GT_RANK_276 - 3184..GT_RANK_276 - 3180].copy_from_slice(&2u32.to_le_bytes());
    // and so are the result fields: no promotion was issued, so final rank, next class and play-off flag are 0
    let zero_result = |v: &mut [u8]| {
        v[GT_FINAL_RANK_276 - 3184..GT_FINAL_RANK_276 - 3180].fill(0);
        v[GT_NEXT_CLASS_276 - 3184] = 0;
        v[GT_PLAYOFF_276 - 3184] = 0;
    };
    zero_result(&mut want);
    assert_eq!(a[3184..3328], want[..]);
    assert_eq!(
        u32_at(&a, 3192),
        900,
        "the record survives a card-in in its week"
    );

    // the stored 277 (and its record) is from last week: the weekly fields are zero, the rest is kept
    {
        let db = &store.lock().unwrap().0;
        let last = now() - 8 * 86_400;
        db.execute("UPDATE requests SET updated = ?1 WHERE aime_id = 5", [last])
            .unwrap();
        db.execute(
            "UPDATE gt_points SET week = ?1 WHERE aime_id = 5",
            [gt_week(last)],
        )
        .unwrap();
    }
    let a = card_in(5, c5, k5);
    for (at, len) in GT_WEEKLY_276 {
        assert!(a[at..at + len].iter().all(|b| *b == 0), "+{at} reset");
    }
    let mut kept = r5[5564..5708].to_vec();
    for (at, len) in GT_WEEKLY_276 {
        kept[at - 3184..at - 3184 + len].fill(0);
    }
    zero_result(&mut kept);
    assert_eq!(a[3184..3328], kept[..], "shop, time, card, class... kept");
    assert_eq!(u32_at(&a, 3088), {
        let mut z = a.clone();
        put(&mut z, 3088, 0);
        sum32(&z)
    });

    // ties: the record set first, then the lower Aime id
    {
        let st = store.lock().unwrap();
        let mut r = vec![0u8; 8572];
        put(&mut r, GT_RECORD_277, 500);
        for (aime, at) in [(3, 50), (2, 60), (1, 60)] {
            record_gt(&st.0, 1, aime, &r, at).unwrap();
        }
        // the same record again keeps its time
        record_gt(&st.0, 1, 3, &r, 99).unwrap();
        // a record above the bound, or one that is negative as an s32, is not kept
        for forged in [GT_RECORD_MAX + 1, 0x8000_0000] {
            put(&mut r, GT_RECORD_277, forged);
            record_gt(&st.0, 1, 4, &r, 10).unwrap();
        }
        let order: Vec<_> = st.gt_ranking(1).unwrap().iter().map(|e| e.0).collect();
        assert_eq!(order, [3, 1, 2]);
    }

    // no VS (an arcade's stand-in): the stored rank bytes come back as saved and nothing is recorded
    let (_, b, _) = answer(279, &fixture(279, [7, 0, 0]), &store, None, &[], None, &log).unwrap();
    let (c7, k7) = (u32_at(&b, 8), u32_at(&b, 12));
    let mut r = fixture(277, [7, c7, k7]);
    put(&mut r, GT_RECORD_277, 4_000);
    put(&mut r, GT_RANK_276 + 2380, 33);
    answer(277, &r, &store, None, &[], None, &log).unwrap();
    let a = answer(275, &req(275, [7, c7, k7]), &store, None, &[], None, &log)
        .unwrap()
        .1;
    assert_eq!(u32_at(&a, GT_RANK_276), 33);
    let n: i64 = store
        .lock()
        .unwrap()
        .0
        .query_row(
            "SELECT count(*) FROM gt_points WHERE aime_id = 7",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}

/// A version-2 store (before `gt_points`) keeps every row and gets each stored 277's record under the week it
/// was saved in.
#[test]
fn a_v2_store_gets_gt_points_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gamesvr.db");
    let t = utc_secs(2026, 11, 4, 18, 0);
    let mut r = fixture(277, [5, 1, 9]);
    put(&mut r, GT_RECORD_277, 700);
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("DROP TABLE gt_points; PRAGMA user_version = 2")
            .unwrap();
        db.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO saves VALUES (5, 1, zeroblob(3072), 12, 3072, 2, ?1, ?2)",
            params![t, &r[TAIL..]],
        )
        .unwrap();
        // the same record from a later session, inserted first so the scan (rowid order) meets it first: the
        // earlier session keeps the tie-break only through ORDER BY updated
        db.execute(
            "INSERT INTO requests VALUES (5, 0, 277, ?1, ?2)",
            params![r, t + 3_600],
        )
        .unwrap();
        db.execute(
            "INSERT INTO requests VALUES (5, 1, 277, ?1, ?2)",
            params![r, t],
        )
        .unwrap();
    }
    let st = Store::open(&path).unwrap();
    let n = |q: &str| -> i64 { st.0.query_row(q, [], |r| r.get(0)).unwrap() };
    assert_eq!(n("PRAGMA user_version"), SCHEMA_VERSION);
    assert_eq!(
        [
            n("SELECT count(*) FROM coaches"),
            n("SELECT count(*) FROM saves"),
            n("SELECT count(*) FROM requests")
        ],
        [1, 1, 2]
    );
    let kept: Vec<u8> =
        st.0.query_row("SELECT payload FROM requests WHERE coach_id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, r);
    assert_eq!(st.gt_ranking(gt_week(t)).unwrap(), [(5, 700)]);
    let at: i64 =
        st.0.query_row("SELECT at FROM gt_points", [], |r| r.get(0))
            .unwrap();
    assert_eq!(at, t);
}

/// Promotion at card-in on a server with VS: the card's record of the closed week against the threshold of the
/// club's saved class (277 +5,609), one class up, once per week and card, never past Area A; every other card-in
/// sends no result, whatever the stored 277 carries.
#[test]
fn a_closed_week_record_promotes_once_and_never_past_area_a() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let log: Log = {
        let lines = lines.clone();
        Arc::new(move |l: &str| lines.lock().unwrap().push(l.to_owned()))
    };
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
    let ask = |cmd: u16, p: &[u8]| {
        let (_, body, why) = answer(cmd, p, &store, None, &[], vs, &log).unwrap();
        assert!(why.is_none(), "{cmd}: {why:?}");
        body
    };
    let week = gt_week(now());
    // a save of club (club, card) of `aime` in `class`, carrying a stale result (rank 7, next Local, play-off 1)
    let save = |aime: u32, (club, card): (u32, u32), class: u8| {
        let mut r = fixture(277, [aime, club, card]);
        r[GT_CLASS_277] = class;
        put(&mut r, GT_FINAL_RANK_276 + 2380, 7);
        r[GT_PLAYOFF_276 + 2380] = 1;
        ask(277, &r);
    };
    // a new club of `aime` saved in `class`
    let club = |aime: u32, class: u8| {
        let b = ask(279, &fixture(279, [aime, 0, 0]));
        let k = (u32_at(&b, 8), u32_at(&b, 12));
        save(aime, k, class);
        k
    };
    let last_week = |aime: u32, record: u32| {
        let mut r = vec![0u8; 8572];
        put(&mut r, GT_RECORD_277, record);
        record_gt(&store.lock().unwrap().0, week - 1, aime, &r, 1).unwrap();
    };
    // (final rank, next class, play-off flag) of a card-in
    let result = |aime: u32, (club, card): (u32, u32)| {
        let a = ask(275, &req(275, [aime, club, card]));
        assert_eq!(u32_at(&a, 3088), {
            let mut z = a.clone();
            put(&mut z, 3088, 0);
            sum32(&z)
        });
        (
            u32_at(&a, GT_FINAL_RANK_276),
            a[GT_NEXT_CLASS_276],
            a[GT_PLAYOFF_276],
        )
    };
    last_week(9, 3_000);

    let local = club(5, 0);
    last_week(5, 999);
    assert_eq!(result(5, local), (0, 0, 0), "999 stays Local");
    last_week(5, 1_000);
    assert_eq!(
        result(5, local),
        (2, 1, 0),
        "1,000: Area B, rank 2 behind card 9"
    );
    // the save still shows Local (the 276 was lost, or the session ended before the info scene): the same again
    assert_eq!(result(5, local), (2, 1, 0), "re-issued until applied");
    // the closed week's row pruned: the stored rank again, never a made-up first place
    store
        .lock()
        .unwrap()
        .0
        .execute(
            "DELETE FROM gt_points WHERE week = ?1 AND aime_id = 5",
            [week - 1],
        )
        .unwrap();
    assert_eq!(
        result(5, local),
        (2, 1, 0),
        "stored rank, no closed-week row"
    );
    // the card's other club carded in later the same week: the card's promotion is spent
    let other = club(5, 0);
    assert_eq!(result(5, other), (0, 0, 0));
    save(5, local, 1);
    assert_eq!(result(5, local), (0, 0, 0), "applied: once per week");

    let area_b = club(6, 1);
    last_week(6, 1_199);
    assert_eq!(result(6, area_b), (0, 0, 0), "1,199 stays Area B");
    last_week(6, 1_200);
    assert_eq!(
        result(6, area_b),
        (2, 2, 0),
        "1,200: Area A, rank 2 behind card 9"
    );

    // one card, two clubs: the class is the carding club's; an Area A club carding in first spends nothing
    let area_a = club(13, 2);
    let local13 = club(13, 0);
    last_week(13, 1_100);
    assert_eq!(result(13, area_a), (0, 0, 0), "Area A is the cap");
    assert_eq!(result(13, local13), (3, 1, 0), "Local -> Area B, rank 3");
    assert_eq!(result(13, area_a), (0, 0, 0), "not to the other club");

    // a guest's club, imported again at its next join (new coach and card ids): the promotion follows it
    let guest = club(20, 0);
    last_week(20, 1_000);
    assert_eq!(result(20, guest), (4, 1, 0), "Local -> Area B, rank 4");
    store
        .lock()
        .unwrap()
        .0
        .execute("INSERT INTO guests VALUES (20, 't', 1, 0)", [])
        .unwrap();
    let rejoin = || {
        let mut st = store.lock().unwrap();
        let c = st.club(20).unwrap();
        let link = st.import_public(&c, 20, 1, "t", &|_| false).unwrap();
        (link.coaches[0].1, link.cards[0].1)
    };
    let again = rejoin();
    assert_ne!(again, guest);
    assert_eq!(result(20, again), (4, 1, 0), "re-imported, not applied");
    let again = rejoin();
    assert_eq!(result(20, again), (4, 1, 0), "each join of the week");
    save(20, again, 1);
    let applied = rejoin();
    assert_eq!(result(20, applied), (0, 0, 0), "applied");

    for (aime, class) in [(7, 2), (8, 3), (10, 4), (11, 200)] {
        let k = club(aime, class);
        last_week(aime, 5_000);
        assert_eq!(result(aime, k), (0, 0, 0), "class {class}: no promotion");
    }
    // a record of this week counts next week, not now
    let fresh = club(12, 0);
    let mut r = vec![0u8; 8572];
    put(&mut r, GT_RECORD_277, 5_000);
    record_gt(&store.lock().unwrap().0, week, 12, &r, 1).unwrap();
    assert_eq!(result(12, fresh), (0, 0, 0));

    let rows: Vec<(u32, u32, u8)> = store
        .lock()
        .unwrap()
        .0
        .prepare(
            "SELECT aime_id, coach_id, class FROM gt_promotions WHERE week = ?1 ORDER BY aime_id",
        )
        .unwrap()
        .query_map([week], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            (5, local.0, 1),
            (6, area_b.0, 2),
            (13, local13.0, 1),
            (20, again.0, 1)
        ]
    );
    let lines = lines.lock().unwrap();
    let want = format!(
        "grand trophy: aime 5 coach {} week {week} class 0 -> 1 (record 1000, rank 2)",
        local.0
    );
    assert!(lines.contains(&want), "{want}");

    // never a next class below the class or above Area A, whatever the caller passes
    let mut b = vec![0xFFu8; 6176];
    for (promote, want) in [
        (Some((0, 0)), (1, 1)),
        (Some((1, 4)), (4, 2)),
        (Some((2, 1)), (0, 0)),
        (Some((4, 1)), (0, 0)),
        (None, (0, 0)),
    ] {
        fill_gt_result(&mut b, promote);
        assert_eq!(
            (
                u32_at(&b, GT_FINAL_RANK_276),
                b[GT_NEXT_CLASS_276],
                b[GT_PLAYOFF_276]
            ),
            (want.0, want.1, 0),
            "{promote:?}"
        );
    }
    assert!(GT_RAISE[2..].iter().all(|&p| p == 0), "Area A is the cap");
}

/// A club granted Area B last week whose save fell back to Local: its class is the grant, also after a re-import
/// (a public server's clubs get new ids each session), so this week's record promotes it Area B -> Area A, each
/// card-in of the week sends that grant, and a record below that threshold re-sends last week's grant once, with
/// a log line that is no promotion, writing no grant; never Local -> Area B.
#[test]
fn an_earlier_weeks_grant_is_the_floor_of_the_saved_class() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let log: Log = {
        let lines = lines.clone();
        Arc::new(move |l: &str| lines.lock().unwrap().push(l.to_owned()))
    };
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let vs = Some((&vs, IpAddr::from([127, 0, 0, 1])));
    let ask = |cmd: u16, p: &[u8]| answer(cmd, p, &store, None, &[], vs, &log).unwrap().1;
    let week = gt_week(now());
    let record = |w: u16, aime: u32, points: u32| {
        let mut r = vec![0u8; 8572];
        put(&mut r, GT_RECORD_277, points);
        record_gt(&store.lock().unwrap().0, w, aime, &r, 1).unwrap();
    };
    // a Local club of `aime`, granted Area B at a card-in of last week, saved Local again
    let demoted = |aime: u32| {
        let b = ask(279, &fixture(279, [aime, 0, 0]));
        let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
        let mut r = fixture(277, [aime, club, card]);
        r[GT_CLASS_277] = 0;
        ask(277, &r);
        record(week - 2, aime, 1_000);
        let local = vec![0u8; 8572];
        let granted = store.lock().unwrap().promote(week - 1, aime, club, &local);
        assert_eq!(
            granted.unwrap().map(|g| g.0),
            Some(0),
            "last week: Local -> Area B"
        );
        (club, card)
    };
    let result = |aime: u32, (club, card): (u32, u32)| {
        let a = ask(275, &req(275, [aime, club, card]));
        (u32_at(&a, GT_FINAL_RANK_276), a[GT_NEXT_CLASS_276])
    };

    // the club imported again (a guest's next join, online::club): new coach and card ids, the old coach gone
    let rejoin = |aime: u32| {
        let mut st = store.lock().unwrap();
        st.0.execute("INSERT INTO guests VALUES (?1, 't', 1, 0)", [aime])
            .unwrap();
        let c = st.club(aime).unwrap();
        let link = st.import_public(&c, aime, 1, "t", &|_| false).unwrap();
        let ids = link.coaches.iter().zip(&link.cards);
        ids.map(|(c, k)| (c.1, k.1)).collect::<Vec<_>>()
    };

    let old = demoted(5);
    let k = rejoin(5)[0];
    assert_ne!(k, old);
    record(week - 1, 5, 1_300);
    assert_eq!(
        result(5, k),
        (1, 2),
        "re-imported: Area B -> Area A, not Local -> Area B"
    );
    assert_eq!(
        result(5, k),
        (1, 2),
        "the week's next card-in, save still Local: the grant again, never Area B"
    );

    let k = demoted(6);
    record(week - 1, 6, 500);
    assert_eq!(
        result(6, k),
        (2, 1),
        "no promotion this week: last week's grant (rank 2) again"
    );
    assert_eq!(
        result(6, k),
        (0, 0),
        "the save still below: once per grant, not at every card-in (#334)"
    );
    let want = format!(
        "grand trophy: aime 6 coach {} week {week} save below the week {} grant of class 1: its result sent \
         again, no new promotion (record 1000, rank 2)",
        k.0,
        week - 1
    );
    let said = lines.lock().unwrap().clone();
    assert!(said.contains(&want), "{want}");
    assert!(
        !said
            .iter()
            .any(|l| l.starts_with("grand trophy: aime 6 ") && l.contains("->")),
        "a re-issue never reads as a promotion: {said:?}"
    );
    let rows: i64 = store
        .lock()
        .unwrap()
        .0
        .query_row(
            "SELECT count(*) FROM gt_promotions WHERE week = ?1 AND aime_id = 6",
            [week],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "a re-sent grant spends nothing this week");

    // a second club of the card made in the same second as the granted one: after a re-import (both old coaches
    // gone) the floor stays with the granted club, never the other (#334)
    let first = demoted(7);
    let b = ask(279, &fixture(279, [7, 0, 0]));
    let second = (u32_at(&b, 8), u32_at(&b, 12));
    let mut r = fixture(277, [7, second.0, second.1]);
    r[GT_CLASS_277] = 0;
    ask(277, &r);
    record(week - 1, 7, 500);
    let ks = rejoin(7);
    assert!(!ks.contains(&first) && !ks.contains(&second));
    assert_eq!(
        result(7, ks[1]),
        (0, 0),
        "the other club: no floor, no result"
    );
    assert_eq!(
        result(7, ks[0]),
        (3, 1),
        "the granted club: its grant (rank 3)"
    );
}

/// A server with VS sends the promotion thresholds as GAME_CTRL raisePoint; an arcade's stand-in sends none.
#[test]
fn game_ctrl_carries_the_promotion_thresholds_with_vs() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let mut req = vec![0u8; 16];
    put(&mut req, 0, 1);
    let raise = |vs| {
        let body = answer(237, &req, &store, None, &[], vs, &log).unwrap().1;
        [0, 1, 2, 3, 4].map(|c| u32_at(&body, 16 + 7_676 + 4 * c))
    };
    assert_eq!(
        raise(Some((&vs, IpAddr::from([127, 0, 0, 1])))),
        [1_000, 1_200, 0, 0, 0]
    );
    assert_eq!(raise(None), [0; 5]);
}

/// A version-3 store (before `gt_promotions`) keeps every row, gets the table (from [`SCHEMA`], as `purchases`
/// and `gt_points` did) and the current version, and its `gt_points` is not filled again from the stored 277s; a
/// second open changes nothing.
#[test]
fn a_v3_store_gets_gt_promotions_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gamesvr.db");
    let mut r = fixture(277, [5, 1, 9]);
    put(&mut r, GT_RECORD_277, 700);
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("DROP TABLE gt_promotions; PRAGMA user_version = 3")
            .unwrap();
        db.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 1, 1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO saves VALUES (5, 1, zeroblob(3072), 12, 3072, 2, 1, ?1)",
            [&r[TAIL..]],
        )
        .unwrap();
        db.execute("INSERT INTO requests VALUES (5, 1, 277, ?1, 1)", [&r])
            .unwrap();
        db.execute("INSERT INTO gt_points VALUES (40, 5, 1234, 77)", [])
            .unwrap();
    }
    for open in 0..2 {
        let st = Store::open(&path).unwrap();
        let n = |q: &str| -> i64 { st.0.query_row(q, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("PRAGMA user_version"), SCHEMA_VERSION, "open {open}");
        assert_eq!(
            [
                n("SELECT count(*) FROM coaches"),
                n("SELECT count(*) FROM saves"),
                n("SELECT count(*) FROM requests"),
                n("SELECT count(*) FROM gt_promotions")
            ],
            [1, 1, 1, open],
            "open {open}"
        );
        let kept: Vec<u8> =
            st.0.query_row("SELECT payload FROM requests", [], |r| r.get(0))
                .unwrap();
        assert_eq!(kept, r);
        assert_eq!(n("SELECT count(*) FROM gt_points"), 1, "no second backfill");
        assert_eq!(st.gt_ranking(40).unwrap(), [(5, 1234)]);
        assert_eq!(n("SELECT at FROM gt_points"), 77);
        // the table takes a promotion, and the row outlives the reopen
        st.0.execute(
            "INSERT OR IGNORE INTO gt_promotions VALUES (41, 5, 1, 1, 78, 1, 1234, 70, 0)",
            [],
        )
        .unwrap();
    }
}

/// A version-4 store (`gt_promotions` without `resent`) keeps its grants, and a stored grant is still the floor
/// of its club: a save below it gets the grant's result once, then none (#334).
#[test]
fn a_v4_store_keeps_its_grand_trophy_floors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gamesvr.db");
    let week = gt_week(now());
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("ALTER TABLE gt_promotions DROP COLUMN resent; PRAGMA user_version = 4")
            .unwrap();
        db.execute(
            "INSERT INTO coaches (aime_id, coach_id, card_id, created, updated) VALUES (5, 1, 9, 70, 70)",
            [],
        )
        .unwrap();
        // Area B granted to club 1 two weeks ago
        db.execute(
            "INSERT INTO gt_promotions VALUES (?1, 5, 1, 1, 78, 2, 1234, 70)",
            [week - 2],
        )
        .unwrap();
    }
    let st = Store::open(&path).unwrap();
    let n = |q: &str| -> i64 { st.0.query_row(q, [], |r| r.get(0)).unwrap() };
    assert_eq!(n("PRAGMA user_version"), SCHEMA_VERSION);
    assert_eq!(n("SELECT count(*) FROM gt_promotions WHERE resent = 0"), 1);
    let local = vec![0u8; 8572];
    assert_eq!(
        st.promote(week, 5, 1, &local).unwrap(),
        Some((0, 2, 1234, Some(week - 2))),
        "the stored grant is the floor: its result again"
    );
    assert_eq!(st.promote(week, 5, 1, &local).unwrap(), None, "once");
    drop(st);
    let st = Store::open(&path).unwrap();
    assert_eq!(
        st.promote(week, 5, 1, &local).unwrap(),
        None,
        "a reopen keeps the mark"
    );
}

#[test]
fn vs_echoes_the_entry_user_and_the_data_battle() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let ip = IpAddr::from([127, 0, 0, 1]);
    let ask = |cmd: u16, ids: [u32; 3]| {
        let mut p = vec![0u8; 16];
        put(&mut p, 0, 1);
        for (i, v) in ids.into_iter().enumerate() {
            put(&mut p, 4 + 4 * i, v);
        }
        answer(cmd, &p, &store, None, &[], Some((&vs, ip)), &log)
            .unwrap()
            .1
    };
    // 239: cardId, coachId back; 257: coachId back and status 1
    let b = ask(239, [7, 9, 0]);
    assert_eq!((u32_at(&b, 4), u32_at(&b, 8)), (7, 9));
    let b = ask(257, [9, 7, 0]);
    assert_eq!((u32_at(&b, 4), u32_at(&b, 8)), (9, 1));
    // 0 ids stay zero: the control's SvrErr
    assert!(ask(239, [0, 0, 0])[4..].iter().all(|b| *b == 0));
    assert!(ask(257, [0, 5, 0])[4..].iter().all(|b| *b == 0));
    // no VS: zeros as before
    let b = answer(
        239,
        &[1, 0, 0, 0, 7, 0, 0, 0, 9, 0, 0, 0],
        &store,
        None,
        &[],
        None,
        &log,
    )
    .unwrap()
    .1;
    assert!(b[4..].iter().all(|b| *b == 0));
}

/// A request for the guard test: slot 1, ids at +4/+8/+12, the revision where the club messages carry it.
fn fixture(cmd: u16, ids: [u32; 3]) -> Vec<u8> {
    let (len, rev) = match cmd {
        271 => (20, 16),
        275 => (32, 28),
        277 => (8572, 5940),
        279 => (36, 32),
        285 => (24, 20),
        287 | 289 => (28, 24),
        309 => (3368, 0),
        315 => (24, 0),
        _ => (600, 0),
    };
    let mut r = vec![0u8; len];
    put(&mut r, 0, 1);
    for (i, id) in ids.into_iter().enumerate() {
        put(&mut r, 4 + 4 * i, id);
    }
    if rev != 0 {
        put(&mut r, rev, REVISION);
    }
    match cmd {
        277 => {
            r[24] = 2;
            r[3100..3104].copy_from_slice(&[12, 0, 0, 12]);
        }
        279 => r[12] = 1,
        309 => {
            r[8..3080].fill(0x5A);
            for bit in BLOCK_VERSIONS {
                put_bits(&mut r[8..8 + BLOCK], bit, 32, 12);
            }
        }
        289 => put(&mut r, 16, 1),
        _ => {}
    }
    r
}

/// Every [`RESP`] answer is filled, or all zero with a parser address that proves zero is safe.
#[test]
fn zero_answers_are_handled_or_safe() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let vs = Vs {
        cfg: VsConfig {
            public_ip: std::net::Ipv4Addr::LOCALHOST,
            reflect_port: 1,
            vslink_port: 2,
        },
        matcher: Arc::default(),
    };
    let ip = IpAddr::from([127, 0, 0, 1]);
    let ask = |cmd: u16, p: &[u8]| answer(cmd, p, &store, None, &[], Some((&vs, ip)), &log);
    let aime = 5;
    // a saved club, so the club messages have ids to work on
    let (_, b, _) = ask(279, &fixture(279, [aime, 0, 0])).unwrap();
    let (coach, card) = (u32_at(&b, 8), u32_at(&b, 12));
    let ids = [aime, coach, card];
    assert_eq!(u32_at(&ask(277, &fixture(277, ids)).unwrap().1, 4), 0);
    // a purchase (category = the coach id, item = the card id: Uniform 1), so 286 has something to say
    assert_eq!(u32_at(&ask(315, &fixture(315, ids)).unwrap().1, 4), 0);
    // two paired VS entries (243 -> 245), so the vslink lookup 904 and the match answer 246 have something to say
    let entry = |card: u32, seat: u32, port: u16| {
        let mut e = fixture(243, [0, 0, 0]);
        e.truncate(280);
        put(&mut e, 8, 0x0100_007F);
        e[12..14].copy_from_slice(&port.to_le_bytes());
        put(&mut e, 44, card);
        put(&mut e, 48, seat);
        e
    };
    let poll = |card: u32, seat: u32| {
        let mut p = vec![0u8; 20];
        put(&mut p, 0, 1);
        put(&mut p, 8, card);
        put(&mut p, 12, seat);
        p
    };
    ask(243, &entry(10, 1, 1111)).unwrap();
    ask(243, &entry(11, 2, 2222)).unwrap();
    assert!(
        ask(245, &poll(10, 1)).unwrap().1[4..]
            .iter()
            .any(|b| *b != 0)
    );
    let mut lookup = vec![0u8; 8];
    put(&mut lookup, 0, 0x0100_007F);
    lookup[4..6].copy_from_slice(&1111u16.to_le_bytes());

    for (rcmd, _, zero) in RESP {
        let cmd = rcmd - 1;
        let payload = match cmd {
            245 => poll(10, 1),
            904 => lookup.clone(),
            _ => fixture(cmd, ids),
        };
        let (_, body, refused) = ask(cmd, &payload).unwrap();
        assert_eq!(refused, None, "{rcmd}: the fixture is refused");
        let is_zero = body[4..].iter().all(|b| *b == 0);
        match zero {
            Zero::Handled => assert!(!is_zero, "{rcmd} is all zero and not marked Safe"),
            Zero::Safe(why) => {
                assert!(
                    why.contains("sub_") || why.contains("0x"),
                    "{rcmd}: Safe names no address: {why}"
                );
                assert!(
                    is_zero,
                    "{rcmd} is marked Safe but the answer is not zero (stale entry)"
                );
            }
        }
    }
}

/// 2026-11-06: a Universal Cup winner's saved championship entry date comes back as 0 in the 276, so MATCH
/// SELECT offers the cup again at the first card-in; the bits around it and both sums stay right.
#[test]
fn the_save_answer_clears_the_championship_entry() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let b = ask_store(&store, 279, &fixture(279, [5, 0, 0])).0;
    let (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
    let mut save = fixture(277, [5, club, card]);
    save[28..28 + BLOCK].fill(0xFF);
    // block bytes 2,883..2,886 of the 11-01 winner's save: 01 35 28 ED
    put_bits(&mut save[28..], UC_ENTRY_DATE_BIT - 7, 32, 20_261_101);
    assert_eq!(&save[28 + 2883..28 + 2887], &[0x01, 0x35, 0x28, 0xED]);
    let sum = sum32(&save[28..28 + BLOCK]);
    put(&mut save, 5236, sum);
    assert_eq!(u32_at(&ask_store(&store, 277, &save).0, 4), 0);
    let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
    let block = &a[8..8 + BLOCK];
    assert_eq!(get_bits(block, UC_ENTRY_DATE_BIT, 25), 0);
    let mut want = save[28..28 + BLOCK].to_vec();
    put_bits(&mut want, UC_ENTRY_DATE_BIT, 25, 0);
    assert_eq!(block, &want[..], "only the date changes");
    assert_eq!(u32_at(&a, 3084), sum32(block), "C1");
    let mut z = a.clone();
    put(&mut z, 3088, 0);
    assert_eq!(u32_at(&a, 3088), sum32(&z), "C2");
}

/// #445: a 276 sent 0 as the net FK/CK/PK ids and the client cleared every club's kickers at card-in; it now sends
/// the client's "no setting" id, and the save block that holds the kickers goes out unchanged.
#[test]
fn the_save_answer_keeps_the_kickers() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let [aime, club, card] = new_club(&store);
    let mut save = fixture(277, [aime, club, card]);
    save[28..28 + BLOCK].fill(0x5A);
    // the 276 clears this date, so leave it 0 to compare the block whole
    put_bits(&mut save[28..], UC_ENTRY_DATE_BIT, 25, 0);
    let sum = sum32(&save[28..28 + BLOCK]);
    put(&mut save, 5236, sum);
    assert_eq!(u32_at(&ask_store(&store, 277, &save).0, 4), 0);
    let a = ask_store(&store, 275, &req(275, [aime, club, card])).0;
    for at in NET_KICKERS_276 {
        assert_eq!(u16::from_le_bytes([a[at], a[at + 1]]), 16_383, "id at {at}");
    }
    assert_eq!(
        &a[6086..6088],
        &[0, 0],
        "captain id stays 0: the client skips it"
    );
    assert_eq!(&a[8..8 + BLOCK], &save[28..28 + BLOCK], "block");
    let mut z = a.clone();
    put(&mut z, 3088, 0);
    assert_eq!(u32_at(&a, 3088), sum32(&z), "C2 covers the ids");
}

/// A card-in answer after a 277 holding `set` (277 offset, bytes); `None`: a club with no stored 277 (a save from
/// before the `requests` table).
fn card_in_after(set: Option<&[(usize, &[u8])]>) -> Vec<u8> {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let [aime, club, card] = new_club(&store);
    let mut save = fixture(277, [aime, club, card]);
    for &(at, v) in set.unwrap_or_default() {
        save[at..at + v.len()].copy_from_slice(v);
    }
    assert_eq!(u32_at(&ask_store(&store, 277, &save).0, 4), 0);
    if set.is_none() {
        let st = store.lock().unwrap();
        st.0.execute_batch("DELETE FROM requests").unwrap();
    }
    let (a, why) = ask_store(&store, 275, &req(275, [aime, club, card]));
    assert_eq!((a.len(), u32_at(&a, 4), why), (6176, 0, None));
    let mut z = a.clone();
    put(&mut z, 3088, 0);
    assert_eq!(u32_at(&a, 3088), sum32(&z), "C2 covers the restored tail");
    a
}

/// #449: the chosen title (u16), training wear and flag (u16) at 276 +6,016 come from the 277's +8,352; 0 cleared
/// them at every card-in.
#[test]
fn the_save_answer_keeps_title_wear_and_flag() {
    let a = card_in_after(Some(&[(8352, &[162, 1, 7, 0, 33, 1])]));
    assert_eq!(&a[6016..6022], &[162, 1, 7, 0, 33, 1]);
}

/// #449: the coach's 50 items (276 +3,624) come from the 277's +6,005; 0 emptied the list at every card-in.
#[test]
fn the_save_answer_keeps_the_coach_items() {
    let items: Vec<u8> = (1..=50).collect();
    let a = card_in_after(Some(&[(6005, &items)]));
    assert_eq!(&a[3624..3674], &items[..]);
    assert_eq!(
        &a[3616..3624],
        &[0; 8],
        "the Universal Cup rank and points stay 0"
    );
}

/// #449: the total rematch count (u16) is read at 276 +6,148, not +6,138 where the old span put it.
#[test]
fn the_save_answer_keeps_the_total_rematch_count() {
    let a = card_in_after(Some(&[(8532, &[3, 4, 0x34, 0x12, 1, 0, 0, 0, 2, 0, 0, 0])]));
    assert_eq!(&a[6148..6150], &[0x34, 0x12]);
    assert_eq!(&a[6136..6140], &[3, 4, 0, 0], "serial count and incentive");
    assert_eq!(
        &a[6140..6148],
        &[1, 0, 0, 0, 2, 0, 0, 0],
        "both GT counters"
    );
}

/// #316: the club ranking points, trophies and favourite cards (276 +6,096..+6,135) come from the 277's +8,512;
/// rank, results and the prize-info and penalty flags stay 0.
#[test]
fn the_save_answer_keeps_the_ranking_points() {
    let up: Vec<u8> = (1..=20).collect();
    let a = card_in_after(Some(&[(8512, &up)]));
    assert_eq!(&a[6096..6108], &up[..12], "points and trophies");
    assert_eq!(&a[6126..6132], &up[12..18], "favourite cards");
    assert!(a[6108..6126].iter().chain(&a[6132..6136]).all(|b| *b == 0));
}

/// #449: MOM and worst player (276 +4,154/+4,155) are the 277's +6,556/+6,557, or 16 (no player) with no 277.
#[test]
fn the_save_answer_keeps_mom_and_worst_player() {
    let a = card_in_after(Some(&[(6556, &[13, 2])]));
    assert_eq!(&a[MOM_WORST_276..MOM_WORST_276 + 2], &[13, 2]);
    let a = card_in_after(None);
    assert_eq!(&a[MOM_WORST_276..MOM_WORST_276 + 2], &[NO_PLAYER; 2]);
}

/// #449: the club report reaches the 276 in the client's order. The old span (4158, 6552, 6) sent the evaluation
/// points as the previous cup id (max 59) and the MOM index as the enemy region (max 6).
#[test]
fn the_save_answer_keeps_the_club_report_in_place() {
    // 277: report lines 1..4, cup 21, round 22, evaluation 75, region 5, MOM 13, worst 2
    let a = card_in_after(Some(&[(6548, &[1, 2, 3, 4, 21, 22, 75, 5, 13, 2])]));
    assert_eq!(&a[4154..4164], &[13, 2, 1, 2, 3, 4, 21, 22, 5, 75]);
    assert_ne!(a[4160], 75, "evaluation points are not the previous cup id");
}

/// The team ranking answer 234 is the control's own "no ranking" block (control constructor 0x440844): "NO
/// ENTRY" in the champion's three names and both names of the 8 rows, every other byte 0, the request not echoed.
#[test]
fn the_team_ranking_answer_is_the_controls_no_entry_block() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let (body, refused) = ask_for_test(&store, 233, &fixture(233, [5, 0, 0]), &log);
    assert_eq!(refused, None);
    assert_eq!(body.len(), 1092);
    let mut want = vec![0u8; 1092];
    for at in [8, 36, 92]
        .into_iter()
        .chain((0..8).flat_map(|k| [164 + 116 * k, 220 + 116 * k]))
    {
        want[at..at + 9].copy_from_slice(b"NO ENTRY\0");
    }
    // the last row's second name ends inside the answer: +1,032 + 56 = 1,088, then the row's u32
    assert_eq!(&body[1032..1041], b"NO ENTRY\0");
    assert_eq!(&body[..8], &[0; 8], "term from / to");
    assert_eq!(&body[148..164], &[0; 16], "champion figures and emblem");
    assert_eq!(body, want);
}

/// 2026-11-02: the RANK answer says "no Universal Championship result" (+354) instead of all zeros, so the
/// client clears a Universal Cup winner's championship entry at the play's start.
#[test]
fn the_rank_answer_says_no_championship_result() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = lines.clone();
    let log: Log = Arc::new(move |l: &str| seen.lock().unwrap().push(l.to_owned()));
    let (body, refused) = ask_for_test(&store, 249, &fixture(249, [5, 0, 0]), &log);
    assert_eq!(refused, None);
    assert_eq!(body.len(), 504);
    assert_eq!(body[354], 1, "universalChamp nothingResult");
    assert_eq!(body[8], 0, "the CS rank stays 0");
    // 2026-11-07: a data-match week, never 0, so a cup winner's DATA_VS_ENTRY goes out and keeps its place
    let week = i16::from_le_bytes([body[12], body[13]]);
    assert!(week > 0, "dataVSWeek {week}");
    assert_eq!(week as u16, gt_week(now()).max(1));
    assert!(
        lines.lock().unwrap().iter().any(|l| l
            == &format!("rank answer (250): dataVSWeek {week} - a Universal Cup winner's client sends DATA_VS_ENTRY to the control")),
        "{:?}",
        lines.lock().unwrap()
    );
    let others: Vec<usize> = (4..504)
        .filter(|i| ![12, 13, 354].contains(i) && body[*i] != 0)
        .collect();
    assert!(others.is_empty(), "unexpected bytes at {others:?}");
}

fn req(cmd: u16, ids: [u32; 3]) -> Vec<u8> {
    let (len, rev) = match cmd {
        271 => (20, 16),
        285 => (24, 20),
        _ => (32, 28),
    };
    let mut r = vec![0u8; len];
    put(&mut r, 4, ids[0]);
    put(&mut r, 8, ids[1]);
    put(&mut r, 12, ids[2]);
    put(&mut r, rev, REVISION);
    r
}

fn pad(name: &[u8]) -> Vec<u8> {
    let mut v = name.to_vec();
    v.resize(56, 0);
    v
}

#[test]
fn skip_tutorial_makes_one_club_from_the_template() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(game.join("prog_data").join("cpu_team")).unwrap();
    let template: Vec<u8> = (0..BLOCK + 8).map(|i| (i * 7 + 1) as u8).collect();
    std::fs::write(game.join("prog_data/cpu_team/cpu_50.dat"), &template).unwrap();
    let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let ask = |cmd, ids, skip: Option<&Path>| {
        answer(cmd, &req(cmd, ids), &store, skip, &[], None, &log)
            .unwrap()
            .1
    };

    // off: no club, errCode 0, unchanged
    assert_eq!(u32_at(&ask(271, [5, 0, 0], None), 8 + 60), 0);
    // on: one record with ids and the template
    let body = ask(271, [5, 0, 0], Some(&game));
    let (coach, card) = (u32_at(&body, 8), u32_at(&body, 8 + 60));
    assert!(coach != 0 && card != 0);
    assert_eq!(&body[8 + 4..8 + 4 + 56], &pad(SKIP_COACH)[..]);
    assert_eq!(&body[8 + 64..8 + 64 + 56], &pad(SKIP_CLUB)[..]);
    // a full term (100 matches): 0 is a term that has ended, and the client offers to retire the club
    assert_eq!((body[8 + 122], body[8 + 123]), (100, 0));
    assert_eq!(u32_at(&body, 8 + 124), SKIP_TITLES);
    assert_eq!(u32_at(&body, 8 + 128 + 60), 0, "a single record");
    let save = ask(275, [5, coach, card], Some(&game));
    assert_eq!(u32_at(&save, 4), 0);
    // the template with the seven fields patched
    let mut want = template[..BLOCK].to_vec();
    for (bit, width, value) in [
        (0, 32, 12),
        (185, 32, card),
        (217, 32, coach),
        (20736, 32, 12),
        (21538, 32, 12),
        (22221, 6, 13),
        (18752, 32, 20120221),
        (1504, 25, 20000101),
        (UC_ENTRY_DATE_BIT, 25, 0),
    ] {
        let at = |b: &[u8]| get_bits(b, bit, width);
        put_bits(&mut want, bit, width, value);
        assert_eq!(at(&want), value);
        assert_eq!(at(&save[8..8 + BLOCK]), value, "bit {bit}");
    }
    // the names: manager and club with their readings
    for (at, name) in [
        (76, pad(SKIP_COACH)),
        (132, pad(b"FORZA")),
        (324, pad(SKIP_CLUB)),
        (380, pad(b"FORZA WCCF")),
    ] {
        want[at..at + 56].copy_from_slice(&name);
    }
    assert_eq!(&save[8..8 + BLOCK], &want[..]);
    assert_eq!(u32_at(&save, 3080), 12 | (3072 << 16));
    assert_eq!(u32_at(&save, 3084), sum32(&want));
    assert_ne!(u32_at(&save, 3088), 0);
    // ITEM_LIST: errCode 0, the shop uniforms from the block
    let items = ask(285, [5, coach, card], Some(&game));
    assert_eq!(u32_at(&items, 4), 0);
    let uniform = |bit| get_bits(&want, bit, 8) as u8;
    assert_eq!((items[700], items[701]), (uniform(24249), uniform(24257)));
    // a second 271 returns the same club
    let again = ask(271, [5, 0, 0], Some(&game));
    assert_eq!(again[8..8 + 128], body[8..8 + 128]);
    assert_eq!(u32_at(&again, 8 + 128 + 60), 0);
    // Aime id 0 and a short template make nothing
    assert_eq!(u32_at(&ask(271, [0, 0, 0], Some(&game)), 8 + 60), 0);
    std::fs::write(game.join("prog_data/cpu_team/cpu_50.dat"), [0; 100]).unwrap();
    let (_, body, why) = answer(
        271,
        &req(271, [6, 0, 0]),
        &store,
        Some(&game),
        &[],
        None,
        &log,
    )
    .unwrap();
    assert!(why.is_some() && u32_at(&body, 8 + 60) == 0);
}

#[test]
fn coach_list_pre_registers_the_2d_code_cards() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(game.join("prog_data/player_data")).unwrap();
    // three card-master records: 7127 and 1007 need a 2D code (+0x5DD = 2), 7054 does not (1)
    let mut master = vec![0u8; 3 * RECORD];
    for (i, (no, kind)) in [(7127u16, 2u8), (7054, 1), (1007, 2)]
        .into_iter()
        .enumerate()
    {
        master[i * RECORD + 648..][..2].copy_from_slice(&no.to_le_bytes());
        master[i * RECORD + 0x5DD] = kind;
    }
    std::fs::write(game.join("prog_data/player_data/player_data.bin"), &master).unwrap();
    let rare = rare_cards(&game).unwrap();
    assert_eq!(rare, [7127, 1007]);

    let store = Mutex::new(Store::open(&dir.path().join("gamesvr.db")).unwrap());
    let log: Log = Arc::new(|_: &str| {});
    let body = answer(271, &req(271, [5, 0, 0]), &store, None, &rare, None, &log)
        .unwrap()
        .1;
    assert_eq!(u32_at(&body, 4), 0);
    // World Players List entries: card, registrations 3, owned 0, flag 0
    assert_eq!(&body[1292..1300], &[0xD7, 0x1B, 0, 0, 3, 0, 0, 0]);
    assert_eq!(&body[1300..1308], &[0xEF, 0x03, 0, 0, 3, 0, 0, 0]);
    assert!(body[1308..].iter().all(|b| *b == 0));
    // Aime id 0 (guest): no list
    let guest = answer(271, &req(271, [0, 0, 0]), &store, None, &rare, None, &log)
        .unwrap()
        .1;
    assert!(guest[1292..].iter().all(|b| *b == 0));
    // QR_REGIST, QR_DEL_SERIAL, QR_DEL_CARDID: accepted
    for cmd in [295, 297, 299] {
        let (_, b, why) = answer(cmd, &[0; 32], &store, None, &rare, None, &log).unwrap();
        assert!(why.is_none() && u32_at(&b, 4) == 0, "cmd {cmd}");
    }
}

/// Asks `store` through [`answer`] with no VS and no skip club.
fn ask_store(store: &Mutex<Store>, cmd: u16, p: &[u8]) -> (Vec<u8>, Option<String>) {
    let log: Log = Arc::new(|_: &str| {});
    let (_, body, why) = answer(cmd, p, store, None, &[], None, &log).unwrap();
    (body, why)
}

/// A new club for Aime id 5 in `store`: [aime, coach, card].
fn new_club(store: &Mutex<Store>) -> [u32; 3] {
    let b = ask_store(store, 279, &fixture(279, [5, 0, 0])).0;
    [5, u32_at(&b, 8), u32_at(&b, 12)]
}

/// Sends a 289 INJUSTICE_COUNT with `delta` for the club `ids` and returns the count it answers.
fn injustice(store: &Mutex<Store>, ids: [u32; 3], delta: u32) -> u32 {
    let mut r = fixture(289, ids);
    put(&mut r, 16, delta);
    let (b, why) = ask_store(store, 289, &r);
    assert_eq!(why, None);
    u32_at(&b, 8)
}

/// #267: the client offers a contract renewal only below 5 unfinished plays, so an accepted end-of-play save
/// clears the club's count; five crashed plays no longer block renewals forever.
#[test]
fn an_end_of_play_save_clears_the_injustice_count() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let ids = new_club(&store);
    for n in 1..=5 {
        assert_eq!(injustice(&store, ids, 1), n);
    }
    assert_eq!(u32_at(&ask_store(&store, 277, &fixture(277, ids)).0, 4), 0);
    assert_eq!(injustice(&store, ids, 0), 0);
}

/// Unfinished plays with no accepted save keep counting.
#[test]
fn unfinished_plays_keep_the_injustice_count() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let ids = new_club(&store);
    for _ in 0..5 {
        injustice(&store, ids, 1);
    }
    assert_eq!(injustice(&store, ids, 0), 5);
}

/// A save clears its own club's count only, not another club of the same Aime id.
#[test]
fn a_save_clears_only_its_own_clubs_injustice_count() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let (a, b) = (new_club(&store), new_club(&store));
    assert_ne!(a[1], b[1], "two clubs");
    for _ in 0..5 {
        injustice(&store, a, 1);
        injustice(&store, b, 1);
    }
    assert_eq!(u32_at(&ask_store(&store, 277, &fixture(277, a)).0, 4), 0);
    assert_eq!(injustice(&store, a, 0), 0);
    assert_eq!(injustice(&store, b, 0), 5);
}

/// A play's start (289 +1) and end (277 with its report) as the log says them; other 289s say nothing.
#[test]
fn match_lines_name_the_opponent_score_and_save() {
    let mut start = vec![0u8; 28];
    for (at, v) in [(4, 7), (8, 2), (12, 3), (16, 1)] {
        put(&mut start, at, v);
    }
    assert_eq!(
        match_line(289, &start, None).unwrap(),
        "match start: aime 7 coach 2 card 3"
    );
    put(&mut start, 16, u32::MAX); // -1: the save went through
    assert_eq!(match_line(289, &start, None), None);
    let mut end = vec![0u8; 8572];
    for (at, v) in [(4, 7), (8, 2)] {
        put(&mut end, at, v);
    }
    // a real body's neighbours are nonzero: home 2 then 0x05 0x2D 0x00, away 1 then 0x00 0x3B 0x01 (run log)
    end[3416..3420].copy_from_slice(&[3, 5, 0x2D, 0]);
    end[3428..3432].copy_from_slice(&[1, 0, 0x3B, 1]);
    end[24] = 2;
    end[3300..3303].copy_from_slice(b"CPU");
    end[3356..3366].copy_from_slice(b"BEST ELEVN");
    assert_eq!(
        match_line(277, &end, None).unwrap(),
        "match end: aime 7 coach 2 vs \"BEST ELEVN\" (manager \"CPU\", coach 0) score 3-1 pk 0-0, save kind 2: \
         result save ok"
    );
    assert!(
        match_line(277, &end, Some("block sum 0x1"))
            .unwrap()
            .ends_with("result save FAILED (block sum 0x1)")
    );
}

/// The rows of the 2017-18 tables whose for-sale flags the client checks (ShopUni, GoalBgm, Banner, Jersey), SE left
/// out. The old fixed list's BGM 1..3, Flag 1..3 and Trainingwear 1..2 are not among them.
fn for_sale(cat: u32, item: u32) -> bool {
    match cat {
        1 => (1..=17).contains(&item),
        3 => (38..=51).contains(&item),
        4 => (241..=260).contains(&item),
        5 => matches!(item, 38..=40 | 54..=56 | 90 | 140..=144),
        _ => false,
    }
}

/// Client sub_834520's date check of an entry `[cat, item, start, end]` at a local date and hour: today in
/// `[start, end + 1]`, from 10:00 on the start day, until 02:59 on the day after `end`.
fn client_offers(e: [u32; 4], date: chrono::NaiveDate, hour: u32) -> bool {
    let ymd = |d: chrono::NaiveDate| d.year() as u32 * 10_000 + d.month() * 100 + d.day();
    let end_next = chrono::NaiveDate::parse_from_str(&e[3].to_string(), "%Y%m%d").unwrap()
        + chrono::Days::new(1);
    let today = ymd(date);
    (e[2]..=ymd(end_next)).contains(&today)
        && (today != e[2] || hour >= 10)
        && (today != ymd(end_next) || hour <= 2)
}

#[test]
fn the_sale_list_has_ten_for_sale_entries() {
    let before = now();
    let (body, _) = ask_store(
        &Mutex::new(Store::open(Path::new(":memory:")).unwrap()),
        237,
        &[0; 20],
    );
    let entries: Vec<_> = (0..10)
        .map(|k| {
            let e = SHOP_SALE_AT + 16 * k;
            [0, 4, 8, 12].map(|i| u32_at(&body, e + i))
        })
        .collect();
    assert!(entries.iter().all(|e| for_sale(e[0], e[1])), "{entries:?}");
    // the start and end dates on the wire are shop_sale's of the request time
    let sent = |t| shop_sale(t).collect::<Vec<_>>();
    assert!(
        entries == sent(before) || entries == sent(now()),
        "{entries:?}"
    );
    assert!(
        entries.iter().all(|e| e[2] != 0 && e[3] > e[2]),
        "{entries:?}"
    );
    let mut kinds: Vec<_> = entries.iter().map(|e| e[0]).collect();
    kinds.dedup();
    assert_eq!(kinds, [1, 3, 4, 5], "no SE");
    assert_eq!(SHOP_SALE_AT + 160, 23_072);
    // every for-sale row comes up within three weeks, each day's ten are distinct
    let mut seen = std::collections::HashSet::new();
    for day in 0..21 {
        let list: Vec<_> = shop_sale(utc_secs(2026, 11, 1, 12, 0) + day * 86_400).collect();
        assert_eq!(list.len(), 10);
        assert!(list.iter().all(|e| for_sale(e[0], e[1])), "{list:?}");
        let ids: std::collections::HashSet<_> = list.iter().map(|e| (e[0], e[1])).collect();
        assert_eq!(ids.len(), 10, "{list:?}");
        seen.extend(ids);
    }
    assert_eq!(seen.len(), 17 + 14 + 20 + 12);
}

#[test]
fn the_sale_list_rotates_daily_and_outlives_a_long_running_control() {
    // Saturday 2026-11-21 12:00 CET = 11:00 UTC
    let t = utc_secs(2026, 11, 21, 11, 0);
    let list: Vec<_> = shop_sale(t).collect();
    assert_eq!(list.len(), 10);
    let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
    for e in &list {
        // open at the fetch, on the day before from 10:00; closed before
        assert!(client_offers(*e, day(21), 12));
        assert!(client_offers(*e, day(20), 10) && !client_offers(*e, day(20), 9));
        // a control left running keeps all ten: past a week (the old D+6 end closed at 2026-11-28 03:00) and a month
        assert!(client_offers(*e, day(28), 3) && client_offers(*e, day(30), 12));
        assert!(client_offers(*e, day(21) + chrono::Days::new(60), 12));
    }
    assert!(
        list.iter()
            .all(|e| e[2] == 20_261_120 && e[3] == 20_271_121),
        "{list:?}"
    );
    // deterministic per Rome day: same list all day (00:00..23:59 CET), a new one the next day
    let ids = |t| shop_sale(t).map(|e| (e[0], e[1])).collect::<Vec<_>>();
    assert_eq!(
        ids(utc_secs(2026, 11, 20, 23, 0)),
        ids(utc_secs(2026, 11, 21, 22, 59))
    );
    assert_ne!(ids(t), ids(t + 86_400));
    assert_ne!(ids(t), ids(t - 86_400));
}

#[test]
fn purchases_are_stored_and_applied_to_286_and_276() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gamesvr.db");
    let buy = |store: &Mutex<Store>, cat: u32, item: u32| {
        let mut r = req(315, [5, cat, item]);
        put(&mut r, 16, 99);
        ask_store(store, 315, &r)
    };
    let (club, card);
    {
        let store = Mutex::new(Store::open(&path).unwrap());
        let b = ask_store(&store, 279, &fixture(279, [5, 0, 0])).0;
        (club, card) = (u32_at(&b, 8), u32_at(&b, 12));
        assert_eq!(
            u32_at(&ask_store(&store, 277, &fixture(277, [5, club, card])).0, 4),
            0
        );
        for (cat, item) in [(4, 7), (5, 3), (1, 3), (3, 9)] {
            let (b, why) = buy(&store, cat, item);
            assert!(why.is_none() && u32_at(&b, 4) == 0, "{cat}/{item}");
        }
    }
    // reopened: still bought, and applied for another club of the same Aime id too
    let store = Mutex::new(Store::open(&path).unwrap());
    let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
    assert_eq!((a[5444 + 7], a[5188 + 3], a[5444 + 3]), (1, 1, 0));
    // the stored 277 echoed a 0 for the bit: still 1 after another save
    assert_eq!(
        u32_at(&ask_store(&store, 277, &fixture(277, [5, club, card])).0, 4),
        0
    );
    let a = ask_store(&store, 275, &req(275, [5, club, card])).0;
    assert_eq!((a[5444 + 7], a[5188 + 3]), (1, 1));
    assert_eq!(
        u32_at(&a, 3088),
        {
            let mut z = a.clone();
            put(&mut z, 3088, 0);
            sum32(&z)
        },
        "C2 covers the flags"
    );
    let i = ask_store(&store, 285, &req(285, [5, club, card])).0;
    assert_eq!((i[700], i[701], i[42], i[93]), (3, 3, 9, 0));
    // #316: an SE set k plays ShootSe row 161 + 3k (its first shoot button), not row k
    buy(&store, 2, 4);
    assert_eq!(
        ask_store(&store, 285, &req(285, [5, club, card])).0[93],
        173
    );
    buy(&store, 2, 0);
    assert_eq!(
        ask_store(&store, 285, &req(285, [5, club, card])).0[93],
        161
    );
    // the last set, 13, is row 200; sets past it (and 256, which must not wrap to set 0) leave +93 alone
    for item in [13, 14, 100, 256] {
        buy(&store, 2, item);
        assert_eq!(
            ask_store(&store, 285, &req(285, [5, club, card])).0[93],
            200,
            "set {item}"
        );
    }
    // another Aime id has nothing
    let i = ask_store(&store, 285, &req(285, [6, club, card])).0;
    assert_eq!((i[700], i[42], i[93]), (0, 0, 0));
    // a later uniform replaces the earlier one
    buy(&store, 1, 4);
    assert_eq!(ask_store(&store, 285, &req(285, [5, club, card])).0[700], 4);
    // a store error is errCode 1
    store
        .lock()
        .unwrap()
        .0
        .execute_batch("DROP TABLE purchases")
        .unwrap();
    let (b, why) = buy(&store, 4, 1);
    assert!(why.is_some() && u32_at(&b, 4) == 1);
}

/// A 315 is at most 64 bytes, a repeat of an item adds no row (and moves it to the newest, so the last uniform bought
/// still wins), and an Aime id holds at most 1,000 items: what a club copy takes, so its copy back still checks.
#[test]
fn buy_is_bounded_and_copy_back_still_checks() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let buy = |cat: u32, item: u32, len: usize| {
        let mut r = req(315, [5, cat, item]);
        r.resize(len, 0);
        let (b, why) = ask_store(&store, 315, &r);
        (u32_at(&b, 4), why)
    };
    let rows = || -> u32 {
        let st = store.lock().unwrap();
        st.0.query_row("SELECT count(*) FROM purchases", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(buy(4, 7, 65).0, 1, "a 65-byte 315 is refused");
    assert_eq!(rows(), 0);
    assert_eq!(buy(4, 7, 24), (0, None));
    assert_eq!(buy(4, 7, 64), (0, None), "a repeat answers OK");
    assert_eq!(rows(), 1, "and adds no row");
    let club = new_club(&store);
    let uniform = || ask_store(&store, 285, &req(285, club)).0[700];
    for item in [3, 4, 3] {
        assert_eq!(buy(1, item, 24).0, 0);
    }
    assert_eq!((uniform(), rows()), (3, 3), "the repeat is the newest");
    for item in 1000..1000 + MAX_PURCHASES as u32 - 3 {
        assert_eq!(buy(5, item, 24).0, 0, "item {item}");
    }
    assert_eq!(rows(), MAX_PURCHASES as u32);
    let (code, why) = buy(5, 9, 24);
    assert!(code == 1 && why.is_some(), "the 1,001st item is refused");
    assert_eq!(buy(4, 7, 24), (0, None), "a repeat still answers OK");
    assert_eq!(rows(), MAX_PURCHASES as u32);
    let st = store.lock().unwrap();
    super::super::club::export(&st.0, 5)
        .unwrap()
        .check()
        .unwrap();
}

/// A 311 request: slot 1 and an area id of 0.
const SLOT: [u8; 8] = [1, 0, 0, 0, 0, 0, 0, 0];

/// A synthetic 309: every byte a function of its offset, shop id 7, the block's version fields 12 and a NUL ending
/// the shop name.
fn upload_309(seed: usize) -> Vec<u8> {
    let mut p: Vec<u8> = (0..3368)
        .map(|i| ((i * 13 + seed) % 251) as u8 + 1)
        .collect();
    put(&mut p, 4, 7);
    for bit in BLOCK_VERSIONS {
        put_bits(&mut p[8..8 + BLOCK], bit, 32, 12);
    }
    p[3221] = 0;
    p
}

/// Uploads from any number of shops keep one row, cut to 3,368 bytes; a 309 whose block is not a current save or
/// whose shop name has no NUL is refused and keeps the last good one; a server with VS answers the CPU team.
#[test]
fn best_eleven_keeps_one_checked_row() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    let rows = || -> (u32, i64) {
        let st = store.lock().unwrap();
        st.0.query_row(
            "SELECT count(*), coalesce(max(length(payload)), 0) FROM requests WHERE cmd = 309",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    for shop in 1..50 {
        let mut p = upload_309(shop as usize);
        put(&mut p, 4, shop);
        p.resize(8000, 0xAA);
        assert_eq!(ask_store(&store, 309, &p).1, None);
    }
    assert_eq!(rows(), (1, 3368), "one row, trimmed");
    let good = ask_store(&store, 311, &SLOT).0;
    assert_eq!(u32_at(&good, 4), 0);
    let mut old = upload_309(60);
    put_bits(&mut old[8..8 + BLOCK], BLOCK_VERSIONS[1], 32, 10);
    let mut unnamed = upload_309(61);
    unnamed[3194..3222].fill(b'A');
    for bad in [old, unnamed] {
        let (b, why) = ask_store(&store, 309, &bad);
        assert!(why.is_some() && u32_at(&b, 4) == 1);
    }
    assert_eq!(
        ask_store(&store, 311, &SLOT).0,
        good,
        "the last good one stays"
    );
    // with VS (`gt`): errCode 1, the CPU team
    let log: Log = Arc::new(|_: &str| {});
    let mut body = vec![0u8; 3352];
    put(&mut body, 4, 1);
    club(311, &SLOT, &mut body, &store, None, &[], true, &log).unwrap();
    assert_eq!((u32_at(&body, 4), &body[152..160]), (1, &[0u8; 8][..]));
}

#[test]
fn best_eleven_309_comes_back_as_312() {
    let store = Mutex::new(Store::open(Path::new(":memory:")).unwrap());
    // nothing stored: errCode 1, the CPU team
    let (b, why) = ask_store(&store, 311, &SLOT);
    assert_eq!((b.len(), u32_at(&b, 4), why), (3352, 1, None));
    let (b, why) = ask_store(&store, 309, &upload_309(1));
    assert_eq!((b.len(), u32_at(&b, 4), why), (8, 0, None));
    let p = upload_309(2);
    assert_eq!(u32_at(&ask_store(&store, 309, &p).0, 4), 0);
    let (b, why) = ask_store(&store, 311, &SLOT);
    assert_eq!((b.len(), u32_at(&b, 4), why), (3352, 0, None));
    assert_eq!(b[8..120], [0; 112], "names the client does not read");
    assert_eq!(b[120..148], p[3194..3222]);
    assert_eq!(b[148..152], p[3224..3228]);
    assert_eq!(b[152..3224], p[8..3080]);
    assert_eq!(b[3224..3352], p[3232..3360]);
    // ALL.Net's "none" is -1; a short 309 is refused (errCode 1) and keeps the last good one
    let mut q = upload_309(3);
    put(&mut q, 3224, 0x8000_0000);
    ask_store(&store, 309, &q);
    assert_eq!(u32_at(&ask_store(&store, 311, &SLOT).0, 148), u32::MAX);
    let (b, why) = ask_store(&store, 309, &q[..100]);
    assert!(why.is_some() && u32_at(&b, 4) == 1);
    assert_eq!(ask_store(&store, 311, &SLOT).0[152], q[8]);
}

#[test]
fn frame_cap_rejects_before_buffering() {
    let mut h = [0u8; 40];
    h[16..20].copy_from_slice(&(MAX_FRAME as u32).to_le_bytes());
    assert_eq!(frame_len(&h), Ok(MAX_FRAME));
    h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(frame_len(&h), Err(u32::MAX as usize));
    h[16..20].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(frame_len(&h), Ok(40), "a size below 40 is floored");
    const { assert!(MAX_FRAME >= 8572 + 40) }
}

#[test]
fn oversized_header_closes_and_per_ip_limit_holds() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let o = Options {
        max_per_ip: 1,
        dump: false,
        ..Options::default()
    };
    let _svc = spawn_opts(l, dir.path().into(), Arc::new(|_| {}), o).unwrap();
    let mut a = std::net::TcpStream::connect(addr).unwrap();
    let mut h = [0u8; 40];
    h[..8].copy_from_slice(&MAGIC);
    h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    std::io::Write::write_all(&mut a, &h).unwrap();
    a.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    assert_eq!(a.read(&mut [0u8; 8]).unwrap(), 0, "closed by the server");
    // the slot is free again: a second connection is served, a third at the same time is not
    std::thread::sleep(std::time::Duration::from_millis(300));
    let _b = std::net::TcpStream::connect(addr).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut c = std::net::TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    assert_eq!(c.read(&mut [0u8; 8]).unwrap(), 0, "over the per-IP limit");
    assert!(
        std::fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("req_")),
        "dump off"
    );
}

/// A game server on loopback with `o`, and a link to it that answers in 5 s.
fn serve(o: Options) -> (Handle, impl Fn() -> std::net::TcpStream, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let svc = spawn_opts(l, dir.path().into(), Arc::new(|_| {}), o).unwrap();
    let link = move || {
        let c = std::net::TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c
    };
    (svc, link, dir)
}

/// `n` keep-alives (18) in one write, then their `n` answers read back.
fn echoes(c: &mut std::net::TcpStream, n: u16) {
    use std::io::Read;
    let mut out = Vec::new();
    for seq in 0..n {
        out.extend_from_slice(&header(18, seq, 0, 4, 0));
        out.extend_from_slice(&[1, 0, 0, 0]);
    }
    c.write_all(&out).unwrap();
    let mut back = vec![0u8; 44 * usize::from(n)];
    c.read_exact(&mut back).unwrap();
    assert_eq!(
        u16_at(&back[44 * (usize::from(n) - 1)..], 10),
        n - 1,
        "the last one answered"
    );
}

/// 250 frames sent at once are all answered, but at 100 a second: the link waits twice.
#[test]
fn a_flood_is_slowed_not_dropped() {
    let (_svc, link, _dir) = serve(Options {
        dump: false,
        ..Options::default()
    });
    let mut c = link();
    let t = Instant::now();
    echoes(&mut c, 250);
    let took = t.elapsed();
    assert!(took >= Duration::from_millis(1800), "{took:?}");
    // a client at its own pace waits for nothing
    std::thread::sleep(Duration::from_secs(1));
    let t = Instant::now();
    echoes(&mut c, 20);
    assert!(t.elapsed() < Duration::from_millis(500));
}

#[test]
fn raw_files_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..205 {
        std::fs::write(dir.path().join(format!("req_1_{i}.bin")), [0]).unwrap();
    }
    std::fs::write(dir.path().join("gamesvr.db"), [0]).unwrap();
    prune(dir.path(), 200);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        201,
        "200 .bin files and the store"
    );
}
