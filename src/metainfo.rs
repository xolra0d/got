use crate::{
    bencoding::{BencodeString, ConvError, Value},
    error::Error,
    url::Url,
};
use chrono::{DateTime, TimeZone, Utc};
use core::net;
use std::{collections::HashMap, path::PathBuf, str::Utf8Error};

fn remove_mandatory<'a>(
    info: &mut HashMap<BencodeString<'a>, Value<'a>>,
    var: &str,
) -> Result<Value<'a>, Error> {
    info.remove(var.as_bytes())
        .ok_or(Error::MissingField(String::from(var)))
}

pub fn strings_to_path(raw: &[BencodeString<'_>]) -> Result<PathBuf, Utf8Error> {
    let mut path = PathBuf::new();
    for part in raw {
        path.push(str::from_utf8(part.inner_ref())?);
    }
    Ok(path)
}

#[derive(Clone, Debug)]
pub struct FileDesc {
    pub length: i64,              // file size
    pub md5sum: Option<[u8; 16]>, // 32-char hex string of md5sum of the file.
    pub path: PathBuf,            // path, split by '/'
}

impl Files {
    fn parse<'a>(info: &mut HashMap<BencodeString<'a>, Value<'a>>) -> Result<Self, Error> {
        let name = remove_mandatory(info, "name")?.as_string()?;
        let path = strings_to_path(&[name])?;

        if let Some(single_length) = info.remove(b"length".as_slice()) {
            let length = single_length.as_integer()?;

            let md5sum = match info.remove(b"md5sum".as_slice()) {
                None => None,
                Some(sum) => {
                    let sum = sum.as_string()?;
                    sum.inner_ref().try_into().ok()
                }
            };

            return Ok(Self {
                path: "./".into(),
                files: vec![FileDesc {
                    path,
                    length,
                    md5sum,
                }],
            });
        }

        let raw_files = remove_mandatory(info, "files")?.as_list()?;
        let mut files = Vec::with_capacity(raw_files.len());

        for file in raw_files {
            let mut file = file.as_dictionary()?;
            let path = remove_mandatory(&mut file, "path")?;
            let paths = path
                .as_list()?
                .into_iter()
                .map(|x| x.as_string())
                .collect::<Result<Vec<BencodeString<'a>>, ConvError>>()?;
            let path = strings_to_path(&paths)?;

            let length = remove_mandatory(&mut file, "length")?.as_integer()?;

            let md5sum = match file.remove(b"md5sum".as_slice()) {
                None => None,
                Some(sum) => {
                    let sum = sum.as_string()?;
                    sum.inner_ref().try_into().ok()
                }
            };
            files.push(FileDesc {
                length,
                md5sum,
                path,
            });
        }
        Ok(Self { path, files })
    }
}

#[derive(Clone, Debug)]
pub struct Files {
    pub path: PathBuf,
    pub files: Vec<FileDesc>,
}

#[derive(Clone, Debug)]
pub struct Info<'a> {
    pub piece_length: i64,      // length of one piece
    pub pieces: &'a [[u8; 20]], // concatenation of all 20-byte SHA-1. one per piece.
    _private: Option<i64>,      // ignore
    pub files: Files,
}

impl<'a> Info<'a> {
    fn new(mut info: HashMap<BencodeString<'a>, Value<'a>>) -> Result<Self, Error> {
        let piece_length = remove_mandatory(&mut info, "piece length")?.as_integer()?;

        let pieces = remove_mandatory(&mut info, "pieces")?.as_string()?;
        let pieces = pieces.inner_ref();
        let (pieces, remainder) = pieces.as_chunks::<20>();
        if !remainder.is_empty() {
            return Err(Error::InvalidField(format!(
                "Field `pieces` len must be multiple of 20. Instead got a reminder: {:?}",
                remainder
            )));
        }

        let files = Files::parse(&mut info)?;

        Ok(Info {
            piece_length,
            pieces,
            files,
            _private: None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct MetaInfo<'a> {
    pub info: Info<'a>,
    pub announce: Url<'a>,                    // URL of tracker
    pub announce_list: Vec<net::SocketAddr>,  // List of other trackers
    pub creation_date: Option<DateTime<Utc>>, // UNIX EPOCH of created by
    pub comment: Option<&'a str>,             // description
    pub created_by: Option<&'a str>,          // program used to gen .torrent
    pub encoding: Option<&'a str>,            // encoding used to gen `pieces` part of the `info`.

    pub raw_info: HashMap<BencodeString<'a>, Value<'a>>,
}

// fn parse_announce_list(list: Option<Value>) -> Option<Vec<Vec<BencodeString>>> {
//     let Some(Value::List(val)) = list else {
//         return None;
//     };
//
//     val.into_iter()
//         .map(|inner| match inner {
//             Value::List(inner) => inner
//                 .into_iter()
//                 .map(|inner| match inner {
//                     Value::String(s) => Some(s),
//                     _ => None,
//                 })
//                 .collect::<Option<Vec<_>>>(),
//             _ => None,
//         })
//         .collect()
// }

impl<'a> MetaInfo<'a> {
    pub fn new(mut meta: HashMap<BencodeString<'a>, Value<'a>>) -> Result<Self, Error> {
        let info = remove_mandatory(&mut meta, "info")?.as_dictionary()?;

        let raw_info = info.clone();

        let info = Info::new(info)?;

        let announce = remove_mandatory(&mut meta, "announce")?.as_string()?;
        let announce = Url::parse(announce.as_str()?)?;

        // let announce_list = parse_announce_list(meta.remove(b"announce-list".as_slice()));

        let _announce_list = remove_mandatory(&mut meta, "announce-list")?.as_list()?;

        let creation_date = meta
            .remove(b"creation date".as_slice())
            .map(|raw| raw.as_integer())
            .transpose()?
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single());

        let comment = match meta.remove(b"comment".as_slice()) {
            Some(Value::String(comment)) => Some(comment.as_str()?),
            _ => None,
        };

        let created_by = match meta.remove(b"created by".as_slice()) {
            Some(Value::String(created_by)) => Some(created_by.as_str()?),
            _ => None,
        };

        let encoding = match meta.remove(b"encoding".as_slice()) {
            Some(Value::String(encoding)) => Some(encoding.as_str()?),
            _ => None,
        };

        log::debug!("Other fields are present in metainfo: {:?}", meta);

        Ok(Self {
            info,
            announce,
            announce_list: Vec::new(),
            creation_date,
            comment,
            created_by,
            encoding,
            raw_info,
        })
    }
}
