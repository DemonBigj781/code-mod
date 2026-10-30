use anyhow::Result;
use anyhow::anyhow;
use clap::Parser;

#[derive(Debug, Parser)]
pub(crate) struct RustwrightMcpArgs {}

pub(crate) async fn run(_args: RustwrightMcpArgs) -> Result<()> {
    code_rustwright_mcp::serve_stdio()
        .await
        .map_err(|error| anyhow!("Rustwright MCP server failed: {error}"))
}
