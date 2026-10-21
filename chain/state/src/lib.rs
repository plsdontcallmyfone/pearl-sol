//! Agent-chain state: schema, parameters and QMDB storage.

pub mod config;
pub mod contracts;
pub mod dm;
pub mod reject;
pub mod schema;
pub mod social;
pub mod store;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;

pub use config::Config;
pub use schema::Key;
pub use store::{Snapshot, Store, View, WriteSet};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum StateError {
    #[error("storage: {0}")]
    Db(String),
    #[error("corrupt record: {0}")]
    Corrupt(String),
}

impl StateError {
    pub fn db(e: impl std::fmt::Display) -> Self {
        Self::Db(e.to_string())
    }
}

/// Read access to a state version. Implemented by QMDB views and [`MemState`].
pub trait StateRead: Sync {
    fn get(&self, key: &Key) -> impl Future<Output = Result<Option<Vec<u8>>, StateError>> + Send;
}

/// In-memory state for executor tests and invariant scans.
#[derive(Default, Debug)]
pub struct MemState {
    pub map: Mutex<BTreeMap<Key, Vec<u8>>>,
}

impl MemState {
    pub fn apply(&self, ws: &WriteSet) {
        let mut m = self.map.lock().unwrap();
        for (k, v) in &ws.writes {
            match v {
                Some(v) => {
                    m.insert(*k, v.clone());
                }
                None => {
                    m.remove(k);
                }
            }
        }
    }

    pub fn values(&self) -> Vec<Vec<u8>> {
        self.map.lock().unwrap().values().cloned().collect()
    }
}

impl StateRead for MemState {
    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StateError> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
}
