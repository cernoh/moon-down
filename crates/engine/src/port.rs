use std::net::TcpListener;

/// Return the first free localhost port in 6800-6899, or None if all busy.
pub fn pick_free_port() -> Option<u16> {
    for port in 6800..=6899u16 {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_free_port_in_range() {
        let p = pick_free_port().unwrap();
        assert!((6800..=6899).contains(&p));
    }
    #[test]
    fn first_free_is_lowest() {
        // Bind 6800, then pick should be 6801 if free.
        let _g = TcpListener::bind(("127.0.0.1", 6800)).ok();
        if _g.is_some() {
            let p = pick_free_port().unwrap();
            assert_eq!(p, 6801);
        }
    }
}
