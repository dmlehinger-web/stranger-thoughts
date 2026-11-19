use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdError;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                if !valid_id(&value) {
                    return Err(IdError);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

fn valid_id(value: &str) -> bool {
    let len = value.len();
    (1..=128).contains(&len)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.')
}

id_newtype!(WriterId);
id_newtype!(ReaderId);
id_newtype!(MatchId);
id_newtype!(UserId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_whitespace() {
        assert!(WriterId::new("").is_err());
        assert!(WriterId::new("has space").is_err());
        assert!(ReaderId::new("ok-id_1").is_ok());
    }
}
