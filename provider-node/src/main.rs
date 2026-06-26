// SPDX-License-Identifier: GPL-3.0-only

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    storage_provider_node::command::run().await
}
