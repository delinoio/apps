// SPDX-License-Identifier: Apache-2.0
use crate::error::{Result, invalid};
pub fn url(raw: &str) -> Result<String> {
    let bad = || invalid("Invalid repository URL.");
    if raw.contains(['\\', '?', '#']) {
        return Err(bad());
    }
    let raw = if raw.contains("://") {
        raw.to_owned()
    } else {
        let at = raw.find('@').ok_or_else(bad)?;
        let colon = raw.find(':').ok_or_else(bad)?;
        if at == 0 || colon < at {
            return Err(bad());
        }
        format!("ssh://{}/{}", &raw[..colon], &raw[colon + 1..])
    };
    if raw
        .split_once("://")
        .is_none_or(|(_, rest)| rest.is_empty() || rest.starts_with('/'))
    {
        return Err(bad());
    }
    let u = url::Url::parse(&raw).map_err(|_| bad())?;
    if !matches!(u.scheme(), "https" | "ssh") {
        return Err(bad());
    }
    let host = u.host_str().filter(|h| !h.is_empty()).ok_or_else(bad)?;
    // SSH uses an opaque host parser. Canonicalize it with the same IDNA and
    // case rules as HTTPS so both transports share a repository identity.
    let canonical = url::Url::parse(&format!("https://{host}/")).map_err(|_| bad())?;
    let host = canonical.host_str().ok_or_else(bad)?;
    let host = match u.port() {
        Some(0) => return Err(bad()),
        Some(22) if u.scheme() == "ssh" => host.to_owned(),
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let rest = raw.split_once("://").ok_or_else(bad)?.1;
    let path = rest.find('/').map_or("", |i| rest[i..].trim_matches('/'));
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let pair = bytes.get(i + 1..i + 3).ok_or_else(bad)?;
        let h = std::str::from_utf8(pair).map_err(|_| bad())?;
        let c = u8::from_str_radix(h, 16).map_err(|_| bad())?;
        if c == b'/' || c == b'\\' {
            return Err(bad());
        }
        if c.is_ascii_alphanumeric() || b"-._~".contains(&c) {
            out.push(c);
        } else {
            out.extend_from_slice(format!("%{c:02X}").as_bytes());
        }
        i += 3;
    }
    let path = String::from_utf8(out).map_err(|_| bad())?;
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(bad());
    }
    let path = path.strip_suffix(".git").unwrap_or(&path);
    if path.is_empty() {
        return Err(bad());
    }
    Ok(format!("{host}/{path}"))
}
