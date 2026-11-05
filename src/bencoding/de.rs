use crate::bencoding::{BencodeString, Value};
use std::{collections::HashMap, fmt::Display};

const MAX_I64_LENGTH: usize = i64::MAX.ilog10() as usize + 2;

#[derive(Debug, Clone)]
pub struct DecodeError {
    pos: usize,
    expected: &'static str,
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "expected: {}, at pos: {}",
            self.expected, self.pos
        ))
    }
}

pub fn from_bencode<'a>(bytes: &'a [u8]) -> Result<Value<'a>, DecodeError> {
    _from_bencode(bytes, 0).map(|x| x.0)
}

fn _from_bencode<'a>(
    bytes: &'a [u8],
    current_index: usize,
) -> Result<(Value<'a>, usize), DecodeError> {
    match bytes[current_index] {
        b'i' => parse_integer(bytes, current_index + 1).map(|(val, i)| (Value::Integer(val), i)),
        b'0'..=b'9' => parse_string(bytes, current_index).map(|(val, i)| (Value::String(val), i)),
        b'l' => parse_list(bytes, current_index + 1).map(|(val, i)| (Value::List(val), i)),
        b'd' => {
            parse_dictionary(bytes, current_index + 1).map(|(val, i)| (Value::Dictionary(val), i))
        }
        _ => Err(DecodeError {
            pos: current_index,
            expected: "i|0-9|l|d",
        }),
    }
}

fn parse_integer(bytes: &[u8], mut current_index: usize) -> Result<(i64, usize), DecodeError> {
    let mut res: i64 = 0;
    for _ in 0..(MAX_I64_LENGTH.min(bytes.len() - current_index)) {
        match bytes[current_index] {
            b'0'..=b'9' => {
                let n = bytes[current_index] - b'0';
                if res == 0 && n == 0 {
                    return Err(DecodeError {
                        pos: current_index,
                        expected: "0-9",
                    });
                }
                res = res * 10 + n as i64;
            }
            b'-' => {
                if res < 0 {
                    return Err(DecodeError {
                        pos: current_index,
                        expected: "-",
                    });
                }
                res *= -1;
            }
            b'e' => return Ok((res, current_index + 1)),
            _ => {
                return Err(DecodeError {
                    pos: current_index,
                    expected: "0-9|-|e",
                });
            }
        }
        current_index += 1;
    }

    if res < 0 {
        Err(DecodeError {
            pos: current_index,
            expected: "0-9|e",
        })
    } else {
        Err(DecodeError {
            pos: current_index,
            expected: "0-9|-|e",
        })
    }
}

fn _parse_i64_for_string(
    bytes: &[u8],
    mut current_index: usize,
) -> Result<(usize, usize), DecodeError> {
    let mut res = 0;
    for _ in 0..(MAX_I64_LENGTH.min(bytes.len() - current_index)) {
        match bytes[current_index] {
            b'0'..=b'9' => {
                let n = bytes[current_index] - b'0';
                res = res * 10 + n as usize;
            }
            b':' => return Ok((res, current_index + 1)),
            _ => {
                return Err(DecodeError {
                    pos: current_index,
                    expected: "0-9|:",
                });
            }
        }
        current_index += 1;
    }

    Err(DecodeError {
        pos: current_index,
        expected: "0-9|:",
    })
}

fn parse_string<'a>(
    bytes: &'a [u8],
    current_index: usize,
) -> Result<(BencodeString<'a>, usize), DecodeError> {
    let (num, start) = _parse_i64_for_string(bytes, current_index)?;

    if bytes.len() - start < num {
        return Err(DecodeError {
            pos: start + num,
            expected: "string",
        });
    }

    Ok((BencodeString(&bytes[start..(start + num)]), start + num))
}

fn parse_list<'a>(
    bytes: &'a [u8],
    mut current_index: usize,
) -> Result<(Vec<Value<'a>>, usize), DecodeError> {
    let mut res = Vec::new();
    loop {
        if bytes[current_index] == b'e' {
            return Ok((res, current_index + 1));
        }
        let (val, i) = _from_bencode(bytes, current_index)?;
        res.push(val);
        current_index = i;
    }
}

fn parse_dictionary<'a>(
    bytes: &'a [u8],
    mut current_index: usize,
) -> Result<(HashMap<BencodeString<'a>, Value<'a>>, usize), DecodeError> {
    let mut res = HashMap::new();
    loop {
        if bytes[current_index] == b'e' {
            return Ok((res, current_index + 1));
        }
        let (key, i) = parse_string(bytes, current_index)?;
        let (val, i) = _from_bencode(bytes, i)?;

        res.insert(key, val);
        current_index = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integers() {
        let bytes = b"i34356789e";
        let res = _from_bencode(bytes, 0).unwrap();
        assert_eq!(res.0, Value::Integer(34356789));
        assert_eq!(res.1, bytes.len());
    }

    #[test]
    fn parses_strings() {
        let bytes = b"10:HelloWorld";
        let res = _from_bencode(bytes, 0).unwrap();
        assert_eq!(res.0, Value::String(BencodeString(b"HelloWorld")));
        assert_eq!(res.1, bytes.len());

        let bytes = b"0:";
        let res = _from_bencode(bytes, 0).unwrap();
        assert_eq!(res.0, Value::String(BencodeString(b"")));
        assert_eq!(res.1, bytes.len());
    }

    #[test]
    fn parses_lists() {
        let bytes = b"l10:HelloWorldi1234ee";
        let (val, next) = _from_bencode(bytes, 0).unwrap();
        assert_eq!(
            val,
            Value::List(vec![
                Value::String(BencodeString(b"HelloWorld")),
                Value::Integer(1234)
            ])
        );
        assert_eq!(next, bytes.len());
    }

    #[test]
    fn parses_dicts() {
        let bytes = b"d10:HelloWorldi1234e4:Porki1235ee";
        let (val, next) = _from_bencode(bytes, 0).unwrap();

        assert_eq!(
            val,
            Value::Dictionary(HashMap::from([
                (BencodeString(b"HelloWorld"), Value::Integer(1234)),
                (BencodeString(b"Pork"), Value::Integer(1235))
            ]))
        );
        assert_eq!(next, bytes.len());
    }
}
