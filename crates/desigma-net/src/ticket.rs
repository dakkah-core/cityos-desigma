//! Join tickets and `desigma://join/<base32>` links.
use crate::NetError;
use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};

const SCHEME: &str = "desigma://join/";
const TICKET_VERSION: u8 = 1;

/// Everything a guest needs to find the host: endpoint id, addresses, and whether a password is
/// required. The ticket is the session secret; anyone holding it can try to join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub addr: EndpointAddr,
    pub password_required: bool,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    v: u8,
    pw: bool,
    addr: EndpointAddr,
}

impl Ticket {
    pub fn endpoint_id(&self) -> EndpointId {
        self.addr.id
    }

    /// The same ticket with only relay addresses, when it has any. Direct addresses change
    /// between runs; a link meant to last (an always-on host) should not carry them.
    pub fn relay_only(&self) -> Ticket {
        let mut t = self.clone();
        if t.addr.addrs.iter().any(|a| a.is_relay()) {
            t.addr.addrs.retain(|a| a.is_relay());
        }
        t
    }

    /// Bare ticket text (lowercase base32).
    pub fn encode(&self) -> String {
        let wire = Wire { v: TICKET_VERSION, pw: self.password_required, addr: self.addr.clone() };
        let bytes = postcard::to_stdvec(&wire).expect("ticket serializes");
        BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
    }

    /// Shareable link: `desigma://join/<ticket>`.
    pub fn to_link(&self) -> String {
        format!("{SCHEME}{}", self.encode())
    }

    /// Accepts a link, a bare ticket, with any whitespace or case.
    pub fn parse(text: &str) -> Result<Ticket, NetError> {
        let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let body = match flat.get(..SCHEME.len()) {
            Some(p) if p.eq_ignore_ascii_case(SCHEME) => &flat[SCHEME.len()..],
            _ => flat.as_str(),
        };
        let body = body.trim_end_matches('/').to_ascii_uppercase();
        let bytes = BASE32_NOPAD.decode(body.as_bytes()).map_err(|_| NetError::Ticket("not a desigma ticket".into()))?;
        let wire: Wire = postcard::from_bytes(&bytes).map_err(|_| NetError::Ticket("not a desigma ticket".into()))?;
        if wire.v != TICKET_VERSION {
            return Err(NetError::Ticket("ticket from a different desigma version".into()));
        }
        Ok(Ticket { addr: wire.addr, password_required: wire.pw })
    }
}

impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_link())
    }
}

impl std::str::FromStr for Ticket {
    type Err = NetError;
    fn from_str(s: &str) -> Result<Self, NetError> {
        Ticket::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{SecretKey, TransportAddr};

    fn sample(pw: bool) -> Ticket {
        let id = SecretKey::generate().public();
        let addr = EndpointAddr::new(id).with_ip_addr("192.168.1.20:4242".parse().unwrap()).with_ip_addr("[::1]:4242".parse().unwrap());
        Ticket { addr, password_required: pw }
    }

    #[test]
    fn round_trip_link_and_bare() {
        for pw in [false, true] {
            let t = sample(pw);
            assert_eq!(Ticket::parse(&t.to_link()).unwrap(), t);
            assert_eq!(Ticket::parse(&t.encode()).unwrap(), t);
            assert!(t.addr.addrs.iter().any(|a| matches!(a, TransportAddr::Ip(_))));
        }
    }

    #[test]
    fn parses_whitespace_case_and_slash() {
        let t = sample(true);
        let link = t.to_link();
        let (a, b) = link.split_at(link.len() / 2);
        let messy = format!("  \n{}\r\n {}/ \t", a.to_ascii_uppercase().replace("DESIGMA://JOIN/", "desigma://join/"), b);
        assert_eq!(Ticket::parse(&messy).unwrap(), t);
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "desigma://join/", "hello world", "desigma://join/!!!!", "aaaa"] {
            assert!(Ticket::parse(s).is_err(), "{s:?}");
        }
    }
}
