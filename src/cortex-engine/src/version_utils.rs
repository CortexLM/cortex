//! Version utilities.
//!
//! Provides utilities for version parsing, comparison,
//! and semantic versioning operations.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Semantic version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemVer {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl SemVer {
    /// Create a new version.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Create a version from major only.
    pub const fn major_only(major: u32) -> Self {
        Self {
            major,
            minor: 0,
            patch: 0,
        }
    }

    /// Check if this is a major version (x.0.0).
    pub fn is_major(&self) -> bool {
        self.minor == 0 && self.patch == 0
    }

    /// Check if this is a minor version (x.y.0).
    pub fn is_minor(&self) -> bool {
        self.patch == 0
    }

    /// Check if this is a pre-release version (0.x.x).
    pub fn is_prerelease(&self) -> bool {
        self.major == 0
    }

    /// Increment major version.
    pub fn bump_major(&self) -> Self {
        Self {
            major: self.major + 1,
            minor: 0,
            patch: 0,
        }
    }

    /// Increment minor version.
    pub fn bump_minor(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
            patch: 0,
        }
    }

    /// Increment patch version.
    pub fn bump_patch(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch + 1,
        }
    }

    /// Check if compatible with another version.
    pub fn is_compatible(&self, other: &Self) -> bool {
        if self.major == 0 && other.major == 0 {
            // For 0.x.x versions, minor must match
            self.minor == other.minor
        } else {
            // For 1.0.0+, major must match
            self.major == other.major
        }
    }

    /// Check if this version satisfies a requirement.
    pub fn satisfies(&self, req: &VersionReq) -> bool {
        req.matches(self)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => match self.minor.cmp(&other.minor) {
                Ordering::Equal => self.patch.cmp(&other.patch),
                ord => ord,
            },
            ord => ord,
        }
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for SemVer {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim_start_matches('v');
        let parts: Vec<&str> = s.split('.').collect();

        if parts.is_empty() || parts.len() > 3 {
            return Err(VersionError::InvalidFormat);
        }

        let major = parts[0].parse().map_err(|_| VersionError::InvalidNumber)?;
        let minor = parts
            .get(1)
            .map(|s| s.parse())
            .transpose()
            .map_err(|_| VersionError::InvalidNumber)?
            .unwrap_or(0);
        let patch = parts
            .get(2)
            .map(|s| s.parse())
            .transpose()
            .map_err(|_| VersionError::InvalidNumber)?
            .unwrap_or(0);

        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl Default for SemVer {
    fn default() -> Self {
        Self::new(0, 0, 0)
    }
}

/// Version error.
#[derive(Debug, Clone)]
pub enum VersionError {
    /// Invalid format.
    InvalidFormat,
    /// Invalid number.
    InvalidNumber,
    /// Invalid operator.
    InvalidOperator,
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => write!(f, "Invalid version format"),
            Self::InvalidNumber => write!(f, "Invalid version number"),
            Self::InvalidOperator => write!(f, "Invalid version operator"),
        }
    }
}

impl std::error::Error for VersionError {}

/// Version requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionReq {
    /// Constraints.
    pub constraints: Vec<VersionConstraint>,
}

impl VersionReq {
    /// Create a new requirement.
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    /// Create an exact requirement.
    pub fn exact(version: SemVer) -> Self {
        Self {
            constraints: vec![VersionConstraint {
                op: VersionOp::Eq,
                version,
            }],
        }
    }

    /// Create a compatible requirement (^).
    pub fn compatible(version: SemVer) -> Self {
        Self {
            constraints: vec![VersionConstraint {
                op: VersionOp::Compatible,
                version,
            }],
        }
    }

    /// Create a tilde requirement (~).
    pub fn tilde(version: SemVer) -> Self {
        Self {
            constraints: vec![VersionConstraint {
                op: VersionOp::Tilde,
                version,
            }],
        }
    }

    /// Add constraint.
    pub fn with_constraint(mut self, constraint: VersionConstraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    /// Check if version matches requirement.
    pub fn matches(&self, version: &SemVer) -> bool {
        self.constraints.iter().all(|c| c.matches(version))
    }
}

impl Default for VersionReq {
    fn default() -> Self {
        Self::new()
    }
}

impl FromStr for VersionReq {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut constraints = Vec::new();

        for part in s.split(',') {
            let part = part.trim();
            constraints.push(part.parse()?);
        }

        Ok(Self { constraints })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .constraints
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        write!(f, "{}", parts.join(", "))
    }
}

/// Version constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionConstraint {
    /// Operator.
    pub op: VersionOp,
    /// Version.
    pub version: SemVer,
}

impl VersionConstraint {
    /// Check if version matches constraint.
    pub fn matches(&self, version: &SemVer) -> bool {
        match self.op {
            VersionOp::Eq => version == &self.version,
            VersionOp::Ne => version != &self.version,
            VersionOp::Lt => version < &self.version,
            VersionOp::Le => version <= &self.version,
            VersionOp::Gt => version > &self.version,
            VersionOp::Ge => version >= &self.version,
            VersionOp::Compatible => {
                version >= &self.version && version.is_compatible(&self.version)
            }
            VersionOp::Tilde => {
                version >= &self.version
                    && version.major == self.version.major
                    && version.minor == self.version.minor
            }
            VersionOp::Wildcard => true,
        }
    }
}

impl FromStr for VersionConstraint {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s == "*" {
            return Ok(Self {
                op: VersionOp::Wildcard,
                version: SemVer::default(),
            });
        }

        let (op, version_str) = if s.starts_with(">=") {
            (VersionOp::Ge, &s[2..])
        } else if s.starts_with("<=") {
            (VersionOp::Le, &s[2..])
        } else if s.starts_with("!=") {
            (VersionOp::Ne, &s[2..])
        } else if s.starts_with('^') {
            (VersionOp::Compatible, &s[1..])
        } else if s.starts_with('~') {
            (VersionOp::Tilde, &s[1..])
        } else if s.starts_with('>') {
            (VersionOp::Gt, &s[1..])
        } else if s.starts_with('<') {
            (VersionOp::Lt, &s[1..])
        } else if s.starts_with('=') {
            (VersionOp::Eq, &s[1..])
        } else {
            (VersionOp::Eq, s)
        };

        let version = version_str.trim().parse()?;
        Ok(Self { op, version })
    }
}

impl fmt::Display for VersionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.op {
            VersionOp::Eq => write!(f, "={}", self.version),
            VersionOp::Ne => write!(f, "!={}", self.version),
            VersionOp::Lt => write!(f, "<{}", self.version),
            VersionOp::Le => write!(f, "<={}", self.version),
            VersionOp::Gt => write!(f, ">{}", self.version),
            VersionOp::Ge => write!(f, ">={}", self.version),
            VersionOp::Compatible => write!(f, "^{}", self.version),
            VersionOp::Tilde => write!(f, "~{}", self.version),
            VersionOp::Wildcard => write!(f, "*"),
        }
    }
}

/// Version operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionOp {
    /// Equal (=).
    Eq,
    /// Not equal (!=).
    Ne,
    /// Less than (<).
    Lt,
    /// Less than or equal (<=).
    Le,
    /// Greater than (>).
    Gt,
    /// Greater than or equal (>=).
    Ge,
    /// Compatible (^).
    Compatible,
    /// Tilde (~).
    Tilde,
    /// Wildcard (*).
    Wildcard,
}

/// Version range.
#[derive(Debug, Clone)]
pub struct VersionRange {
    /// Minimum version (inclusive).
    pub min: Option<SemVer>,
    /// Maximum version (exclusive).
    pub max: Option<SemVer>,
}

impl VersionRange {
    /// Create a new range.
    pub fn new(min: Option<SemVer>, max: Option<SemVer>) -> Self {
        Self { min, max }
    }

    /// Create an unbounded range.
    pub fn any() -> Self {
        Self {
            min: None,
            max: None,
        }
    }

    /// Create a range from min only.
    pub fn from_min(min: SemVer) -> Self {
        Self {
            min: Some(min),
            max: None,
        }
    }

    /// Create a range up to max.
    pub fn up_to(max: SemVer) -> Self {
        Self {
            min: None,
            max: Some(max),
        }
    }

    /// Create a range between versions.
    pub fn between(min: SemVer, max: SemVer) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
        }
    }

    /// Check if version is in range.
    pub fn contains(&self, version: &SemVer) -> bool {
        if let Some(ref min) = self.min
            && version < min
        {
            return false;
        }
        if let Some(ref max) = self.max
            && version >= max
        {
            return false;
        }
        true
    }

    /// Check if ranges overlap.
    pub fn overlaps(&self, other: &Self) -> bool {
        // Check if either range is completely before the other
        if let (Some(my_max), Some(other_min)) = (&self.max, &other.min)
            && my_max <= other_min
        {
            return false;
        }
        if let (Some(my_min), Some(other_max)) = (&self.min, &other.max)
            && my_min >= other_max
        {
            return false;
        }
        true
    }

    /// Intersect with another range.
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        if !self.overlaps(other) {
            return None;
        }

        let min = match (&self.min, &other.min) {
            (Some(a), Some(b)) => Some(*a.max(b)),
            (Some(a), None) => Some(*a),
            (None, Some(b)) => Some(*b),
            (None, None) => None,
        };

        let max = match (&self.max, &other.max) {
            (Some(a), Some(b)) => Some(*a.min(b)),
            (Some(a), None) => Some(*a),
            (None, Some(b)) => Some(*b),
            (None, None) => None,
        };

        Some(Self { min, max })
    }
}

/// Compare versions.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let va: SemVer = a.parse().ok()?;
    let vb: SemVer = b.parse().ok()?;
    Some(va.cmp(&vb))
}

/// Get latest version from list.
pub fn latest<'a>(versions: &'a [&'a str]) -> Option<&'a str> {
    versions
        .iter()
        .filter_map(|v| v.parse::<SemVer>().ok().map(|ver| (*v, ver)))
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(s, _)| s)
}

/// Filter versions matching requirement.
pub fn filter_matching<'a>(versions: &'a [&str], req: &VersionReq) -> Vec<&'a str> {
    versions
        .iter()
        .filter(|v| {
            v.parse::<SemVer>()
                .map(|ver| req.matches(&ver))
                .unwrap_or(false)
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_new() {
        let v = SemVer::new(1, 2, 3);
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_semver_parse() {
        assert_eq!("1.2.3".parse::<SemVer>().unwrap(), SemVer::new(1, 2, 3));
        assert_eq!("v1.2.3".parse::<SemVer>().unwrap(), SemVer::new(1, 2, 3));
        assert_eq!("1.2".parse::<SemVer>().unwrap(), SemVer::new(1, 2, 0));
        assert_eq!("1".parse::<SemVer>().unwrap(), SemVer::new(1, 0, 0));
    }

    #[test]
    fn test_semver_display() {
        assert_eq!(SemVer::new(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn test_semver_compare() {
        assert!(SemVer::new(2, 0, 0) > SemVer::new(1, 9, 9));
        assert!(SemVer::new(1, 2, 0) > SemVer::new(1, 1, 9));
        assert!(SemVer::new(1, 2, 3) > SemVer::new(1, 2, 2));
        assert!(SemVer::new(1, 2, 3) == SemVer::new(1, 2, 3));
    }

    #[test]
    fn test_semver_bump() {
        let v = SemVer::new(1, 2, 3);
        assert_eq!(v.bump_major(), SemVer::new(2, 0, 0));
        assert_eq!(v.bump_minor(), SemVer::new(1, 3, 0));
        assert_eq!(v.bump_patch(), SemVer::new(1, 2, 4));
    }

    #[test]
    fn test_version_req_exact() {
        let req = VersionReq::exact(SemVer::new(1, 0, 0));
        assert!(req.matches(&SemVer::new(1, 0, 0)));
        assert!(!req.matches(&SemVer::new(1, 0, 1)));
    }

    #[test]
    fn test_version_req_compatible() {
        let req = VersionReq::compatible(SemVer::new(1, 2, 0));
        assert!(req.matches(&SemVer::new(1, 2, 0)));
        assert!(req.matches(&SemVer::new(1, 3, 0)));
        assert!(req.matches(&SemVer::new(1, 9, 9)));
        assert!(!req.matches(&SemVer::new(2, 0, 0)));
        assert!(!req.matches(&SemVer::new(1, 1, 0)));
    }

    #[test]
    fn test_version_req_parse() {
        let req: VersionReq = ">=1.0.0, <2.0.0".parse().unwrap();
        assert!(req.matches(&SemVer::new(1, 5, 0)));
        assert!(!req.matches(&SemVer::new(0, 9, 0)));
        assert!(!req.matches(&SemVer::new(2, 0, 0)));
    }

    #[test]
    fn test_version_range() {
        let range = VersionRange::between(SemVer::new(1, 0, 0), SemVer::new(2, 0, 0));
        assert!(range.contains(&SemVer::new(1, 5, 0)));
        assert!(!range.contains(&SemVer::new(0, 9, 0)));
        assert!(!range.contains(&SemVer::new(2, 0, 0)));
    }

    #[test]
    fn test_compare() {
        assert_eq!(compare("1.0.0", "2.0.0"), Some(Ordering::Less));
        assert_eq!(compare("2.0.0", "1.0.0"), Some(Ordering::Greater));
        assert_eq!(compare("1.0.0", "1.0.0"), Some(Ordering::Equal));
    }

    #[test]
    fn test_latest() {
        let versions = vec!["1.0.0", "2.0.0", "1.5.0", "0.9.0"];
        assert_eq!(latest(&versions), Some("2.0.0"));
    }

    #[test]
    fn test_filter_matching() {
        let versions = vec!["1.0.0", "1.5.0", "2.0.0", "2.5.0"];
        let req = VersionReq::compatible(SemVer::new(1, 0, 0));
        let matching = filter_matching(&versions, &req);
        assert_eq!(matching, vec!["1.0.0", "1.5.0"]);
    }
}
