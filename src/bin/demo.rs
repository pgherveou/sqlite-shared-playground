#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sqlite_shared_playground::{Client, PurseTag};

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "purses.sqlite3".into());
    let client = Client::open(&path).await?;
    let tag = PurseTag(b"demo".to_vec());
    let id = client
        .create_purse("Rust purse".into(), tag.clone())
        .await?
        .ok_or("purse IDs exhausted")?;
    println!("Created: {:?}", client.purse(id).await?);
    println!(
        "All demo purses: {:?}",
        client.purses_tagged(tag.clone()).await?
    );
    client.close().await?;

    let reopened = Client::open(&path).await?;
    println!(
        "After reopening {path}: {:?}",
        reopened.purses_tagged(tag).await?
    );
    reopened.close().await?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
