//! Minimal XML tree for Huawei API responses.
//!
//! The firmware returns small documents (`<response>..</response>` or
//! `<error><code>..</code></error>`), some with nested lists (`Hosts/Host`,
//! `LTEBandList/LTEBand`). We build a generic tree once and query it, instead of
//! writing a bespoke parser per endpoint.

use crate::error::ApiError;
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: impl Into<String>) -> Self {
        Node {
            name: name.into(),
            text: String::new(),
            children: Vec::new(),
        }
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Depth-first search for the first node with this name.
    pub fn find(&self, name: &str) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        for c in &self.children {
            if let Some(found) = c.find(name) {
                return Some(found);
            }
        }
        None
    }

    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.find(name).map(|n| n.text.as_str())
    }

    /// Trimmed, non-empty text, treating the firmware's `" "` placeholder as absent.
    pub fn value(&self, name: &str) -> Option<&str> {
        let t = self.text_of(name)?.trim();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }
}

pub fn parse(src: &str) -> Result<Node, ApiError> {
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                stack.push(Node::new(name));
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let node = Node::new(name);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => {
                        root = Some(node);
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(top) = stack.last_mut() {
                    top.text
                        .push_str(&unescape(&String::from_utf8_lossy(e.as_ref())));
                }
            }
            Ok(Event::CData(e)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(e.as_ref()));
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => {
                            root = Some(node);
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(ApiError::Parse(format!("{e}"))),
        }
    }

    root.ok_or_else(|| ApiError::Parse("empty document".into()))
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after[..after.len().min(12)].find(';') else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix('#').and_then(|num| {
                if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else {
                    num.parse::<u32>().ok().and_then(char::from_u32)
                }
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_response() {
        let doc = "<?xml version=\"1.0\"?><response><rsrp>-96dBm</rsrp><band>1</band></response>";
        let n = parse(doc).unwrap();
        assert_eq!(n.name, "response");
        assert_eq!(n.value("rsrp"), Some("-96dBm"));
        assert_eq!(n.value("band"), Some("1"));
    }

    #[test]
    fn parses_nested_hosts() {
        let doc = "<response><Hosts><Host><HostName>tv</HostName><IpAddress>192.168.8.101;fe80::1</IpAddress></Host><Host><HostName>x</HostName></Host></Hosts><Count>2</Count></response>";
        let n = parse(doc).unwrap();
        let hosts: Vec<_> = n.find("Hosts").unwrap().children_named("Host").collect();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].value("HostName"), Some("tv"));
        assert_eq!(n.value("Count"), Some("2"));
    }

    #[test]
    fn parses_error_document() {
        let n = parse("<error><code>100003</code><message/></error>").unwrap();
        assert_eq!(n.name, "error");
        assert_eq!(n.value("code"), Some("100003"));
    }

    #[test]
    fn space_placeholder_is_absent() {
        let n = parse("<response><nrrsrp> </nrrsrp></response>").unwrap();
        assert_eq!(n.value("nrrsrp"), None);
    }

    #[test]
    fn numeric_entities_are_decoded() {
        // The firmware writes "11.0.2.21&#40;H329SP1C7002&#41;"
        let n = parse("<response><sw>11.0.2.21&#40;H329&#41;</sw></response>").unwrap();
        assert_eq!(n.value("sw"), Some("11.0.2.21(H329)"));
        let amp = parse("<response><a>x &amp; y &lt;z&gt;</a></response>").unwrap();
        assert_eq!(amp.value("a"), Some("x & y <z>"));
    }
}
