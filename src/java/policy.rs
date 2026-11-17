use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JavaPolicyError {
    #[error("incompatible Java requirements")]
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaPolicy {
    pub majors: Vec<u32>,
    pub allow_newer: bool,
}

impl JavaPolicy {
    pub fn accepts(&self, major: u32) -> bool {
        self.majors.contains(&major)
            || (self.allow_newer && self.majors.iter().max().is_some_and(|max| major > *max))
    }

    pub fn intersect(&self, other: &Self) -> Result<Self, JavaPolicyError> {
        let mut seen = HashSet::new();
        let majors: Vec<u32> = self
            .majors
            .iter()
            .chain(&other.majors)
            .copied()
            .filter(|m| self.accepts(*m) && other.accepts(*m) && seen.insert(*m))
            .collect();

        if majors.is_empty() {
            return Err(JavaPolicyError::Incompatible);
        }

        Ok(Self {
            majors,
            allow_newer: self.allow_newer && other.allow_newer,
        })
    }
}
