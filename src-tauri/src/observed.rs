/// A value that came from outside this machine. It cannot be constructed
/// without its age and its origin, and it cannot be rendered without them.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed<T> {
    pub value: T,
    pub fetched_at: String, // UTC RFC3339
    pub origin: String,     // the source URL
}

impl<T> Observed<T> {
    pub fn new(value: T, fetched_at: impl Into<String>, origin: impl Into<String>) -> Self {
        Self {
            value,
            fetched_at: fetched_at.into(),
            origin: origin.into(),
        }
    }
}
