//! Shared, bounded replay protection backed by Redis.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use num_bigint::BigInt;
use redis::aio::MultiplexedConnection;
use rtao_primitives::AccountId;

use crate::{BoxFuture, NonceStore, StoreError};

const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// A shared replay store using atomic Redis `SET ... NX PX` operations.
///
/// Redis must have a positive `maxmemory` bound and `maxmemory-policy
/// noeviction`; construction checks both. Exhausted memory returns
/// [`StoreError::Full`] instead of evicting an accepted nonce. The store
/// never deletes keys, refreshes duplicate keys, or retries an ambiguous
/// operation. Connection failures and timeouts fail closed.
///
/// Use a dedicated, trusted Redis deployment. Its administrators must keep
/// the checked policy and memory bound fixed, prevent deletion of live
/// `btauth:*` keys, and preserve replay state across restart and failover.
/// Deny `CONFIG SET`, flush/delete, and key-expiry mutation commands to
/// application credentials. The connection needs `CONFIG GET` at startup
/// and `SET` on `btauth:*` keys. After any replay-state loss, stop accepting
/// requests for at least the configured retention before constructing a new
/// store. A connection-time configuration check cannot prevent privileged
/// changes to the server or its stored data.
///
/// The connection is multiplexed and cloneable. Construction and operations
/// require Tokio and have five-second deadlines; a failed connection is not
/// automatically replaced with an unchecked connection. Errors omit the
/// Redis URI, credentials, server responses, and nonce keys.
/// A timeout or backend error closes every clone of the store to further
/// operations. Reconstruct it after checking that replay state was retained;
/// memory exhaustion alone does not close the store.
#[derive(Clone)]
pub struct RedisNonceStore {
    connection: MultiplexedConnection,
    retention: Duration,
    ttl_ms: i64,
    closed: Arc<AtomicBool>,
}

impl RedisNonceStore {
    /// Connect and verify that Redis cannot evict live replay keys.
    ///
    /// Retention must be positive and fit Redis's signed millisecond range.
    /// Durations round upward with one additional millisecond. This protects
    /// the inclusive freshness boundary despite Redis's millisecond expiry
    /// rounding. Invalid configuration, missing `CONFIG GET`
    /// permission, and connection failures return a sanitized store error.
    pub async fn connect(connection_url: &str, retention: Duration) -> Result<Self, StoreError> {
        let ttl_ms = retention_ms(retention)?;
        let client = redis::Client::open(connection_url)
            .map_err(|_| unavailable("invalid Redis connection configuration"))?;
        let mut connection =
            tokio::time::timeout(IO_TIMEOUT, client.get_multiplexed_async_connection())
                .await
                .map_err(|_| unavailable("Redis connection timed out"))?
                .map_err(|_| unavailable("Redis connection failed"))?;

        let policy = read_config(&mut connection, "maxmemory-policy").await?;
        if policy != "noeviction" {
            return Err(unavailable("Redis requires maxmemory-policy noeviction"));
        }
        let memory = read_config(&mut connection, "maxmemory").await?;
        if memory.parse::<u64>().ok().is_none_or(|memory| memory == 0) {
            return Err(unavailable("Redis requires a positive maxmemory bound"));
        }
        Ok(Self {
            connection,
            retention,
            ttl_ms,
            closed: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl NonceStore for RedisNonceStore {
    fn retention(&self) -> Duration {
        self.retention
    }

    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce_ns: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(unavailable("Redis nonce connection must be re-established"));
            }
            let mut connection = self.connection.clone();
            let mut command = redis::cmd("SET");
            command
                .arg(format!("btauth:{hotkey}:{nonce_ns}"))
                .arg("1")
                .arg("NX")
                .arg("PX")
                .arg(self.ttl_ms);
            let response = tokio::time::timeout(
                IO_TIMEOUT,
                command.query_async::<Option<String>>(&mut connection),
            )
            .await
            .map_err(|_| {
                self.closed.store(true, Ordering::Release);
                unavailable("Redis nonce operation timed out")
            })?
            .map_err(|error| {
                let error = store_error(error);
                if !matches!(error, StoreError::Full) {
                    self.closed.store(true, Ordering::Release);
                }
                error
            })?;
            match response.as_deref() {
                Some("OK") => Ok(true),
                None => Ok(false),
                Some(_) => {
                    self.closed.store(true, Ordering::Release);
                    Err(unavailable("unexpected Redis nonce response"))
                }
            }
        })
    }
}

fn retention_ms(retention: Duration) -> Result<i64, StoreError> {
    if retention.is_zero() {
        return Err(unavailable(
            "Redis retention must fit a positive signed millisecond TTL",
        ));
    }
    let milliseconds = retention.as_nanos().div_ceil(1_000_000) + 1;
    i64::try_from(milliseconds)
        .ok()
        .filter(|milliseconds| *milliseconds > 0)
        .ok_or_else(|| unavailable("Redis retention must fit a positive signed millisecond TTL"))
}

async fn read_config(
    connection: &mut MultiplexedConnection,
    name: &str,
) -> Result<String, StoreError> {
    let mut command = redis::cmd("CONFIG");
    command.arg("GET").arg(name);
    let mut response = tokio::time::timeout(
        IO_TIMEOUT,
        command.query_async::<HashMap<String, String>>(connection),
    )
    .await
    .map_err(|_| unavailable("Redis configuration check timed out"))?
    .map_err(|_| unavailable("Redis configuration check failed"))?;
    response
        .remove(name)
        .ok_or_else(|| unavailable("Redis configuration check returned no value"))
}

fn store_error(error: redis::RedisError) -> StoreError {
    if error.code() == Some("OOM") {
        StoreError::Full
    } else {
        unavailable("Redis nonce operation failed")
    }
}

fn unavailable(reason: &str) -> StoreError {
    StoreError::Unavailable(reason.into())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;
    use tokio::task::JoinSet;

    #[derive(Default)]
    struct State {
        policy: String,
        maxmemory: String,
        config_denied: bool,
        insert_error: Option<String>,
        disconnect_on_set: bool,
        set_reply_delay: Duration,
        entries: HashMap<String, Instant>,
        commands: Vec<Vec<String>>,
    }

    /// A RESP2 protocol fixture, rather than evidence from a live Redis server.
    struct FakeRedis {
        url: String,
        state: Arc<Mutex<State>>,
        shutdown: Option<oneshot::Sender<()>>,
    }

    impl FakeRedis {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture binds loopback");
            let url = format!("redis://{}/", listener.local_addr().unwrap());
            let state = Arc::new(Mutex::new(State {
                policy: "noeviction".into(),
                maxmemory: "1048576".into(),
                ..State::default()
            }));
            let shared = state.clone();
            let (shutdown, mut shutdown_rx) = oneshot::channel();
            tokio::spawn(async move {
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let shared = shared.clone();
                            connections.spawn(async move { serve(stream, shared).await });
                        }
                        Some(_) = connections.join_next(), if !connections.is_empty() => {}
                    }
                }
            });
            Self {
                url,
                state,
                shutdown: Some(shutdown),
            }
        }

        async fn store(&self, retention: Duration) -> RedisNonceStore {
            RedisNonceStore::connect(&self.url, retention)
                .await
                .expect("safe fixture connects")
        }
    }

    impl Drop for FakeRedis {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
        }
    }

    async fn read_command(reader: &mut BufReader<TcpStream>) -> io::Result<Vec<String>> {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(Vec::new());
        }
        let count: usize = line
            .trim_end()
            .strip_prefix('*')
            .ok_or_else(|| io::Error::other("expected RESP array"))?
            .parse()
            .map_err(io::Error::other)?;
        let mut command = Vec::with_capacity(count);
        for _ in 0..count {
            line.clear();
            reader.read_line(&mut line).await?;
            let length: usize = line
                .trim_end()
                .strip_prefix('$')
                .ok_or_else(|| io::Error::other("expected RESP bulk string"))?
                .parse()
                .map_err(io::Error::other)?;
            let mut bytes = vec![0; length + 2];
            reader.read_exact(&mut bytes).await?;
            assert_eq!(&bytes[length..], b"\r\n");
            bytes.truncate(length);
            command.push(String::from_utf8(bytes).map_err(io::Error::other)?);
        }
        Ok(command)
    }

    async fn serve(stream: TcpStream, shared: Arc<Mutex<State>>) -> io::Result<()> {
        let mut reader = BufReader::new(stream);
        loop {
            let command = read_command(&mut reader).await?;
            if command.is_empty() {
                return Ok(());
            }
            let (response, delay) = {
                let mut state = shared.lock().unwrap();
                state.commands.push(command.clone());
                if command[0] == "SET" && state.disconnect_on_set {
                    return Ok(());
                }
                let delay = if command[0] == "SET" {
                    state.set_reply_delay
                } else {
                    Duration::ZERO
                };
                let response = match command[0].as_str() {
                    "CLIENT" | "SELECT" | "AUTH" => "+OK\r\n".into(),
                    "CONFIG" if state.config_denied => {
                        "-NOPERM configuration denied secret-password\r\n".into()
                    }
                    "CONFIG" => {
                        assert_eq!(command[1], "GET");
                        let value = match command[2].as_str() {
                            "maxmemory-policy" => &state.policy,
                            "maxmemory" => &state.maxmemory,
                            _ => panic!("unexpected configuration query"),
                        };
                        format!(
                            "*2\r\n${}\r\n{}\r\n${}\r\n{}\r\n",
                            command[2].len(),
                            command[2],
                            value.len(),
                            value
                        )
                    }
                    "SET" if state.insert_error.is_some() => {
                        format!("-{}\r\n", state.insert_error.as_ref().unwrap())
                    }
                    "SET" => {
                        assert_eq!(&command[2..5], ["1", "NX", "PX"]);
                        assert_eq!(command.len(), 6);
                        let now = Instant::now();
                        state.entries.retain(|_, expiry| *expiry > now);
                        if state.entries.contains_key(&command[1]) {
                            "$-1\r\n".into()
                        } else {
                            let ttl = Duration::from_millis(command[5].parse().unwrap());
                            state.entries.insert(command[1].clone(), now + ttl);
                            "+OK\r\n".into()
                        }
                    }
                    _ => panic!("unexpected Redis command: {command:?}"),
                };
                (response, delay)
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            reader.get_mut().write_all(response.as_bytes()).await?;
        }
    }

    #[test]
    fn ttl_rounds_up_and_rejects_invalid_ranges() {
        assert_eq!(retention_ms(Duration::from_nanos(1)), Ok(2));
        assert_eq!(retention_ms(Duration::from_micros(1_001)), Ok(3));
        assert_eq!(retention_ms(Duration::from_secs(12)), Ok(12_001));
        assert_eq!(retention_ms(Duration::from_secs(60)), Ok(60_001));
        assert!(retention_ms(Duration::ZERO).is_err());
        assert!(retention_ms(Duration::MAX).is_err());
    }

    #[tokio::test]
    async fn two_connections_share_atomic_replay_state_and_signed_keys() {
        let redis = FakeRedis::start().await;
        let first = redis.store(Duration::from_secs(60)).await;
        let second = redis.store(Duration::from_secs(60)).await;
        let hotkey = AccountId([7; 32]);
        let nonce = -(BigInt::from(1) << 100u32);
        let (one, two) = tokio::join!(
            first.check_and_store(&hotkey, &nonce),
            second.check_and_store(&hotkey, &nonce),
        );
        assert!(matches!(
            (one, two),
            (Ok(true), Ok(false)) | (Ok(false), Ok(true))
        ));
        assert_eq!(first.check_and_store(&hotkey, &nonce).await, Ok(false));
        assert_eq!(
            first.check_and_store(&AccountId([8; 32]), &nonce).await,
            Ok(true)
        );
        assert_eq!(first.check_and_store(&hotkey, &-nonce).await, Ok(true));
        let state = redis.state.lock().unwrap();
        assert!(
            state
                .entries
                .contains_key(&format!("btauth:{hotkey}:-1267650600228229401496703205376"))
        );
        assert_eq!(state.entries.len(), 3);
        assert!(
            state
                .commands
                .iter()
                .all(|command| { matches!(command[0].as_str(), "CLIENT" | "CONFIG" | "SET") })
        );
    }

    #[tokio::test]
    async fn duplicates_do_not_extend_retention_and_expiry_allows_reuse() {
        let redis = FakeRedis::start().await;
        let retention = Duration::from_secs(60);
        let store = redis.store(retention).await;
        assert_eq!(store.retention(), retention);
        let hotkey = AccountId([1; 32]);
        let nonce = BigInt::from(42);
        let key = format!("btauth:{hotkey}:{nonce}");
        assert_eq!(store.check_and_store(&hotkey, &nonce).await, Ok(true));
        let deadline = redis.state.lock().unwrap().entries[&key];
        assert_eq!(store.check_and_store(&hotkey, &nonce).await, Ok(false));
        assert_eq!(redis.state.lock().unwrap().entries[&key], deadline);
        redis
            .state
            .lock()
            .unwrap()
            .entries
            .insert(key, Instant::now());
        assert_eq!(store.check_and_store(&hotkey, &nonce).await, Ok(true));
    }

    #[tokio::test]
    async fn unsafe_or_unbounded_server_configuration_is_rejected() {
        let redis = FakeRedis::start().await;
        for policy in ["allkeys-lru", "volatile-ttl", ""] {
            redis.state.lock().unwrap().policy = policy.into();
            assert!(
                RedisNonceStore::connect(&redis.url, Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
        redis.state.lock().unwrap().policy = "noeviction".into();
        for maxmemory in ["0", "-1", "invalid"] {
            redis.state.lock().unwrap().maxmemory = maxmemory.into();
            assert!(
                RedisNonceStore::connect(&redis.url, Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
        redis.state.lock().unwrap().maxmemory = "1048576".into();
        redis.state.lock().unwrap().config_denied = true;
        let error = RedisNonceStore::connect(&redis.url, Duration::from_secs(1))
            .await
            .err()
            .expect("denied configuration fails closed");
        assert_eq!(
            error.to_string(),
            "nonce store is unavailable: Redis configuration check failed"
        );
        assert!(!error.to_string().contains("secret-password"));
    }

    #[tokio::test]
    async fn oom_and_backend_errors_fail_closed_without_eviction_or_secret_leaks() {
        let redis = FakeRedis::start().await;
        let store = redis.store(Duration::from_secs(60)).await;
        let hotkey = AccountId([1; 32]);
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(1)).await,
            Ok(true)
        );
        redis.state.lock().unwrap().insert_error = Some("OOM command denied".into());
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(2)).await,
            Err(StoreError::Full)
        );
        redis.state.lock().unwrap().insert_error = None;
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(1)).await,
            Ok(false)
        );
        redis.state.lock().unwrap().insert_error = Some("ERR secret-password internal path".into());
        let error = store
            .check_and_store(&hotkey, &BigInt::from(2))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "nonce store is unavailable: Redis nonce operation failed"
        );
        assert_eq!(redis.state.lock().unwrap().entries.len(), 1);
        redis.state.lock().unwrap().insert_error = None;
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(2)).await,
            Err(unavailable("Redis nonce connection must be re-established"))
        );
        let store = redis.store(Duration::from_secs(60)).await;
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(1)).await,
            Ok(false)
        );
        assert_eq!(
            store.check_and_store(&hotkey, &BigInt::from(2)).await,
            Ok(true)
        );
    }

    #[tokio::test]
    async fn malformed_uri_is_sanitized_and_bad_retention_connects_to_nothing() {
        let error = RedisNonceStore::connect("secret-password not a uri", Duration::from_secs(1))
            .await
            .err()
            .expect("invalid URI is rejected");
        assert_eq!(
            error.to_string(),
            "nonce store is unavailable: invalid Redis connection configuration"
        );
        let redis = FakeRedis::start().await;
        assert!(
            RedisNonceStore::connect(&redis.url, Duration::ZERO)
                .await
                .is_err()
        );
        assert!(
            RedisNonceStore::connect(&redis.url, Duration::MAX)
                .await
                .is_err()
        );
        assert!(redis.state.lock().unwrap().commands.is_empty());
    }

    #[tokio::test]
    async fn redis_store_does_not_block() {
        let redis = FakeRedis::start().await;
        let store = redis.store(Duration::from_secs(60)).await;
        redis.state.lock().unwrap().set_reply_delay = Duration::from_millis(100);
        let hotkey = AccountId([1; 32]);
        let nonce = BigInt::from(1);
        let operation = store.check_and_store(&hotkey, &nonce);
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => panic!("delayed response completed before the independent timer: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
        assert_eq!(operation.await, Ok(true));
    }

    #[tokio::test]
    async fn lost_connection_never_accepts_or_inserts_a_nonce() {
        let redis = FakeRedis::start().await;
        let store = redis.store(Duration::from_secs(60)).await;
        redis.state.lock().unwrap().disconnect_on_set = true;
        let hotkey = AccountId([1; 32]);
        let nonce = BigInt::from(1);
        assert_eq!(
            store.check_and_store(&hotkey, &nonce).await,
            Err(unavailable("Redis nonce operation failed"))
        );
        assert_eq!(
            store.clone().check_and_store(&hotkey, &nonce).await,
            Err(unavailable("Redis nonce connection must be re-established"))
        );
        assert!(redis.state.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn timed_out_success_fails_closed_and_is_not_retried() {
        let redis = FakeRedis::start().await;
        let store = redis.store(Duration::from_secs(60)).await;
        redis.state.lock().unwrap().set_reply_delay = IO_TIMEOUT + Duration::from_millis(200);
        let hotkey = AccountId([1; 32]);
        let nonce = BigInt::from(1);
        assert_eq!(
            store.check_and_store(&hotkey, &nonce).await,
            Err(unavailable("Redis nonce operation timed out"))
        );
        assert_eq!(
            store.check_and_store(&hotkey, &nonce).await,
            Err(unavailable("Redis nonce connection must be re-established"))
        );
        let state = redis.state.lock().unwrap();
        assert!(
            state
                .entries
                .contains_key(&format!("btauth:{hotkey}:{nonce}"))
        );
        assert_eq!(
            state
                .commands
                .iter()
                .filter(|command| command[0] == "SET")
                .count(),
            1
        );
    }
}
