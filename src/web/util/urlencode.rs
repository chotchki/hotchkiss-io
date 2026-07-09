//! Minimal RFC-3986 percent-encoding for a QUERY VALUE — enough to keep a
//! search term or path with spaces/specials from breaking the URL it rides in.
//! Encodes everything outside the unreserved set. (The greylist challenge page
//! keeps its own deliberately-minimal encoder with a documented precondition —
//! this is the one to reach for by default.)

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
