//! Optional Subxt access over the client's existing supervised transport.
//!
//! This escape hatch has its own metadata and transaction machinery. The typed
//! rtao APIs continue to use the core codec. Subxt is constructed with an
//! explicit legacy backend, so it never probes the chain-head or archive-v1
//! namespaces and does not create a second WebSocket connection.

use std::sync::Arc;

use ::subxt::{OnlineClient, SubstrateConfig, backend::LegacyBackend, error::OnlineClientError};
use futures::StreamExt;
use subxt_rpcs::{
    RpcClient,
    client::{RawRpcFuture, RawRpcSubscription, RawValue, RpcClientT},
};

use crate::Client;

/// Subxt's standard Substrate configuration over the shared rtao connection.
///
/// This API is available only with the `subxt` feature. Its backend retains one
/// rtao client clone, so dropping the original client does not close a transport
/// that is still used by the returned Subxt client. Explicit [`Client::close`]
/// closes the connection for both APIs.
pub type SubxtClient = OnlineClient<SubstrateConfig>;

impl Client {
    /// Construct an opt-in Subxt client using the same transport and an explicit
    /// legacy backend.
    ///
    /// This requests the genesis hash through the existing connection. Later
    /// Subxt operations fetch their own metadata on demand; rtao's runtime cache,
    /// nonce leases, signing checks and submission policy are separate APIs.
    /// Use the ordinary rtao transaction APIs when those guarantees are needed.
    pub async fn subxt(&self) -> Result<SubxtClient, OnlineClientError> {
        OnlineClient::from_backend(Arc::new(shared_backend(self.clone()))).await
    }
}

fn shared_backend(client: Client) -> LegacyBackend<SubstrateConfig> {
    LegacyBackend::builder().build(RpcClient::new(ClientRpc(client)))
}

// Keeping only `Client::rpc().clone()` would lose the user-facing lifetime
// guard: dropping the last rtao client would close a still-used Subxt socket.
struct ClientRpc(Client);

impl RpcClientT for ClientRpc {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<RawValue>>,
    ) -> RawRpcFuture<'a, Box<RawValue>> {
        Box::pin(async move {
            self.0
                .ensure_open()
                .map_err(|error| subxt_rpcs::Error::Client(Box::new(error)))?;
            tokio::select! {
                biased;
                () = self.0.closed() => Err(closed_error()),
                result = self.0.rpc().request_raw(method, params) => result,
            }
        })
    }

    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<RawValue>>,
        unsub: &'a str,
    ) -> RawRpcFuture<'a, RawRpcSubscription> {
        Box::pin(async move {
            self.0
                .ensure_open()
                .map_err(|error| subxt_rpcs::Error::Client(Box::new(error)))?;
            let subscription = tokio::select! {
                biased;
                () = self.0.closed() => return Err(closed_error()),
                result = self.0.rpc().subscribe_raw(sub, params, unsub) => result?,
            };
            let state = (subscription.stream, self.0.clone(), false);
            let stream = futures::stream::unfold(state, |(mut stream, client, done)| async move {
                if done {
                    return None;
                }
                tokio::select! {
                    biased;
                    () = client.closed() => Some((Err(closed_error()), (stream, client, true))),
                    item = stream.next() => item.map(|item| (item, (stream, client, false))),
                }
            });
            Ok(RawRpcSubscription {
                id: subscription.id,
                stream: Box::pin(stream),
            })
        })
    }
}

fn closed_error() -> subxt_rpcs::Error {
    subxt_rpcs::Error::Client(Box::new(crate::ClientError::from(
        rtao_rpc::RpcError::Closed,
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use ::subxt::backend::Backend;
    use ::subxt::config::substrate::H256;
    use futures::TryStreamExt;
    use rtao_testkit::{FakeChain, FakeNode, FakeState, Reply};
    use serde_json::{Value, json};

    async fn fixture() -> (Client, FakeChain) {
        let fake = FakeChain::from_state(FakeState::default());
        let mut config =
            crate::ClientConfig::new(crate::Network::resolve_with("local", |_| None).unwrap())
                .unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
            .await
            .unwrap();
        fake.mutate_state(|state| state.requests.clear());
        (client, fake)
    }

    #[tokio::test]
    async fn constructor_uses_shared_legacy_transport_without_namespace_discovery() {
        let (client, fake) = fixture().await;
        let expected = client.genesis().0;
        let subxt = client.subxt().await.unwrap();
        assert_eq!(subxt.genesis_hash().0, expected);
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "chain_getBlockHash");
        assert_eq!(requests[0].params, json!([0]));
    }

    #[tokio::test]
    async fn pinned_storage_uses_the_same_transport_and_exact_block() {
        let (client, fake) = fixture().await;
        let backend = shared_backend(client);
        let at = H256([19; 32]);
        fake.mutate_state(|state| {
            state.queue_reply("state_getStorage", Reply::result(json!("0x010203")));
        });
        let values = backend
            .storage_fetch_values(vec![vec![0xaa, 0xbb]], at)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].key, vec![0xaa, 0xbb]);
        assert_eq!(values[0].value, vec![1, 2, 3]);
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "state_getStorage");
        assert_eq!(
            requests[0].params,
            json!(["0xaabb", format!("0x{}", hex::encode(at.0))])
        );
    }

    #[tokio::test]
    async fn backend_retains_client_lifetime_and_obeys_explicit_close() {
        let (client, fake) = fixture().await;
        let closer = client.clone();
        let backend = shared_backend(client.clone());
        drop(client);
        assert_eq!(backend.genesis_hash().await.unwrap().0, closer.genesis().0);
        closer.close().await;
        fake.mutate_state(|state| state.requests.clear());
        assert!(backend.genesis_hash().await.is_err());
        assert!(closer.subxt().await.is_err());
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn sole_backend_owner_keeps_socket_alive_until_backend_drop() {
        let mut node = FakeNode::start().await.unwrap();
        let mut config =
            crate::ClientConfig::new(crate::Network::resolve_with(node.url(), |_| None).unwrap())
                .unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.session.rate_limit = None;
        let client = Client::connect(config).await.unwrap();
        let expected = client.genesis().0;
        let backend = shared_backend(client.clone());
        drop(client);
        assert_eq!(backend.genesis_hash().await.unwrap().0, expected);
        drop(backend);
        tokio::time::timeout(std::time::Duration::from_secs(1), node.wait_for_closed(1))
            .await
            .unwrap();
        node.shutdown().await;
    }

    #[tokio::test]
    async fn explicit_close_cancels_injected_pending_request_and_subscription_setup() {
        for subscription in [false, true] {
            let (client, fake) = fixture().await;
            let method = if subscription {
                "chain_subscribeNewHeads"
            } else {
                "chain_getBlockHash"
            };
            fake.mutate_state(|state| state.queue_reply(method, Reply::silent()));
            let rpc = ClientRpc(client.clone());
            let pending = tokio::spawn(async move {
                if subscription {
                    rpc.subscribe_raw(method, None, "chain_unsubscribeNewHeads")
                        .await
                        .map(|_| ())
                } else {
                    rpc.request_raw(method, None).await.map(|_| ())
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while fake.requests().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            client.close().await;
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap();
            assert!(result.is_err());
            assert_eq!(fake.requests().len(), 1);
        }
    }

    #[tokio::test]
    async fn explicit_close_terminates_an_injected_subscription_stream() {
        let (client, fake) = fixture().await;
        fake.mutate_state(|state| {
            state.queue_reply(
                "chain_subscribeNewHeads",
                Reply::subscription("subxt-pending-head", vec![], false),
            );
        });
        let rpc = ClientRpc(client.clone());
        let mut subscription = rpc
            .subscribe_raw("chain_subscribeNewHeads", None, "chain_unsubscribeNewHeads")
            .await
            .unwrap();
        client.close().await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                subscription.stream.next()
            )
            .await
            .unwrap()
            .unwrap()
            .is_err()
        );
        assert!(subscription.stream.next().await.is_none());
    }

    #[tokio::test]
    async fn constructor_preserves_rpc_failure_without_probing_another_backend() {
        let (client, fake) = fixture().await;
        fake.mutate_state(|state| {
            state.queue_reply(
                "chain_getBlockHash",
                Reply::error(-32601, "legacy genesis unavailable", None),
            );
        });
        let error = client.subxt().await.unwrap_err();
        assert!(error.to_string().contains("legacy genesis unavailable"));
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "chain_getBlockHash");
    }

    #[tokio::test]
    async fn raw_subscription_preserves_method_params_items_and_unsubscribe() {
        let (client, fake) = fixture().await;
        let rpc = ClientRpc(client.clone());
        fake.mutate_state(|state| {
            state.queue_reply(
                "chain_subscribeNewHeads",
                Reply::subscription("subxt-head", vec![json!({"number":"0x2a"})], false),
            );
        });
        let mut subscription = rpc
            .subscribe_raw(
                "chain_subscribeNewHeads",
                Some(serde_json::value::to_raw_value(&json!([])).unwrap()),
                "chain_unsubscribeNewHeads",
            )
            .await
            .unwrap();
        assert_eq!(subscription.id.as_deref(), Some("subxt-head"));
        let item = subscription.stream.next().await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(item.get()).unwrap(),
            json!({"number":"0x2a"})
        );
        drop(subscription);
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "chain_subscribeNewHeads");
        assert_eq!(requests[0].params, json!([]));
        assert_eq!(requests[1].method, "chain_unsubscribeNewHeads");
        assert_eq!(requests[1].params, json!(["subxt-head"]));

        client.close().await;
        assert!(
            rpc.subscribe_raw("chain_subscribeNewHeads", None, "chain_unsubscribeNewHeads")
                .await
                .is_err()
        );
        assert_eq!(fake.requests().len(), 2);
    }
}
