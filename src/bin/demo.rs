#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sqlite_shared_playground::{Client, PurseDao, PurseTag, initialize_database};

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "purses.sqlite3".into());
    let client = Client::open(&path, initialize_database).await?;
    let purses = PurseDao::new(client.clone());
    let tag = PurseTag(b"demo".to_vec());
    let id = purses
        .create("Rust purse".into(), tag.clone())
        .await?
        .ok_or("purse IDs exhausted")?;
    println!("Created: {:?}", purses.get(id).await?);
    println!("All demo purses: {:?}", purses.tagged(tag.clone()).await?);
    client.close().await?;

    let reopened = Client::open(&path, initialize_database).await?;
    let reopened_purses = PurseDao::new(reopened.clone());
    println!(
        "After reopening {path}: {:?}",
        reopened_purses.tagged(tag).await?
    );
    reopened.close().await?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
