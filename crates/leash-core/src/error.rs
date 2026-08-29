use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    Empty(&'static str),
    InvalidCharacter(&'static str),
    NonFinite(&'static str),
    OutOfRange(&'static str),
    Zero(&'static str),
    Overflow(&'static str),
    TimeReversed,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(name) => write!(formatter, "{name} cannot be empty"),
            Self::InvalidCharacter(name) => {
                write!(formatter, "{name} contains an invalid character")
            }
            Self::NonFinite(name) => write!(formatter, "{name} must be finite"),
            Self::OutOfRange(name) => write!(formatter, "{name} is outside its valid range"),
            Self::Zero(name) => write!(formatter, "{name} must be non-zero"),
            Self::Overflow(name) => write!(formatter, "{name} overflowed"),
            Self::TimeReversed => formatter.write_str("monotonic time moved backwards"),
        }
    }
}

impl std::error::Error for DomainError {}
