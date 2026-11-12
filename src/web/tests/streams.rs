//! Event streams, and who a client is behind a proxy.

use super::*;

#[test]
fn clients_are_told_apart_by_address_or_ipv6_prefix() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    assert!(on_our_network(ip("172.18.0.2")) && on_our_network(ip("::1")) && on_our_network(ip("fd00::1")));
    assert!(!on_our_network(ip("141.94.96.119")) && !on_our_network(ip("2001:db8::1")) && !on_our_network(ip("::ffff:141.94.96.119")));
    assert_eq!(client_key(ip("2001:db8:1:2:aaaa::1")), client_key(ip("2001:db8:1:2:bbbb::2")));
    assert_ne!(client_key(ip("2001:db8:1:2::1")), client_key(ip("2001:db8:1:3::1")));
    assert_eq!(client_key(ip("::ffff:203.0.113.1")), ip("203.0.113.1"));
}

/// A client of `/events`, with the server's thread serving it.
struct EventStream {
    client: BufReader<TcpStream>,
    server: std::thread::JoinHandle<Result<()>>,
}

impl EventStream {
    fn open(app: &Arc<App>, send: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        client.write_all(send.as_bytes()).unwrap();
        let (conn, _) = listener.accept().unwrap();
        let app = app.clone();
        EventStream { client: BufReader::new(client), server: std::thread::spawn(move || serve(conn, &app)) }
    }

    /// What came in up to and including the first line with `needle`.
    fn read_to(&mut self, needle: &str) -> String {
        read_to(&mut self.client, needle)
    }

    /// Hangs up and waits for the server to notice, which it does when writing the next lines.
    fn close(self, hub: &SharedHub) {
        drop(self.client);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.server.is_finished() {
            assert!(Instant::now() < deadline, "the stream outlived its client");
            publish(hub, &server("cp_process_f12"));
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.server.join();
    }
}

fn read_to(reader: &mut impl BufRead, needle: &str) -> String {
    let mut seen = String::new();
    while !seen.lines().last().is_some_and(|l| l.contains(needle)) {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap_or_else(|e| panic!("waiting for {needle}: {e}, after {seen}"));
        assert!(n > 0, "the stream ended waiting for {needle}, after {seen}");
        seen.push_str(&line);
    }
    seen
}

fn one_match() -> (Arc<App>, SharedHub) {
    let hub = SharedHub::default();
    (Arc::new(app(Matches::One(hub.clone()))), hub)
}

fn heal() -> Line {
    Line::Heal(output::Heal { tick: 1, patient: PlayerId(4), healer: Some(PlayerId(3)), amount: 20, source: output::HealSource::Medigun })
}

#[test]
fn an_event_stream_sends_what_happened_and_then_what_happens() {
    let (app, hub) = one_match();
    publish(&hub, &server("cp_process_f12"));
    let mut stream = EventStream::open(&app, "GET /events HTTP/1.1\r\n\r\n");
    let head = stream.read_to("retry: 1000");
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Type: text/event-stream"), "{head}");
    assert!(!head.contains("Content-Encoding"), "{head}");
    stream.read_to("cp_process_f12");
    publish(&hub, &server("cp_badlands"));
    let live = stream.read_to("cp_badlands");
    assert!(live.lines().last().unwrap().starts_with("data: {"), "{live}");
    stream.close(&hub);
}

#[test]
fn a_gzipped_event_stream_can_be_read_after_every_batch() {
    let (app, hub) = one_match();
    publish(&hub, &server("cp_process_f12"));
    let mut stream = EventStream::open(&app, "GET /events HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n");
    let head = stream.read_to("Access-Control");
    assert!(head.contains("Content-Encoding: gzip"), "{head}");
    stream.read_to("X-Content-Type-Options");
    let mut blank = String::new();
    stream.client.read_line(&mut blank).unwrap();
    let mut events = BufReader::new(GzDecoder::new(stream.client));
    assert!(read_to(&mut events, "cp_process_f12").starts_with("retry: 1000"));
    // Only got here if the first batch was flushed; so must the next, with the stream still open.
    publish(&hub, &server("cp_badlands"));
    read_to(&mut events, "cp_badlands");
}

#[test]
fn only_streams_that_ask_hear_detail_lines() {
    let (app, hub) = one_match();
    let mut plain = EventStream::open(&app, "GET /events HTTP/1.1\r\n\r\n");
    let mut detail = EventStream::open(&app, "GET /events?detail=1 HTTP/1.1\r\n\r\n");
    plain.read_to("retry:");
    detail.read_to("retry:");
    // Both are subscribed once this line reaches them.
    publish(&hub, &server("cp_process_f12"));
    plain.read_to("cp_process_f12");
    detail.read_to("cp_process_f12");
    publish(&hub, &heal());
    publish(&hub, &server("cp_badlands"));
    assert!(!plain.read_to("cp_badlands").contains("\"heal\""));
    assert!(detail.read_to("cp_badlands").contains("\"heal\""));
    plain.close(&hub);
    detail.close(&hub);
}

#[test]
fn closing_a_match_stream_frees_its_viewer_and_slot() {
    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let app = Arc::new(app(Matches::Many(sessions)));
    let mut stream = EventStream::open(&app, "GET /w/demo/events HTTP/1.1\r\n\r\n");
    stream.read_to("retry:");
    assert!(m.watched());
    assert_eq!(app.streams.lock().unwrap().values().sum::<usize>(), 1);
    stream.close(&m.hub);
    assert!(!m.watched());
    assert!(app.streams.lock().unwrap().is_empty());
}

#[test]
fn too_many_streams_are_refused_and_not_counted_as_viewers() {
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let (one, _hub) = one_match();
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| one.stream_slot(loopback).unwrap()).collect();
    assert!(EventStream::open(&one, "GET /events HTTP/1.1\r\n\r\n").read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));

    let sessions = Sessions::new(false, 1, 20.0);
    sessions.pin("demo", |_| {});
    let m = sessions.get("demo").unwrap();
    let many = Arc::new(app(Matches::Many(sessions)));
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| many.stream_slot(loopback).unwrap()).collect();
    assert!(EventStream::open(&many, "GET /w/demo/events HTTP/1.1\r\n\r\n").read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));
    assert!(!m.watched());
}

#[test]
fn only_a_proxy_of_ours_is_believed_about_the_client() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let claimed = [ip("203.0.113.9")];
    assert_eq!(client_of(&claimed, ip("172.18.0.2")), ip("203.0.113.9"));
    assert_eq!(client_of(&claimed, ip("141.94.96.119")), ip("141.94.96.119"), "a stranger cannot choose whose streams count");
    assert_eq!(client_of(&[], ip("172.18.0.2")), ip("172.18.0.2"));
    // Visitor, Cloudflare, our proxy: the visitor, whatever they claimed before that.
    let via = [ip("10.0.0.1"), ip("81.2.69.160"), ip("162.158.1.2")];
    assert_eq!(client_of(&via, ip("172.18.0.2")), ip("81.2.69.160"));
    assert_eq!(client_of(&[ip("81.2.69.160"), ip("2a06:98c0::1")], ip("172.18.0.2")), ip("81.2.69.160"));
    // Straight to our proxy, claiming to be Cloudflare: themselves.
    assert_eq!(client_of(&[ip("81.2.69.160"), ip("162.158.1.2"), ip("89.160.20.112")], ip("172.18.0.2")), ip("89.160.20.112"));
    assert!(is_cloudflare(ip("104.27.255.255")) && !is_cloudflare(ip("104.28.0.0")));
    assert!(is_cloudflare(ip("::ffff:173.245.63.1")) && !is_cloudflare(ip("2a06:98c8::")));
}

#[test]
fn forwarded_addresses_are_read_in_order() {
    let forwarded = |header: &str| {
        let send = format!("GET / HTTP/1.1\r\n{header}\r\n\r\n");
        read_request(&mut send.as_bytes()).unwrap().forwarded.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
    };
    assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9"), "203.0.113.9");
    assert_eq!(forwarded("x-forwarded-for: 198.51.100.7, 203.0.113.9"), "198.51.100.7 203.0.113.9");
    assert_eq!(forwarded("X-Forwarded-For: 2001:db8::1"), "2001:db8::1");
    assert_eq!(forwarded("X-Forwarded-For: 203.0.113.9, nonsense"), "");
    assert_eq!(forwarded("Host: x"), "");
}

#[test]
fn streams_behind_a_proxy_are_capped_per_forwarded_client() {
    let (app, hub) = one_match();
    let full: IpAddr = "203.0.113.9".parse().unwrap();
    let _open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| app.stream_slot(full).unwrap()).collect();
    let mut refused = EventStream::open(&app, "GET /events HTTP/1.1\r\nX-Forwarded-For: 203.0.113.9\r\n\r\n");
    assert!(refused.read_to("HTTP/1.1").starts_with("HTTP/1.1 429"));
    let mut other = EventStream::open(&app, "GET /events HTTP/1.1\r\nX-Forwarded-For: 203.0.113.10\r\n\r\n");
    other.read_to("retry:");
    other.close(&hub);
}
