use crate::{
    bencoding::{ConvError, DecodeError},
    url::UrlParseError,
};
use std::{
    fmt::{self, Display},
    io, str,
};

#[derive(Debug)]
pub enum Error {
    // metainfo
    MissingField(String),
    InvalidField(String),

    Utf8Error(str::Utf8Error),

    ReadFile(io::Error),
    WriteFile(io::Error),
    SocketBind(io::Error),

    /// mod udp
    SocketConnect(String),

    Connect,
    Announce,

    ConvError(ConvError),
    DecodeError(DecodeError),
    UrlParseError(String),

    Internal(String),
}

impl From<str::Utf8Error> for Error {
    fn from(value: str::Utf8Error) -> Self {
        Self::Utf8Error(value)
    }
}

impl From<ConvError> for Error {
    fn from(value: ConvError) -> Self {
        Self::ConvError(value)
    }
}

impl From<DecodeError> for Error {
    fn from(value: DecodeError) -> Self {
        Self::DecodeError(value)
    }
}

impl From<UrlParseError> for Error {
    fn from(value: UrlParseError) -> Self {
        Self::UrlParseError(value.0)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "Missing field: {field}"),
            Self::InvalidField(field) => write!(f, "Invalid field: {field}"),
            Self::Utf8Error(error) => write!(f, "UTF-8 error: {error}"),
            Self::ReadFile(error) => write!(f, "Failed to read file: {error}"),
            Self::WriteFile(error) => write!(f, "Failed to write file: {error}"),
            Self::SocketBind(error) => write!(f, "Failed to bind socket: {error}"),
            Self::Connect => write!(f, "failed to get connection id"),
            Self::Announce => write!(f, "failed to get announce"),
            Self::ConvError(error) => write!(f, "Bencode conversion error: {error}"),
            Self::DecodeError(error) => write!(f, "Bencode decode error: {error}"),
            Self::UrlParseError(error) => write!(f, "URL parse error: {error}"),
            Self::Internal(error) => write!(f, "Internal error: {error}"),
            Self::SocketConnect(error) => write!(f, "Failed to connect to socket: {error}"),
        }
    }
}
