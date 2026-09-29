use std::net::TcpListener;

/// Return the first free localhost port in 6800-6899, or None if all busy.
pub fn pick_free_port() -> Option<u16> {
    pick_free_port_in(6800..=6899)
}

/// Same scan against an explicit range. Kept separate so tests can use a range
/// nothing else on the machine or in this test binary touches.
pub fn pick_free_port_in(range: std::ops::RangeInclusive<u16>) -> Option<u16> {
    for port in range {
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
        // Own range: 6800-6899 is shared with every other test in this binary
        // and with any real aria2c on the machine, so scanning it in parallel
        // makes "lowest" unobservable.
        let base = 6930u16;
        let _held = TcpListener::bind(("127.0.0.1", base)).expect("bind base port");
        let p = pick_free_port_in(base..=base + 9).unwrap();
        assert_eq!(p, base + 1, "must skip the held port and take the next one");
        // with nothing held, the range start itself is free
        drop(_held);
        assert_eq!(pick_free_port_in(base..=base + 9).unwrap(), base);
    }
}
