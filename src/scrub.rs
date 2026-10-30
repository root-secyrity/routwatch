//! Values that identify a person, a SIM or a network, removed from raw firmware
//! text while every field name stays.
//!
//! `report::redacted` works on the parsed snapshot and is enough for a model that
//! is close to one we already know. A different vendor needs the raw shapes -
//! which tags exist, in what order, with what kind of value - and that is what
//! this keeps. The names are the interesting part for a parser writer; the
//! IMEI never is.

/// Tags whose text is identity. Compared case-insensitively; a firmware that
/// spells `IMEI` and one that spells `Imei` are the same field.
pub const IDENTITY_TAGS: &[&str] = &[
    "imei",
    "imsi",
    "iccid",
    "msisdn",
    "serialnumber",
    "macaddress",
    "macaddress1",
    "macaddress2",
    "wanipaddress",
    "wanipv6address",
    "lanipaddress",
    "ssid",
    "wifissid",
    "hostname",
];

/// What an identity value is replaced with. Same word the report and the parsed
/// snapshot use, so one grep means "this was hidden" everywhere.
pub const HIDDEN: &str = "(hidden)";

fn is_identity(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    IDENTITY_TAGS.contains(&lower.as_str())
}

/// Replace the *text* of every identity element, leaving the tag names, the
/// attributes, the order and every other value exactly as the router sent them.
pub fn scrub_xml(raw: &str) -> String {
    if !raw.contains('<') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < raw.len() {
        // Copy everything that is not the start of a tag.
        if bytes[i] != b'<' {
            let next = raw[i..].find('<').map(|o| i + o).unwrap_or(raw.len());
            out.push_str(&raw[i..next]);
            i = next;
            continue;
        }
        // A tag: find its end, read the name.
        let Some(close_rel) = raw[i..].find('>') else {
            out.push_str(&raw[i..]);
            break;
        };
        let close = i + close_rel;
        let head = &raw[i + 1..close];
        let name: String = head
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == ':')
            .collect();
        let self_closing = head.ends_with('/');
        let is_open = !head.starts_with('/') && !head.starts_with('!') && !head.starts_with('?');

        if is_open && !self_closing && is_identity(&name) {
            let end_tag = format!("</{name}>");
            // An unclosed element falls through: keep the tag, drop nothing.
            if let Some(rel) = raw[close + 1..].find(&end_tag) {
                let value_end = close + 1 + rel;
                out.push_str(&raw[i..close + 1]);
                out.push_str(HIDDEN);
                out.push_str(&end_tag);
                i = value_end + end_tag.len();
                continue;
            }
        }
        out.push_str(&raw[i..close + 1]);
        i = close + 1;
    }
    out
}

/// Field names an XML document reports, nested with dots (`Host.IpAddress`).
/// Values never leave this function.
pub fn field_names(raw: &str) -> Vec<String> {
    let Ok(node) = crate::xml::parse(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    fn walk(node: &crate::xml::Node, prefix: &str, out: &mut Vec<String>) {
        for child in &node.children {
            let path = if prefix.is_empty() {
                child.name.clone()
            } else {
                format!("{prefix}.{}", child.name)
            };
            if child.children.is_empty() {
                if !out.contains(&path) {
                    out.push(path);
                }
            } else {
                walk(child, &path, out);
            }
        }
    }
    walk(&node, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = r#"<response>
<DeviceName>Mock-CPE</DeviceName>
<SerialNumber>MOCKSERIAL0001</SerialNumber>
<Imei>000000000000000</Imei>
<MacAddress1>02:00:00:00:0A:01</MacAddress1>
<WanIPAddress>203.0.113.77</WanIPAddress>
<rsrp>-95dBm</rsrp>
<Hosts><Host><HostName>living-room-tv</HostName><IpAddress>192.168.8.101</IpAddress></Host></Hosts>
</response>"#;

    /// The promise: a dump a maintainer can read in public. Names survive,
    /// values do not.
    #[test]
    fn identity_values_are_replaced_and_every_field_name_survives() {
        let out = scrub_xml(RAW);
        for secret in [
            "MOCKSERIAL0001",
            "000000000000000",
            "02:00:00:00:0A:01",
            "203.0.113.77",
            "living-room-tv",
        ] {
            assert!(!out.contains(secret), "{secret} survived:\n{out}");
        }
        for name in [
            "<SerialNumber>",
            "<Imei>",
            "<MacAddress1>",
            "<WanIPAddress>",
            "<HostName>",
        ] {
            assert!(out.contains(name), "{name} was dropped:\n{out}");
        }
        assert!(out.contains(HIDDEN));
        // A value that identifies nothing is untouched: the parser needs it.
        assert!(out.contains("<rsrp>-95dBm</rsrp>"), "{out}");
        assert!(out.contains("<DeviceName>Mock-CPE</DeviceName>"), "{out}");
    }

    /// Firmwares disagree on spelling; both have to be caught.
    #[test]
    fn identity_tags_are_matched_regardless_of_case() {
        let out = scrub_xml(
            "<response><IMEI>123456789012345</IMEI><IMSI>310150123456789</IMSI></response>",
        );
        assert!(!out.contains("123456789012345"), "{out}");
        assert!(!out.contains("310150123456789"), "{out}");
        assert!(out.contains("<IMEI>"), "{out}");
    }

    /// A tag with attributes is still the same field.
    #[test]
    fn attributes_do_not_hide_an_identity_tag() {
        let out =
            scrub_xml(r#"<response><MacAddress idx="1">AA:BB:CC:DD:EE:FF</MacAddress></response>"#);
        assert!(!out.contains("AA:BB:CC:DD:EE:FF"), "{out}");
        assert!(out.contains(r#"<MacAddress idx="1">"#), "{out}");
    }

    /// Everything else passes through byte for byte, including a value that
    /// merely looks like a tag.
    #[test]
    fn plain_text_is_left_alone() {
        let raw = "<response><note>a &lt;tag&gt; here</note><empty></empty></response>";
        assert_eq!(scrub_xml(raw), raw);
        assert_eq!(scrub_xml("no xml at all"), "no xml at all");
    }

    /// The field inventory is what a second vendor needs: names, nested, no values.
    #[test]
    fn field_names_are_listed_without_their_values() {
        let names = field_names(RAW);
        assert!(names.contains(&"SerialNumber".to_string()), "{names:?}");
        assert!(
            names.contains(&"Hosts.Host.HostName".to_string()),
            "{names:?}"
        );
        assert!(!names.join(",").contains("MOCKSERIAL0001"));
        assert!(!names.join(",").contains("living-room-tv"));
    }
}
