use std::{convert::TryFrom, error::Error, fmt, str::FromStr};

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Method {
    GET,
    HEAD,
    POST,
    PUT,
}

impl Method {
    pub fn as_str(&self) -> &str {
        match self {
            Method::GET => "GET",
            Method::HEAD => "HEAD",
            Method::POST => "POST",
            Method::PUT => "PUT",
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Method::GET => b"GET",
            Method::HEAD => b"HEAD",
            Method::POST => b"POST",
            Method::PUT => b"PUT",
        }
    }
}

impl TryFrom<&str> for Method {
    type Error = InvalidMethod;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "GET" => Ok(Method::GET),
            "HEAD" => Ok(Method::HEAD),
            "POST" => Ok(Method::POST),
            "PUT" => Ok(Method::PUT),
            _ => Err(InvalidMethod),
        }
    }
}

impl TryFrom<&[u8]> for Method {
    type Error = InvalidMethod;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            b"GET" => Ok(Method::GET),
            b"HEAD" => Ok(Method::HEAD),
            b"POST" => Ok(Method::POST),
            b"PUT" => Ok(Method::PUT),
            _ => Err(InvalidMethod),
        }
    }
}

impl FromStr for Method {
    type Err = InvalidMethod;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "GET" => Ok(Method::GET),
            "HEAD" => Ok(Method::HEAD),
            "POST" => Ok(Method::POST),
            "PUT" => Ok(Method::PUT),
            _ => Err(InvalidMethod),
        }
    }
}

impl AsRef<str> for Method {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'a> PartialEq<&'a Method> for Method {
    #[inline]
    fn eq(&self, other: &&'a Method) -> bool {
        self == *other
    }
}

impl<'a> PartialEq<Method> for &'a Method {
    #[inline]
    fn eq(&self, other: &Method) -> bool {
        *self == other
    }
}

impl PartialEq<str> for Method {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<Method> for str {
    #[inline]
    fn eq(&self, other: &Method) -> bool {
        self == other.as_ref()
    }
}

impl<'a> PartialEq<&'a str> for Method {
    #[inline]
    fn eq(&self, other: &&'a str) -> bool {
        self.as_ref() == *other
    }
}

impl<'a> PartialEq<Method> for &'a str {
    #[inline]
    fn eq(&self, other: &Method) -> bool {
        *self == other.as_ref()
    }
}

impl fmt::Debug for Method {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl fmt::Display for Method {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.write_str(self.as_ref())
    }
}

impl Default for Method {
    #[inline]
    fn default() -> Method {
        Method::GET
    }
}

impl<'a> From<&'a Method> for Method {
    #[inline]
    fn from(t: &'a Method) -> Self {
        *t
    }
}

#[derive(Debug)]
pub struct InvalidMethod;

impl fmt::Display for InvalidMethod {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl Error for InvalidMethod {
    fn description(&self) -> &str {
        "Invalid HTTP method"
    }
}
