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
    "ipaddress",
    "ssid",
    "wifissid",
    "wifikey",
    "wifiwpapsk",
    "wpapsk",
    "presharedkey",
    "password",
    // TR-069 credentials: an ACS URL names the operator's account and the
    // password is a password. Neither is a field a reader needs a value for.
    "tr069password",
    "tr069acsurl",
    "hostname",
    "actualname",
    "associatedssid",
    "fullname",
    "shortname",
    "numeric",
    // The SMS surfaces: the body, the sender and the service centre. H122-373
    // (capture of 2026-11-22) spells the sender `Phone` in one document and
    // `phone_number` in another, so the comparison strips the underscore.
    "phone",
    "phonenumber",
    "sca",
    "content",
    // Operator and SIM identity: the MCC/MNC pair, and the element name the
    // status document uses for the same thing.
    "mccmnc",
    "plmn",
    // The resolvers the LAN hands out. A custom pair names a provider or a
    // private resolver, and that is where the owner lives, not how the router
    // behaves. `lan_ip` is deliberately absent: it is the router's own address,
    // which every report prints anyway.
    "primarydns",
    "secondarydns",
    "primaryipv6dns",
    "secondaryipv6dns",
    "wandnsaddress",
    "wanipv6dnsaddress",
];

/// What an identity value is replaced with. Same word the report and the parsed
/// snapshot use, so one grep means "this was hidden" everywhere.
pub const HIDDEN: &str = "(hidden)";

/// A firmware is free to spell the same field `GuestSsid` or
/// `Ipv6IpAddress`; the fixed list would miss those. Suffix and substring rules
/// cover the families without pretending every `*name` is identity (`DeviceName`
/// is not).
fn is_identity(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    // `wan_dns_address` and `WanDNSAddress` are one field: an underscore is
    // punctuation between the words, not part of the name.
    let stripped;
    let key: &str = if lower.contains('_') {
        stripped = lower.replace('_', "");
        &stripped
    } else {
        &lower
    };
    IDENTITY_TAGS.contains(&key)
        || lower.ends_with("ssid")
        || lower.ends_with("ipaddress")
        || lower.ends_with("psk")
        || lower.ends_with("password")
        // H122-373, capture of 2026-11-22: the radios report as
        // `WifiMacAddrWl0` and `WifiMacAddrWl1`, so a fixed list can never
        // finish this family; the tag carries the index.
        || lower.contains("macaddr")
}

/// The open tag as it should be written: `<`, the head, `>`. An identity
/// attribute has its value replaced, because H122-373 (capture of 2026-11-22)
/// carries a MAC in `macaddr="..."` on a host row as often as it carries one in
/// an element. The rule follows the attribute NAME, never the element that owns
/// it.
fn open_tag(head: &str, is_open: bool) -> String {
    let mut out = String::with_capacity(head.len() + 2);
    out.push('<');
    if is_open && head.contains('=') {
        out.push_str(&mask_attributes(head));
    } else {
        out.push_str(head);
    }
    out.push('>');
    out
}

fn mask_attributes(head: &str) -> String {
    let mut out = String::with_capacity(head.len());
    let mut rest = head;
    while let Some(eq) = rest.find('=') {
        let before = &rest[..eq];
        // The name is the last word before the `=`.
        let (prefix, name) = match before.rfind(|c: char| c.is_whitespace() || c == '/') {
            Some(cut) => (&before[..cut + 1], &before[cut + 1..]),
            None => ("", before),
        };
        out.push_str(prefix);
        let hide = !name.is_empty() && is_identity(name);
        out.push_str(name);
        out.push('=');
        let after = &rest[eq + 1..];
        // A quoted value ends at its own quote; an unquoted one at the first
        // space. A hidden value is written in double quotes, the word every
        // other hidden field uses.
        let (value, tail) = if let Some(body) = after.strip_prefix('"') {
            match body.find('"') {
                Some(end) => (&after[..end + 2], &after[end + 2..]),
                None => (after, ""),
            }
        } else if let Some(body) = after.strip_prefix('\'') {
            match body.find('\'') {
                Some(end) => (&after[..end + 2], &after[end + 2..]),
                None => (after, ""),
            }
        } else {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if hide {
            out.push('"');
            out.push_str(HIDDEN);
            out.push('"');
        } else {
            out.push_str(value);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Where the element `name` ends, from `from`. The close tag does not have to
/// repeat the open tag's letter case; the case rule is unverified against a
/// device, so the search compares the name case-insensitively rather than
/// trusting the open tag's spelling.
fn find_close(raw: &str, from: usize, name: &str) -> Option<usize> {
    let mut at = from;
    while let Some(off) = raw[at..].find("</") {
        let tag = at + off;
        let rest = &raw[tag + 2..];
        let got: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'))
            .collect();
        let ends = rest[got.len()..]
            .chars()
            .next()
            .is_some_and(|c| c == '>' || c.is_whitespace());
        if got.eq_ignore_ascii_case(name) && ends {
            return Some(tag);
        }
        at = tag + 2;
    }
    None
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
            // An unclosed element falls through: keep the tag, drop nothing.
            if let Some(value_end) = find_close(raw, close + 1, &name) {
                if let Some(gt) = raw[value_end..].find('>') {
                    let end_at = value_end + gt + 1;
                    out.push_str(&open_tag(head, is_open));
                    out.push_str(HIDDEN);
                    // The close tag goes back exactly as the router spelled it.
                    out.push_str(&raw[value_end..end_at]);
                    i = end_at;
                    continue;
                }
            }
        }
        out.push_str(&open_tag(head, is_open));
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
<WifiMacAddrWl0>02:00:00:00:0A:10</WifiMacAddrWl0>
<WifiMacAddrWl1>02:00:00:00:0A:11</WifiMacAddrWl1>
<WanIPAddress>203.0.113.77</WanIPAddress>
<Mccmnc>00101</Mccmnc>
<plmn>001-01</plmn>
<Sca>+12055550100</Sca>
<phone_number>+12055550101</phone_number>
<PrimaryDns>203.0.113.53</PrimaryDns>
<SecondaryDns>203.0.113.54</SecondaryDns>
<wan_dns_address>203.0.113.55</wan_dns_address>
<rsrp>-95dBm</rsrp>
<Hosts><Host><HostName>living-room-tv</HostName><IpAddress>192.168.8.101</IpAddress></Host></Hosts>
</response>"#;

    #[test]
    fn tr069_credentials_never_survive_the_scrub() {
        let raw = "<response><Tr069Enable>1</Tr069Enable><Tr069ACSURL>http://acs.example/x</Tr069ACSURL><Tr069Password>hunter2</Tr069Password></response>";
        let out = crate::scrub::scrub_xml(raw);
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("acs.example"), "{out}");
        assert!(
            out.contains("<Tr069Enable>1</Tr069Enable>"),
            "field names stay"
        );
    }

    /// The promise: a dump a maintainer can read in public. Names survive,
    /// values do not.
    #[test]
    fn identity_values_are_replaced_and_every_field_name_survives() {
        let out = scrub_xml(RAW);
        for secret in [
            "MOCKSERIAL0001",
            "000000000000000",
            "02:00:00:00:0A:01",
            "02:00:00:00:0A:10",
            "02:00:00:00:0A:11",
            "203.0.113.77",
            "living-room-tv",
            "00101",
            "001-01",
            "+12055550100",
            "+12055550101",
            "203.0.113.53",
            "203.0.113.54",
            "203.0.113.55",
        ] {
            assert!(!out.contains(secret), "{secret} survived:\n{out}");
        }
        for name in [
            "<SerialNumber>",
            "<Imei>",
            "<MacAddress1>",
            "<WifiMacAddrWl0>",
            "<WifiMacAddrWl1>",
            "<WanIPAddress>",
            "<HostName>",
            "<Mccmnc>",
            "<Sca>",
            "<PrimaryDns>",
            "<wan_dns_address>",
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

    /// A close tag in another case must not win the scrub: the search matches
    /// the name, not one spelling of it.
    #[test]
    fn a_mixed_case_close_tag_is_still_hidden() {
        let raw = "<response><Imei>000000000000000</imei><SerialNumber>MOCKSERIAL0002</SerialNumber ></response>";
        let out = scrub_xml(raw);
        assert!(!out.contains("000000000000000"), "{out}");
        assert!(!out.contains("MOCKSERIAL0002"), "{out}");
        assert!(out.contains("<Imei>(hidden)</imei>"), "{out}");
        assert!(
            out.contains("<SerialNumber>(hidden)</SerialNumber >"),
            "the close tag goes back as the router spelled it: {out}"
        );
    }

    /// A MAC is as often an attribute as an element; the rule follows the
    /// attribute name and leaves an attribute that identifies nothing.
    #[test]
    fn identity_attributes_are_hidden() {
        let raw = r#"<response><Host macaddr="02:00:00:00:0A:21" idx="3"><HostName>tv</HostName></Host><Entry ssid='HomeNet' count="4"/></response>"#;
        let out = scrub_xml(raw);
        assert!(!out.contains("02:00:00:00:0A:21"), "{out}");
        assert!(!out.contains("HomeNet"), "{out}");
        assert!(out.contains(r#"macaddr="(hidden)""#), "{out}");
        assert!(out.contains(r#"idx="3""#), "{out}");
        assert!(out.contains(r#"count="4""#), "{out}");
        assert!(out.contains("<HostName>(hidden)</HostName>"), "{out}");
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

    /// A wifi key is the worst thing a "shareable" dump can carry: it is the
    /// network's password, not just a name.
    #[test]
    fn wifi_keys_never_survive() {
        let raw = r#"<response><WifiSsid>HomeNet</WifiSsid><WifiKey>Secret123</WifiKey><WifiWpaPsk>Secret123</WifiWpaPsk><DeviceName>Mock-CPE</DeviceName></response>"#;
        let out = scrub_xml(raw);
        assert!(!out.contains("Secret123"), "{out}");
        assert!(out.contains("<WifiKey>(hidden)</WifiKey>"), "{out}");
        // The device name identifies nothing a maintainer cannot see.
        assert!(
            out.contains("<DeviceName>Mock-CPE</DeviceName>"),
            "device name must stay: {out}"
        );
    }

    /// The real host-list tags are `ActualName` and `IpAddress`, not the two
    /// spellings the fixed list happened to carry.
    #[test]
    fn real_host_list_shapes_are_covered() {
        let raw = r#"<response><Hosts><Host><HostName>tv</HostName><ActualName>living-room-tv</ActualName><IpAddress>192.168.8.101;fe80::1</IpAddress><MacAddress>02:11:22:33:44:55</MacAddress><AssociatedSsid>HomeNet</AssociatedSsid></Host></Hosts></response>"#;
        let out = scrub_xml(raw);
        for secret in [
            "living-room-tv",
            "192.168.8.101",
            "fe80::1",
            "HomeNet",
            "02:11:22:33:44:55",
        ] {
            assert!(!out.contains(secret), "{secret} survived:\n{out}");
        }
        for name in [
            "<ActualName>",
            "<IpAddress>",
            "<AssociatedSsid>",
            "<HostName>",
        ] {
            assert!(out.contains(name), "{name} dropped:\n{out}");
        }
    }

    /// The operator name says where a person lives; a new firmware that spells
    /// the tag `GuestSsid` must still be covered.
    #[test]
    fn operator_identity_and_new_ssid_spellings_are_hidden() {
        let raw = "<response><FullName>OperatorNet</FullName><ShortName>OPN</ShortName><Numeric>00101</Numeric><GuestSsid>HomeNet</GuestSsid></response>";
        let out = scrub_xml(raw);
        for s in ["OperatorNet", "OPN", "HomeNet"] {
            assert!(!out.contains(s), "{out}");
        }
        assert!(out.contains("<Numeric>(hidden)</Numeric>"), "{out}");
    }
}
