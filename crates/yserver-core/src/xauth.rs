//! Shared decoder for the Xau (`~/.Xauthority`) binary record format.
//!
//! Each record is a sequence of big-endian length-prefixed fields:
//! `family(2)  addr_len(2) addr  num_len(2) num  name_len(2) name  data_len(2) data`.
//! Both the host-X11 client reader (selects one cookie by family/number)
//! and the server-side authorizer (loads every MIT cookie) decode through
//! here; record *selection* stays at the call site.

pub const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XauthRecord {
    pub family: u16,
    pub address: Vec<u8>,
    pub number: Vec<u8>,
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}

/// Decode records until one fails to parse (truncated/short tail),
/// keeping every fully-decoded record before that point. Mirrors
/// `XauReadAuth` looping until it returns NULL.
pub fn parse_records(bytes: &[u8]) -> Vec<XauthRecord> {
    let mut cursor = 0usize;
    let mut out = Vec::new();
    while cursor < bytes.len() {
        let Some(family) = read_be_u16(bytes, &mut cursor) else {
            break;
        };
        let Some(address) = read_field(bytes, &mut cursor) else {
            break;
        };
        let Some(number) = read_field(bytes, &mut cursor) else {
            break;
        };
        let Some(name) = read_field(bytes, &mut cursor) else {
            break;
        };
        let Some(data) = read_field(bytes, &mut cursor) else {
            break;
        };
        out.push(XauthRecord {
            family,
            address,
            number,
            name,
            data,
        });
    }
    out
}

fn read_be_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let end = *cursor + 2;
    let value = u16::from_be_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_field(bytes: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    let len = read_be_u16(bytes, cursor)? as usize;
    let end = *cursor + len;
    let value = bytes.get(*cursor..end)?.to_vec();
    *cursor = end;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-encode one Xau record. Field framing per the format above.
    fn rec(family: u16, addr: &[u8], num: &[u8], name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&family.to_be_bytes());
        for f in [addr, num, name, data] {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    #[test]
    fn parses_two_records() {
        // family 256 = FamilyLocal; address = hostname; number = display.
        let mut bytes = rec(256, b"bee", b"0", MIT_MAGIC_COOKIE.as_bytes(), &[1u8; 16]);
        bytes.extend(rec(
            256,
            b"bee",
            b"1",
            MIT_MAGIC_COOKIE.as_bytes(),
            &[2u8; 16],
        ));
        let recs = parse_records(&bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].number, b"0");
        assert_eq!(recs[0].data, vec![1u8; 16]);
        assert_eq!(recs[1].number, b"1");
        assert_eq!(recs[1].name, MIT_MAGIC_COOKIE.as_bytes());
    }

    #[test]
    fn stops_at_truncated_tail_keeping_prior_records() {
        let mut bytes = rec(256, b"bee", b"0", MIT_MAGIC_COOKIE.as_bytes(), &[7u8; 16]);
        bytes.extend_from_slice(&[0x01, 0x00, 0x00]); // a dangling, incomplete record
        let recs = parse_records(&bytes);
        assert_eq!(
            recs.len(),
            1,
            "valid leading record kept, truncated tail dropped"
        );
        assert_eq!(recs[0].data, vec![7u8; 16]);
    }
}
