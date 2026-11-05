mod de;
mod ser;

pub use de::{DecodeError, from_bencode};
pub use ser::ToBencode;

use std::{borrow::Borrow, collections::HashMap, fmt::Display, str};

#[derive(Debug, Clone)]
pub struct BencodeString<'a>(&'a [u8]);

#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    Integer(i64),
    String(BencodeString<'a>),
    List(Vec<Value<'a>>),
    Dictionary(HashMap<BencodeString<'a>, Value<'a>>),
}

#[derive(Debug, Clone)]
pub enum ValueType {
    Integer,
    String,
    List,
    Dictionary,
}

#[derive(Debug, Clone)]
pub struct ConvError {
    expected: ValueType,
    recv: ValueType,
}

impl ValueType {
    const INTEGER: &str = "integer";
    const STRING: &str = "string";
    const LIST: &str = "list";
    const DICTIONARY: &str = "dictionary";

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Integer => Self::INTEGER,
            Self::String => Self::STRING,
            Self::List => Self::LIST,
            Self::Dictionary => Self::DICTIONARY,
        }
    }
}

impl Display for ConvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "expected: {}, received: {}",
            self.expected.as_str(),
            self.recv.as_str()
        ))
    }
}

impl<'a> Value<'a> {
    pub fn get_type(&self) -> ValueType {
        match self {
            Value::Integer(_) => ValueType::Integer,
            Value::String(_) => ValueType::String,
            Value::List(_) => ValueType::List,
            Value::Dictionary(_) => ValueType::Dictionary,
        }
    }

    // Returns either a value or it's true type.
    pub fn as_string(self) -> Result<BencodeString<'a>, ConvError> {
        match self {
            Value::String(res) => Ok(res),
            _ => Err(ConvError {
                recv: self.get_type(),
                expected: ValueType::String,
            }),
        }
    }

    // Returns either a value or it's true type.
    pub fn as_integer(self) -> Result<i64, ConvError> {
        match self {
            Value::Integer(res) => Ok(res),
            _ => Err(ConvError {
                recv: self.get_type(),
                expected: ValueType::Integer,
            }),
        }
    }

    // Returns either a value or it's true type.
    pub fn as_list(self) -> Result<Vec<Value<'a>>, ConvError> {
        match self {
            Value::List(res) => Ok(res),
            _ => Err(ConvError {
                recv: self.get_type(),
                expected: ValueType::List,
            }),
        }
    }

    // Returns either a value or it's true type.
    pub fn as_dictionary(self) -> Result<HashMap<BencodeString<'a>, Value<'a>>, ConvError> {
        match self {
            Value::Dictionary(res) => Ok(res),
            _ => Err(ConvError {
                recv: self.get_type(),
                expected: ValueType::Dictionary,
            }),
        }
    }
}

impl PartialEq<&[u8]> for BencodeString<'_> {
    fn eq(&self, other: &&[u8]) -> bool {
        self.0 == *other
    }
}
impl PartialEq<BencodeString<'_>> for [u8] {
    fn eq(&self, other: &BencodeString) -> bool {
        self == other.0
    }
}

impl PartialEq<BencodeString<'_>> for &[u8] {
    fn eq(&self, other: &BencodeString) -> bool {
        *self == other.0
    }
}

impl Borrow<[u8]> for BencodeString<'_> {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> BencodeString<'a> {
    pub fn as_str(&self) -> Result<&'a str, std::str::Utf8Error> {
        str::from_utf8(&self.0)
    }
    pub fn inner_ref(&self) -> &'a [u8] {
        self.0
    }
}

// impl PartialEq<[u8]> for BencodeString<'_> {
//     fn eq(&self, other: &[u8]) -> bool {
//         self.0.as_slice() == other
//     }
// }
// impl<const N: usize> PartialEq<[u8; N]> for BencodeString {
//     fn eq(&self, other: &[u8; N]) -> bool {
//         self.0.as_slice() == other.as_slice()
//     }
// }
//
// impl<const N: usize> PartialEq<&[u8; N]> for BencodeString {
//     fn eq(&self, other: &&[u8; N]) -> bool {
//         self.0.as_slice() == other.as_slice()
//     }
// }
//

//
// impl<const N: usize> PartialEq<BencodeString> for [u8; N] {
//     fn eq(&self, other: &BencodeString) -> bool {
//         self.as_slice() == other.0.as_slice()
//     }
// }
//
// impl<const N: usize> PartialEq<BencodeString> for &[u8; N] {
//     fn eq(&self, other: &BencodeString) -> bool {
//         self.as_slice() == other.0.as_slice()
//     }
// }
//
