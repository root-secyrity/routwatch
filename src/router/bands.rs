//! Band-mask arithmetic. The firmware uses bit (band-1) in a 64-bit mask, and
//! the "all bands" value differs per model - it must be read from the device,
//! never hardcoded (the Electron app hardcodes 0x3FFFFFFF and silently drops
//! bands 38-43).

use crate::xml::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandDef {
    pub band: Option<u8>,
    pub label: String,
    pub value: u64,
}

/// The exact XML payload `net/net-mode` expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandPayload {
    pub network_mode: String,
    pub network_band: String,
    pub lte_band: String,
    pub network_option: Option<String>,
}

impl BandPayload {
    pub fn xml(&self) -> String {
        let mut body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><NetworkMode>{}</NetworkMode><NetworkBand>{}</NetworkBand><LTEBand>{}</LTEBand>",
            self.network_mode, self.network_band, self.lte_band
        );
        if let Some(opt) = &self.network_option {
            body.push_str(&format!("<NetworkOption>{opt}</NetworkOption>"));
        }
        body.push_str("</request>");
        body
    }
}

/// Map the friendly CLI/TUI spelling to the firmware's code.
pub fn mode_code(mode: &str) -> Option<&'static str> {
    match mode.to_ascii_lowercase().as_str() {
        "auto" | "00" => Some("00"),
        "4g" | "lte" | "03" => Some("03"),
        "5g" | "nr" | "08" => Some("08"),
        _ => None,
    }
}

pub fn option_code(option: &str) -> Option<&'static str> {
    match option.to_ascii_lowercase().as_str() {
        "nsa" | "0" => Some("0"),
        "sa" | "1" => Some("1"),
        "both" | "sa+nsa" | "2" => Some("2"),
        _ => None,
    }
}

/// "1,3,5" / "b1 b3" -> [1, 3, 5]
pub fn parse_list(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for part in s.split([',', ' ', '+']) {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        let digits = t.trim_start_matches(['B', 'b']);
        let band: u8 = digits.parse().map_err(|_| format!("not a band: {t:?}"))?;
        if !(1..=64).contains(&band) {
            return Err(format!("band out of range 1..64: {band}"));
        }
        if !out.contains(&band) {
            out.push(band);
        }
    }
    if out.is_empty() {
        return Err("no bands given".into());
    }
    Ok(out)
}

/// Build the payload for a mode + selection. For Auto the NetworkBand is the
/// device's own all-bands value, not a hardcoded constant.
pub fn resolve(
    mode_code: &str,
    selected: &[u8],
    all_bands_hex: Option<&str>,
    option: Option<&str>,
) -> BandPayload {
    let lte = to_hex(mask_from_bands(selected));
    let network_band = if mode_code == "00" {
        all_bands_hex.unwrap_or(&lte).to_string()
    } else {
        lte.clone()
    };
    BandPayload {
        network_mode: mode_code.to_string(),
        network_band,
        lte_band: lte,
        network_option: if mode_code == "08" {
            option.map(str::to_string).or_else(|| Some("0".into()))
        } else {
            None
        },
    }
}

pub fn mask_from_bands(bands: &[u8]) -> u64 {
    bands
        .iter()
        .fold(0u64, |acc, b| acc | (1u64 << (b.saturating_sub(1)) as u32))
}

pub fn bands_from_mask(mask: u64) -> Vec<u8> {
    (1..=64u8)
        .filter(|b| mask & (1u64 << (b - 1) as u32) != 0)
        .collect()
}

pub fn to_hex(mask: u64) -> String {
    format!("{mask:X}")
}

pub fn from_hex(s: &str) -> Option<u64> {
    let t = s.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u64::from_str_radix(t, 16).ok()
}

/// Read the device's own supported-band list and its "all bands" mask.
pub fn from_net_mode_list(n: &Node) -> (Vec<BandDef>, Option<u64>) {
    let mut defs = Vec::new();
    let mut all = None;

    if let Some(list) = n.find("LTEBandList") {
        for entry in list.children_named("LTEBand") {
            let name = entry.value("Name").unwrap_or("").to_string();
            let value = entry.value("Value").and_then(from_hex);
            if name.eq_ignore_ascii_case("all bands") {
                if all.is_none() {
                    all = value;
                }
                continue;
            }
            for band in parse_bc_list(&name) {
                defs.push(BandDef {
                    band: Some(band),
                    label: format!("B{band}"),
                    value: 1u64 << (band - 1) as u32,
                });
            }
        }
    }
    defs.sort_by_key(|d| d.band.unwrap_or(0));
    defs.dedup_by_key(|d| d.band);
    (defs, all)
}

/// "LTE BC1/LTE BC3/LTE BC20" -> [1, 3, 20]
fn parse_bc_list(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let is_bc = (bytes[i] == b'B' || bytes[i] == b'b')
            && (bytes[i + 1] == b'C' || bytes[i + 1] == b'c');
        if is_bc {
            i += 2;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > start {
                if let Ok(b) = name[start..i].parse::<u8>() {
                    if (1..=64).contains(&b) {
                        out.push(b);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    /// Scrubbed capture: the real device output with names, MACs, IPs
    /// and identifiers replaced. Kept inline so no private data ships.
    const NETMODE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<response>
<AccessList>
<Access>00</Access>
<Access>08</Access>
<Access>03</Access>
</AccessList>
<BandList>
<Band>
<Name>All bands</Name>
<Value>3fffffff</Value>
</Band>
</BandList>
<LTEBandList>
<LTEBand>
<Name>LTE BC1&#x2F;LTE BC3&#x2F;LTE BC5&#x2F;LTE BC7&#x2F;LTE BC8&#x2F;LTE BC20&#x2F;LTE BC28&#x2F;LTE BC32&#x2F;LTE BC34&#x2F;LTE BC38&#x2F;LTE BC39&#x2F;LTE BC40&#x2F;LTE BC41&#x2F;LTE BC42&#x2F;LTE BC43</Name>
<Value>7e2880800d5</Value>
</LTEBand>
<LTEBand>
<Name>All bands</Name>
<Value>7fffffffffffffff</Value>
</LTEBand>
</LTEBandList>
</response>"#;

    #[test]
    fn parses_the_real_device_list() {
        let doc = parse(NETMODE_XML).unwrap();
        let (defs, all) = from_net_mode_list(&doc);
        assert_eq!(defs.len(), 15);
        assert!(
            defs.iter().any(|d| d.band == Some(7)),
            "B7 missing from the app's old constant list"
        );
        assert_eq!(all, from_hex("7fffffffffffffff"));
    }

    #[test]
    fn payload_for_4g_matches_firmware_shape() {
        let p = resolve("03", &[1, 3, 5], Some("7fffffffffffffff"), None);
        assert_eq!(p.lte_band, "15");
        assert_eq!(p.network_band, "15");
        assert_eq!(p.network_option, None);
        assert!(p.xml().starts_with("<?xml"));
        assert!(p.xml().contains("<NetworkMode>03</NetworkMode>"));
        assert!(p.xml().contains("<LTEBand>15</LTEBand>"));
        assert!(!p.xml().contains("NetworkOption"));
    }

    #[test]
    fn auto_uses_the_device_all_bands_mask() {
        let p = resolve("00", &[1, 3], Some("7fffffffffffffff"), None);
        assert_eq!(p.network_band, "7fffffffffffffff");
        assert_eq!(p.lte_band, "5");
    }

    #[test]
    fn five_g_carries_the_nr_option() {
        let p = resolve("08", &[1], None, Some("1"));
        assert_eq!(p.network_option.as_deref(), Some("1"));
        assert!(p.xml().contains("<NetworkOption>1</NetworkOption>"));
        // default when the caller gives nothing
        assert_eq!(
            resolve("08", &[1], None, None).network_option.as_deref(),
            Some("0")
        );
    }

    #[test]
    fn parses_cli_spellings() {
        assert_eq!(parse_list("1,3,5").unwrap(), vec![1, 3, 5]);
        assert_eq!(parse_list("b1 b3 b3").unwrap(), vec![1, 3]);
        assert!(parse_list("abc").is_err());
        assert!(parse_list("99").is_err());
        assert!(parse_list("").is_err());
        assert_eq!(mode_code("4G"), Some("03"));
        assert_eq!(mode_code("auto"), Some("00"));
        assert_eq!(option_code("sa+nsa"), Some("2"));
    }

    #[test]
    fn current_mask_round_trips() {
        // The live router reports LTEBand 88080095 -> B1,B3,B5,B8,B20,B28,B32
        let mask = from_hex("88080095").unwrap();
        assert_eq!(bands_from_mask(mask), vec![1, 3, 5, 8, 20, 28, 32]);
        assert_eq!(mask_from_bands(&[1, 3, 5, 8, 20, 28, 32]), mask);
    }

    #[test]
    fn sixty_four_bit_all_bands_is_not_truncated() {
        let mask = from_hex("7fffffffffffffff").unwrap();
        let bands = bands_from_mask(mask);
        assert_eq!(bands.len(), 63);
        assert!(bands.contains(&43));
        assert_ne!(mask, 0x3FFF_FFFF);
    }

    #[test]
    fn reads_device_list_and_all_bands() {
        let doc = parse(
            "<response><LTEBandList>\
             <LTEBand><Name>LTE BC1/LTE BC3/LTE BC5/LTE BC7/LTE BC8/LTE BC20/LTE BC28/LTE BC32/LTE BC34/LTE BC38/LTE BC39/LTE BC40/LTE BC41/LTE BC42/LTE BC43</Name><Value>7e2880800d5</Value></LTEBand>\
             <LTEBand><Name>All bands</Name><Value>7fffffffffffffff</Value></LTEBand>\
             </LTEBandList></response>",
        )
        .unwrap();
        let (defs, all) = from_net_mode_list(&doc);
        assert_eq!(defs.len(), 15);
        assert!(
            defs.iter().any(|d| d.band == Some(7)),
            "B7 is supported by the router"
        );
        assert_eq!(all, from_hex("7fffffffffffffff"));
    }
}
