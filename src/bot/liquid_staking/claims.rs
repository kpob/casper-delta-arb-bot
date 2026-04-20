use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use odra_cli::scenario::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClaim {
    /// Casper block time (ms since epoch) after which claim() can be called.
    pub claimable_from_ms: u64,
}

pub struct PendingClaims {
    pub(crate) claims: Vec<PendingClaim>,
    file_path: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl PendingClaims {
    /// Loads claims from a JSON file. Returns an empty list if the file is
    /// missing or unreadable.
    pub fn load(file_path: &str) -> Self {
        let claims = std::fs::read_to_string(file_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            claims,
            file_path: file_path.to_string(),
        }
    }

    /// Appends a claim and rewrites the file.
    pub fn add(&mut self, claim: PendingClaim) -> Result<(), Error> {
        self.claims.push(claim);
        self.persist()
    }

    /// Returns true if any claim's `claimable_from_ms` has passed.
    pub fn has_ready_claims(&self) -> bool {
        let now = now_ms();
        self.claims.iter().any(|c| c.claimable_from_ms <= now)
    }

    /// Removes all claims whose `claimable_from_ms` has passed and rewrites the
    /// file. Call this after a successful `staked_cspr.claim()` transaction.
    pub fn remove_ready(&mut self) -> Result<(), Error> {
        let now = now_ms();
        self.claims.retain(|c| c.claimable_from_ms > now);
        if self.file_path.is_empty() {
            return Ok(());
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), Error> {
        if self.file_path.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&self.claims).map_err(|e| Error::OdraError {
            message: format!("Failed to serialize claims: {e}"),
        })?;
        std::fs::write(&self.file_path, json).map_err(|e| Error::OdraError {
            message: format!("Failed to write claims file: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path() -> String {
        format!(
            "{}/test_pending_claims_{}.json",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        )
    }

    #[test]
    fn test_load_from_missing_file_returns_empty() {
        let claims = PendingClaims::load("/nonexistent/path/claims.json");
        assert_eq!(claims.claims.len(), 0);
    }

    #[test]
    fn test_add_persists_to_file_and_reloads() {
        let path = tmp_path();
        let mut claims = PendingClaims::load(&path);
        claims.add(PendingClaim { claimable_from_ms: 9_999_999_999_999 }).unwrap();

        let reloaded = PendingClaims::load(&path);
        assert_eq!(reloaded.claims.len(), 1);
        assert_eq!(reloaded.claims[0].claimable_from_ms, 9_999_999_999_999);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_has_ready_claims_when_claimable_from_is_in_past() {
        let claims = PendingClaims {
            claims: vec![PendingClaim { claimable_from_ms: 1_000 }],
            file_path: String::new(),
        };
        assert!(claims.has_ready_claims());
    }

    #[test]
    fn test_no_ready_claims_when_claimable_from_is_in_future() {
        let claims = PendingClaims {
            claims: vec![PendingClaim { claimable_from_ms: u64::MAX }],
            file_path: String::new(),
        };
        assert!(!claims.has_ready_claims());
    }

    #[test]
    fn test_remove_ready_removes_past_keeps_future() {
        let path = tmp_path();
        let mut claims = PendingClaims {
            claims: vec![
                PendingClaim { claimable_from_ms: 1_000 },       // past
                PendingClaim { claimable_from_ms: u64::MAX },    // future
            ],
            file_path: path.clone(),
        };
        claims.remove_ready().unwrap();
        assert_eq!(claims.claims.len(), 1);
        assert_eq!(claims.claims[0].claimable_from_ms, u64::MAX);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_remove_ready_persists_remaining_claims() {
        let path = tmp_path();
        let mut claims = PendingClaims {
            claims: vec![
                PendingClaim { claimable_from_ms: 1_000 },
                PendingClaim { claimable_from_ms: u64::MAX },
            ],
            file_path: path.clone(),
        };
        claims.remove_ready().unwrap();

        let reloaded = PendingClaims::load(&path);
        assert_eq!(reloaded.claims.len(), 1);
        assert_eq!(reloaded.claims[0].claimable_from_ms, u64::MAX);
        let _ = std::fs::remove_file(&path);
    }
}
