use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Compatible if same major. While we're 0.x, also require same minor —
    /// the SemVer convention is that 0.x minor bumps can break.
    pub fn is_compatible_with(self, engine: Version) -> bool {
        if self.major != engine.major {
            return false;
        }
        if self.major == 0 {
            return self.minor == engine.minor;
        }
        true
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum VersionParseError {
    #[error("version {0:?} must have three dot-separated parts (major.minor.patch)")]
    WrongShape(String),
    #[error("version part {part:?} in {input:?} is not a u16")]
    BadNumber { input: String, part: String },
}

impl FromStr for Version {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(VersionParseError::WrongShape(s.to_string()));
        }
        let parse = |p: &str| {
            p.parse::<u16>().map_err(|_| VersionParseError::BadNumber {
                input: s.to_string(),
                part: p.to_string(),
            })
        };
        Ok(Version {
            major: parse(parts[0])?,
            minor: parse(parts[1])?,
            patch: parse(parts[2])?,
        })
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Version::from_str(&s).map_err(serde::de::Error::custom)
    }
}

pub fn engine_version() -> Version {
    Version::from_str(env!("CARGO_PKG_VERSION"))
        .expect("CARGO_PKG_VERSION should be parseable as Version")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dotted_string() {
        let v: Version = "0.1.0".parse().unwrap();
        assert_eq!(v, Version::new(0, 1, 0));
    }

    #[test]
    fn rejects_wrong_shape() {
        assert!("0.1".parse::<Version>().is_err());
        assert!("0.1.0.0".parse::<Version>().is_err());
        assert!("nope".parse::<Version>().is_err());
    }

    #[test]
    fn compatibility_in_0_x_requires_same_minor() {
        let engine = Version::new(0, 1, 0);
        assert!(Version::new(0, 1, 0).is_compatible_with(engine));
        assert!(Version::new(0, 1, 5).is_compatible_with(engine));
        assert!(!Version::new(0, 2, 0).is_compatible_with(engine));
        assert!(!Version::new(1, 0, 0).is_compatible_with(engine));
    }

    #[test]
    fn compatibility_in_1_x_only_requires_major() {
        let engine = Version::new(1, 4, 0);
        assert!(Version::new(1, 0, 0).is_compatible_with(engine));
        assert!(Version::new(1, 9, 9).is_compatible_with(engine));
        assert!(!Version::new(2, 0, 0).is_compatible_with(engine));
    }

    #[test]
    fn round_trips_via_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
        struct Holder {
            v: Version,
        }
        let h = Holder {
            v: Version::new(0, 1, 0),
        };
        let s = toml::to_string(&h).unwrap();
        assert!(s.contains("\"0.1.0\""));
        let back: Holder = toml::from_str(&s).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn engine_version_parses() {
        let v = engine_version();
        assert!(v.major <= 99);
    }
}
