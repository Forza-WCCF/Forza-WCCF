//! The Launcher's lobby workers against an in-process desk: LOBBY into rows, JOIN into a ticket, and the ticket
//! claiming exactly that seat through `WANT ... TICKET`.

use forza::online::lobby::Reserved;
use forza::online::seat::{self, Timing};
use forza_server::desk::{DeskConfig, start};
use forza_server::{Edition, Handle};
use forza_ui::lobby::{LobbyView, RowState};
use launcher::lobby::{Book, MISSES, Msg, Poller};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn desk() -> Handle {
    let mut c = DeskConfig::new(Edition::E1718);
    c.bind = "127.0.0.1".into();
    c.port = 0;
    c.lobby_gap = Duration::ZERO;
    c.lobby_cache = Duration::ZERO;
    start(c, Arc::new(|_| {})).unwrap()
}

fn next(p: &Poller) -> Msg {
    p.rx.recv_timeout(Duration::from_secs(5)).expect("a worker answer")
}

#[test]
fn lobby_rows_then_join_ticket_then_want_ticket_claims_that_seat() {
    let d = desk();
    let server = d.addr().to_string();
    let wake: launcher::lobby::Wake = Arc::new(|| {});
    let p = Poller::start(
        vec![server.clone()],
        "2017-18",
        Duration::from_millis(100),
        wake.clone(),
    );

    // LOBBY -> rows, with a ping
    let m = next(&p);
    assert!(
        matches!(
            &m,
            Msg::Poll {
                ping_ms: Some(_),
                result: Ok(_),
                ..
            }
        ),
        "{m:?}"
    );
    let mut book = Book::new(std::slice::from_ref(&server));
    book.apply(&m, Instant::now());
    let mut view = LobbyView::new();
    let at = book.fill(&mut view, Instant::now());
    assert_eq!(view.arcades.len(), 1);
    let row = view.arcades[0].clone();
    assert_eq!((row.state.clone(), row.players, row.max), (RowState::Open, 0, 8));
    assert_eq!(at[&row.id], server);
    assert_eq!(view.updated_s, Some(0));

    // JOIN -> a ticket for the lowest free seat, which the desk now counts busy
    p.reserve(&server, &row.id, &row.name, None, wake.clone());
    let Msg::Reserved {
        result: Ok(Reserved::Ticket { ticket, seat: held }),
        ..
    } = next_reserved(&p)
    else {
        panic!("no ticket");
    };
    assert_eq!(held, 1);

    // WANT ... TICKET t claims that very seat; a rejoin asks for seat 5 by number
    let t = Timing::default();
    let lease = seat::claim_with(
        d.addr(),
        None,
        Some(&ticket),
        None,
        Duration::from_secs(30),
        t,
        Arc::new(|_| {}),
    )
    .unwrap();
    assert_eq!(lease.seat, held);
    p.reserve(&server, &row.id, &row.name, Some(5), wake.clone());
    let Msg::Reserved {
        result: Ok(Reserved::Ticket { ticket: t5, seat: s5 }),
        ..
    } = next_reserved(&p)
    else {
        panic!("no ticket");
    };
    assert_eq!(s5, 5);
    let l5 = seat::claim_with(
        d.addr(),
        None,
        Some(&t5),
        None,
        Duration::from_secs(30),
        t,
        Arc::new(|_| {}),
    )
    .unwrap();
    assert_eq!(l5.seat, 5);

    // the ticket was spent by the claim: the heartbeat says gone
    let _beat = p.beat(&server, &ticket, Duration::from_millis(50), wake);
    loop {
        if let Msg::Gone { ticket: g } = next(&p) {
            assert_eq!(g, ticket);
            break;
        }
    }
}

fn next_reserved(p: &Poller) -> Msg {
    loop {
        let m = next(p);
        if matches!(m, Msg::Reserved { .. }) {
            return m;
        }
    }
}

#[test]
fn silent_servers_show_no_answer_and_the_list_goes_offline() {
    let d = desk();
    let server = d.addr().to_string();
    let l = forza::online::lobby::fetch(d.addr(), "2017-18").unwrap();
    let poll = |result| Msg::Poll {
        server: server.clone(),
        ping_ms: Some(9),
        result,
    };
    let mut book = Book::new(std::slice::from_ref(&server));
    let mut view = LobbyView::new();
    let now = Instant::now();
    book.apply(&poll(Ok(l)), now);
    book.fill(&mut view, now);
    assert!(!view.offline);
    for _ in 0..MISSES {
        assert!(!book.offline() || view.arcades[0].state == RowState::NoAnswer);
        book.apply(&poll(Err("down".into())), now);
        book.fill(&mut view, now + Duration::from_secs(20));
    }
    assert!(view.offline && view.direct_open);
    assert_eq!(view.arcades[0].state, RowState::NoAnswer);
    assert_eq!(view.updated_s, Some(20));
}

#[test]
fn leave_releases_the_seat_at_once() {
    let d = desk();
    let server = d.addr().to_string();
    let id = forza::online::lobby::fetch(d.addr(), "2017-18").unwrap().arcades[0]
        .id
        .clone();
    let Ok(Reserved::Ticket { ticket, seat }) = forza::online::lobby::reserve(d.addr(), &id) else {
        panic!("no ticket");
    };
    launcher::lobby::release(&server, &ticket).join().unwrap();
    assert_eq!(
        forza::online::lobby::fetch(d.addr(), "2017-18").unwrap().arcades[0].seats,
        0
    );
    assert_eq!(forza::online::lobby::heartbeat(d.addr(), &ticket), Ok(false));
    assert!(matches!(
        forza::online::lobby::reserve_seat(d.addr(), &id, Some(seat)),
        Ok(Reserved::Ticket { .. })
    ));
}

#[test]
fn servers_take_host_port_and_names() {
    use launcher::lobby::{socket, valid_server};
    assert_eq!(
        socket("203.0.113.10:20032"),
        Some("203.0.113.10:20032".parse().unwrap())
    );
    assert_eq!(socket("10.0.0.5"), Some("10.0.0.5:20030".parse().unwrap()));
    assert_eq!(socket("localhost:20032"), Some("127.0.0.1:20032".parse().unwrap()));
    assert!(valid_server("lobby.wccf.it") && valid_server("203.0.113.10:20032"));
    assert!(!valid_server("") && !valid_server("host:0") && !valid_server("a b"));
}

#[test]
fn the_project_server_is_asked_when_none_is_saved() {
    use launcher::Edition;
    use launcher::lobby::{PROJECT_SERVER_1011, PROJECT_SERVER_1718, servers, valid_server};
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    assert!(valid_server(PROJECT_SERVER_1011) && valid_server(PROJECT_SERVER_1718));
    assert_eq!(servers(&settings, "", Edition::E1011), [PROJECT_SERVER_1011]);
    assert_eq!(servers(&settings, "", Edition::E1718), [PROJECT_SERVER_1718]);
    // the address of an older setup stays first
    assert_eq!(
        servers(&settings, "10.0.0.5", Edition::E1011),
        ["10.0.0.5", PROJECT_SERVER_1011]
    );
    // a saved list (typed addresses go in front of the default, online.rs) is asked as saved
    launcher::choices::set_setting_json(&settings, "servers", serde_json::json!(["lobby.example:20040"])).unwrap();
    assert_eq!(servers(&settings, "10.0.0.5", Edition::E1011), ["lobby.example:20040"]);
}
