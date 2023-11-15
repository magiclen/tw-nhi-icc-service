use std::{
    error::Error,
    fmt,
    fmt::{Display, Formatter},
    num::ParseIntError,
    string::FromUtf8Error,
};

#[derive(Debug)]
pub struct NHICardParseError;

impl Display for NHICardParseError {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("不是正確的健保卡")
    }
}

impl Error for NHICardParseError {}

impl From<FromUtf8Error> for NHICardParseError {
    #[inline]
    fn from(_: FromUtf8Error) -> Self {
        NHICardParseError
    }
}

impl From<ParseIntError> for NHICardParseError {
    #[inline]
    fn from(_: ParseIntError) -> Self {
        NHICardParseError
    }
}

#[derive(Debug)]
pub enum NHICardError {
    PCSCError(pcsc::Error),
    ParseError(NHICardParseError),
}

impl Display for NHICardError {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            NHICardError::PCSCError(err) => Display::fmt(&err, f),
            NHICardError::ParseError(err) => Display::fmt(&err, f),
        }
    }
}

impl Error for NHICardError {}

impl From<pcsc::Error> for NHICardError {
    #[inline]
    fn from(err: pcsc::Error) -> Self {
        NHICardError::PCSCError(err)
    }
}

impl From<NHICardParseError> for NHICardError {
    #[inline]
    fn from(err: NHICardParseError) -> Self {
        NHICardError::ParseError(err)
    }
}
