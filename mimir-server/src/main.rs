#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    mimir_server::start_server(mimir_server::DEFAULT_BIND_ADDR).await
}
