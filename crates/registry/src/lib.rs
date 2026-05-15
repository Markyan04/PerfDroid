//! Registry crate: profiler registration and metadata management.

use std::collections::BTreeMap;

use pdcore::types::ProfilerMetadata;

/// Minimal in-memory registry used by the demo runtime.
#[derive(Debug, Clone, Default)]
pub struct ProfilerRegistry {
    entries: BTreeMap<String, ProfilerMetadata>,
}

impl ProfilerRegistry {
    /// Registers or replaces profiler metadata by `profiler_key`.
    pub fn register(&mut self, metadata: ProfilerMetadata) {
        self.entries.insert(metadata.profiler_key.clone(), metadata);
    }

    /// Removes all registered metadata.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Lists registered profilers in deterministic key order.
    pub fn list(&self) -> Vec<ProfilerMetadata> {
        self.entries.values().cloned().collect()
    }
}
