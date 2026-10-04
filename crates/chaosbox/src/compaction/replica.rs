//! Bounded export from committed custody into authenticated peer publications.
use super::Journal;

impl Journal {
    /// Verify retained intelligence against its original custody snapshots before sync.
    /// Full source archives remain local; the output contains bounded source capsules.
    pub fn replication_publication(&self) -> Result<crate::sync::Publication, String> {
        crate::sync::publication_from_sources(self.bundle()?, |hash| {
            self.evidence(hash)?["source_jsonl"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "missing normalized custody source".into())
        })
    }
}
