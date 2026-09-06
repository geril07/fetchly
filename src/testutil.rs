//! Test-only helpers: throwaway Redis for integration tests.
//!
//! Compiled only under `#[cfg(test)]`. Each test gets its own
//! `redis:8-alpine` container, so tests are isolated by construction.

use redis::aio::ConnectionManager;
use testcontainers::{ContainerAsync, GenericImage};

/// A live Redis plus the container that must stay alive while it is used.
pub(crate) struct TestRedis {
    /// Held (never read) to keep the container running for the test.
    pub _container: ContainerAsync<GenericImage>,
    pub manager: ConnectionManager,
    pub client: redis::Client,
}

/// Start a throwaway Redis 8 container.
///
/// Returns `None` (the caller skips the test) when Docker is unavailable —
/// except under `CI`, where it panics so a broken Docker setup cannot
/// silently green the suite.
pub(crate) async fn start_redis() -> Option<TestRedis> {
    use testcontainers::core::{IntoContainerPort as _, WaitFor};
    use testcontainers::runners::AsyncRunner as _;

    let container = GenericImage::new("redis", "8-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await;
    let container = match container {
        Ok(c) => c,
        Err(e) => {
            assert!(
                std::env::var("CI").is_err(),
                "testcontainers Redis failed in CI: {e}"
            );
            eprintln!("SKIP redis integration test (Docker unavailable: {e})");
            return None;
        }
    };
    let port = container.get_host_port_ipv4(6379.tcp()).await.ok()?;
    let client = redis::Client::open(format!("redis://127.0.0.1:{port}")).ok()?;
    let manager = client.get_connection_manager().await.ok()?;
    Some(TestRedis {
        _container: container,
        manager,
        client,
    })
}
