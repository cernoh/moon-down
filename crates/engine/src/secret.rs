use rand::Rng;

/// Generate a fresh ephemeral secret (32 alphanumeric chars).
pub fn generate_secret() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| {
            let idx = rng.gen_range(0..CHARS.len());
            CHARS[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_is_fresh_and_alphanumeric() {
        let a = generate_secret();
        let b = generate_secret();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_alphanumeric()));
    }
}
