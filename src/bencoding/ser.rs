use crate::bencoding::{BencodeString, Value};
use core::fmt;
use std::{collections::HashMap, hash::Hash, io};

pub trait ToBencode {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error>;
}

impl ToBencode for i64 {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        out.write_all(b"i")?;
        let mut buf = fmt::NumBuffer::new();
        out.write_all(self.format_into(&mut buf).as_bytes())?;
        out.write_all(b"e")
    }
}

impl ToBencode for BencodeString<'_> {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        let mut buf = fmt::NumBuffer::new();
        out.write_all(self.0.len().format_into(&mut buf).as_bytes())?;
        out.write_all(b":")?;
        out.write_all(self.0)
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for BencodeString<'a> {
    fn from(value: &'a [u8; N]) -> Self {
        BencodeString(value.as_slice())
    }
}

impl<'a> From<&'a [u8]> for BencodeString<'a> {
    fn from(value: &'a [u8]) -> Self {
        BencodeString(value)
    }
}

impl PartialEq for BencodeString<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for BencodeString<'_> {}

impl Hash for BencodeString<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialOrd for BencodeString<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BencodeString<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(other.0)
    }
}

impl<T: ToBencode> ToBencode for Vec<T> {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        out.write_all(b"l")?;
        for el in self {
            el.to_bencode(out)?;
        }
        out.write_all(b"e")
    }
}

impl ToBencode for HashMap<BencodeString<'_>, Value<'_>> {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        let mut keys: Vec<&BencodeString> = self.keys().collect();
        keys.sort_unstable();

        out.write_all(b"d")?;

        for k in keys {
            let Some(val) = self.get(k) else {
                panic!("KEY disappeared from dictionary! Key: {k:?}");
            };
            k.to_bencode(out)?;
            val.to_bencode(out)?;
        }

        out.write_all(b"e")
    }
}

impl ToBencode for Value<'_> {
    fn to_bencode(&self, out: &mut impl io::Write) -> Result<(), io::Error> {
        match self {
            Self::Integer(val) => val.to_bencode(out),
            Self::String(val) => val.to_bencode(out),
            Self::List(val) => val.to_bencode(out),
            Self::Dictionary(val) => val.to_bencode(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_strings() {
        let empty = BencodeString(&[]);
        let mut buf: Vec<u8> = Vec::new();
        empty.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"0:");

        let spam = BencodeString::from(b"spam");
        let mut buf: Vec<u8> = Vec::new();
        spam.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"4:spam");

        let empty = BencodeString::from(b"Testing 123. Testing");
        let mut buf: Vec<u8> = Vec::new();
        empty.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"20:Testing 123. Testing");
    }

    #[test]
    #[allow(clippy::zero_prefixed_literal)]
    fn encodes_integers() {
        let mut buf: Vec<u8> = Vec::new();
        0.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"i0e");

        let mut buf: Vec<u8> = Vec::new();
        (-123123214532593).to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"i-123123214532593e");

        let mut buf: Vec<u8> = Vec::new();
        (057274942974).to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"i57274942974e");

        let mut buf: Vec<u8> = Vec::new();
        (123123214532593).to_bencode(&mut buf).unwrap();
        (06324624728428).to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"i123123214532593ei6324624728428e");
    }

    #[test]
    fn encodes_lists() {
        let spam = vec![BencodeString::from(b"spam")];
        let mut buf = Vec::new();
        spam.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"l4:spame");

        let spam = vec![BencodeString::from(b"spam"), BencodeString::from(b"eggs")];
        let mut buf = Vec::new();
        spam.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"l4:spam4:eggse");

        let spam: Vec<BencodeString> = Vec::new();
        let mut buf = Vec::new();
        spam.to_bencode(&mut buf).unwrap();
        assert_eq!(buf, b"le");
    }
}
