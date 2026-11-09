#[tokio::main]
async fn main() -> anyhow::Result<()> {
    taiwalog::cli::run().await
}
