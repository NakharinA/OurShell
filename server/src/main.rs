#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ourshell_server::run().await
}
