#[derive(Clone, Debug)]
pub enum UrlScheme {
    Udp,
    // Http,
}

#[derive(Clone, Debug)]
pub struct Url<'a> {
    scheme: UrlScheme,
    socket: &'a str,
}

#[derive(Clone, Debug)]
pub struct UrlParseError(pub String);

impl<'a> Url<'a> {
    pub fn parse(raw: &'a str) -> Result<Self, UrlParseError> {
        if raw.len() <= 8 {
            return Err(UrlParseError(String::from("Url is too small")));
        }
        if raw.starts_with("udp://") {
            let (_, domain) = raw.split_at(6);

            Ok(Url {
                scheme: UrlScheme::Udp,
                socket: domain,
            })
        } else if raw.starts_with("http://") {
            Err(UrlParseError(String::from(
                "Unsupported url scheme: http or https",
            )))
        } else {
            Err(UrlParseError(String::from("Unsupported url scheme")))
        }
    }

    pub fn scheme(&self) -> &UrlScheme {
        &self.scheme
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }
}
