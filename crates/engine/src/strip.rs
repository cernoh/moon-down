/// Strip `user:pass@` credentials from a URI for stored records / logs.
/// Only strips if `://` is present and an `@` appears before the next `/`.
pub fn strip_credentials(uri: &str) -> String {
    // Find scheme delimiter
    let scheme_end = match uri.find("://") {
        Some(i) => i + 3,
        None => return uri.to_string(),
    };
    let after = &uri[scheme_end..];
    // Find '@' before first '/' or '?' or end
    let slash = after.find('/').unwrap_or(after.len());
    let q = after.find('?').unwrap_or(after.len());
    let host_end = slash.min(q);
    let at = match after[..host_end].find('@') {
        Some(p) => p,
        None => return uri.to_string(),
    };
    // Remove credentials: keep scheme + after '@'
    let mut out = String::with_capacity(uri.len() - (at + 1));
    out.push_str(&uri[..scheme_end]);
    out.push_str(&after[at + 1..]);
    out
}

pub fn strip_many(uris: &[String]) -> Vec<String> {
    uris.iter().map(|u| strip_credentials(u)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips_user_pass() {
        assert_eq!(
            strip_credentials("https://user:pass@example.com/file.zip"),
            "https://example.com/file.zip"
        );
        assert_eq!(
            strip_credentials("https://user@example.com/file"),
            "https://example.com/file"
        );
    }
    #[test]
    fn leaves_no_creds_unchanged() {
        assert_eq!(
            strip_credentials("https://example.com/file"),
            "https://example.com/file"
        );
        assert_eq!(strip_credentials("magnet:?xt=foo"), "magnet:?xt=foo");
        assert_eq!(strip_credentials("/local/path"), "/local/path");
    }
    #[test]
    fn strips_with_port_and_path() {
        assert_eq!(
            strip_credentials("http://u:p@example.com:8080/a/b?x=1"),
            "http://example.com:8080/a/b?x=1"
        );
    }
}
