//! Known per-row divergences (Ruling 6). Mirrors docs/divergence-ledger.md.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDivergence {
    pub id: &'static str,
    pub table: &'static str,
    pub pk: &'static str,
    pub column: &'static str,
    /// Sequence number(s) named in the ledger entry (human-readable authority).
    pub seq_range: &'static str,
}

/// Authoritative list used by verify-replay. Ledger file must match entry for entry.
pub const KNOWN_DIVERGENCES: &[KnownDivergence] = &[
    KnownDivergence {
        id: "DL-001",
        table: "orders",
        pk: "5d5c0021-3753-4591-9582-f0a52e4eeaa0",
        column: "client_reference",
        seq_range: "12",
    },
    KnownDivergence {
        id: "DL-002",
        table: "orders",
        pk: "b324a8fa-888a-4a66-9d89-47f83b7bf4b3",
        column: "client_reference",
        seq_range: "12",
    },
    KnownDivergence {
        id: "DL-003",
        table: "orders",
        pk: "68a0cad5-82bc-43b1-b66c-7beedc5ba0df",
        column: "created_at",
        seq_range: "6",
    },
    KnownDivergence {
        id: "DL-004",
        table: "orders",
        pk: "68a0cad5-82bc-43b1-b66c-7beedc5ba0df",
        column: "updated_at",
        seq_range: "6",
    },
];
