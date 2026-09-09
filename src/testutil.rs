//! Throwaway Redis for integration tests (`redis:8-alpine`, one container per test).

use redis::aio::ConnectionManager;
use testcontainers::{ContainerAsync, GenericImage};

/// Dropping `_container` kills Redis, so tests must bind the guard.
pub(crate) struct TestRedis {
    pub _container: ContainerAsync<GenericImage>,
    pub manager: ConnectionManager,
    pub client: redis::Client,
}

/// Returns `None` (caller skips) when Docker is unavailable; panics under `CI` so broken Docker cannot green the suite.
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
