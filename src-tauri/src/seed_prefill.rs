//! Seed-weight pre-fill proposal for sow (Track 4).
//!
//! Authority: BOOKS-BOUNDARY §3 — recipes may pre-fill physical quantity only.
//! This module proposes oz from `seed_rate_oz_per_tray * tray_count`. It never
//! imports costs, money, or pricing. The operator's field value wins once dirty.

/// Propose a seed weight from the crop rate and tray count.
/// `None` rate → no proposal (blank field).
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn proposed_seed_oz(rate_oz_per_tray: Option<f64>, tray_count: i64) -> Option<f64> {
    let rate = rate_oz_per_tray?;
    if !rate.is_finite() || rate <= 0.0 || tray_count < 1 {
        return None;
    }
    Some(rate * tray_count as f64)
}

/// Format a proposed (or stored) oz value for the sow seed field.
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub fn format_seed_oz(oz: f64) -> String {
    format!("{oz:.1}")
}

/// Dirty-field state for the sow seed input. Once the operator edits, pre-fill
/// stops driving the value — including when tray count changes afterward.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
pub struct SeedFieldState {
    pub value: String,
    pub dirty: bool,
}

impl SeedFieldState {
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn fresh_proposal(rate_oz_per_tray: Option<f64>, tray_count: i64) -> Self {
        let value = match proposed_seed_oz(rate_oz_per_tray, tray_count) {
            Some(oz) => format_seed_oz(oz),
            None => String::new(),
        };
        Self {
            value,
            dirty: false,
        }
    }

    /// Tray count or crop rate changed. Recompute only when not operator-owned.
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn on_proposal_inputs_changed(&mut self, rate_oz_per_tray: Option<f64>, tray_count: i64) {
        if self.dirty {
            return;
        }
        *self = Self::fresh_proposal(rate_oz_per_tray, tray_count);
    }

    /// Operator typed in the field. Empty clears dirty so a fresh proposal may return.
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn on_operator_edit(&mut self, next: String) {
        if next.trim().is_empty() {
            self.value = String::new();
            self.dirty = false;
        } else {
            self.value = next;
            self.dirty = true;
        }
    }

    /// Parse the confirm-time field. Blank → None (no seed record).
    /// Zero/negative → Err. Positive → Some(stored value as typed).
    #[allow(dead_code)] // H-2: live in tests; dead only in the lib target.
    pub fn confirm_quantity(&self) -> Result<Option<f64>, String> {
        let trimmed = self.value.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let n: f64 = trimmed
            .parse()
            .map_err(|_| "Seed weight must be a positive number (oz).".to_string())?;
        if !n.is_finite() {
            return Err("Seed weight must be a positive number (oz).".into());
        }
        if n <= 0.0 {
            return Err("Seed weight must be greater than zero.".into());
        }
        Ok(Some(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t3a_shelf_wins_correction_survives_resubmit() {
        let mut field = SeedFieldState::fresh_proposal(Some(8.0), 3);
        assert_eq!(field.value, "24.0");
        assert!(!field.dirty);
        field.on_operator_edit("22.5".into());
        assert_eq!(field.value, "22.5");
        assert!(field.dirty);
        // Re-render / proposal inputs unchanged but called again — still 22.5.
        field.on_proposal_inputs_changed(Some(8.0), 3);
        assert_eq!(field.value, "22.5");
        assert_eq!(field.confirm_quantity().unwrap(), Some(22.5));
    }

    #[test]
    fn t3b_shelf_wins_under_tray_count_recompute() {
        let mut field = SeedFieldState::fresh_proposal(Some(8.0), 3);
        field.on_operator_edit("22.5".into());
        field.on_proposal_inputs_changed(Some(8.0), 5);
        assert_eq!(field.value, "22.5");
        assert_eq!(field.confirm_quantity().unwrap(), Some(22.5));
    }
}
