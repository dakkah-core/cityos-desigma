//! A session over the public relays and discovery, joined from the shared link text.
//! Needs internet access, so it is ignored by default: `cargo test -p desigma-net --test internet -- --ignored`.
use desigma_net::{Guest, GuestEvent, GuestOptions, Host, HostEvent, HostOptions, Network, Ticket};
use std::time::{Duration, Instant};

const T: Duration = Duration::from_secs(60);

#[test]
#[ignore = "needs internet access to the public relays"]
fn join_over_the_internet_from_a_link() {
    let host = Host::start(HostOptions { network: Network::Internet, password: Some("pw".into()), ..Default::default() }).unwrap();
    let link = host.wait_ticket(T).expect("host bound").to_link();
    let ticket = Ticket::parse(&format!("  {link}\n")).unwrap();
    assert!(ticket.password_required);

    let t0 = Instant::now();
    let (guest, peer) = Guest::join_blocking(&ticket, "Ann", Some("pw"), GuestOptions { network: Network::Internet, ..Default::default() }, T).unwrap();
    eprintln!("joined as peer {peer} in {:?}", t0.elapsed());

    guest.send(b"hello host".to_vec()).unwrap();
    let end = Instant::now() + T;
    loop {
        match host.recv_timeout(end - Instant::now()) {
            Some(HostEvent::Message { peer: 1, bytes }) => {
                assert_eq!(bytes, b"hello host");
                break;
            }
            Some(_) => {}
            None => panic!("no message from the guest"),
        }
    }
    host.send(1, vec![7; 1 << 20]).unwrap();
    loop {
        match guest.recv_timeout(end - Instant::now()) {
            Some(GuestEvent::Message(b)) => {
                assert_eq!(b.len(), 1 << 20);
                break;
            }
            Some(_) => {}
            None => panic!("no message from the host"),
        }
    }
    eprintln!("round trip done in {:?}", t0.elapsed());
    guest.leave();
    host.shutdown();
}
