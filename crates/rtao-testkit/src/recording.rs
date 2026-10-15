//! Descriptor observations retained when a read pins its underlying view.

use bytes::Bytes;
use rtao_client::{ClientError, PinnedBlock, ReadObserver, ReadTouch, Snapshot, View};
use rtao_primitives::H256;
use rtao_runtime::RuntimeHandle;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, Default)]
struct Observations(Mutex<Vec<ReadTouch>>);
impl ReadObserver for Observations {
    fn on_read(&self, touch: ReadTouch) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(touch);
    }
}

/// Wrap a view and retain every descriptor access, including empty map scans.
#[derive(Debug, Clone)]
pub struct RecordingView<V> {
    inner: V,
    observations: Arc<Observations>,
}
impl<V> RecordingView<V> {
    /// Start recording a view without altering its head-selection policy.
    pub fn new(inner: V) -> Self {
        Self {
            inner,
            observations: Arc::default(),
        }
    }
    /// Borrow the wrapped view.
    pub fn inner(&self) -> &V {
        &self.inner
    }
    /// Copy the observations in access order, including repeated descriptors.
    pub fn touches(&self) -> Vec<ReadTouch> {
        self.observations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// Return all immutable state identities used by observed reads.
    pub fn block_hashes(&self) -> BTreeSet<H256> {
        self.touches()
            .into_iter()
            .filter_map(|touch| touch.at)
            .collect()
    }
    /// Discard previous observations before starting another sample.
    pub fn clear(&self) {
        self.observations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}
impl<V: View> View for RecordingView<V> {
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        Some(self.observations.as_ref())
    }
    fn metagraph_view_enabled(&self) -> bool {
        self.inner.metagraph_view_enabled()
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        self.inner.pinned()
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        Ok(self
            .inner
            .pin()
            .await?
            .with_read_observer(self.observations.clone()))
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        self.inner.runtime().await
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        self.pin().await?.storage_raw(key).await
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        self.pin().await?.storage_raw_many(keys).await
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        self.pin().await?.keys_paged(prefix, count, start).await
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        self.pin().await?.state_call(method, args).await
    }
}
