use memory_platform::services::embedding::{EmbeddingConfig, EmbeddingServiceFactory, EmbeddingService};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    
    let config = EmbeddingConfig {
        model: "local".to_string(),
        nvidia_api_url: None,
        nvidia_api_key: None,
        cache_size: 1000,
    };
    
    println!("Creating embedding service...");
    let factory = EmbeddingServiceFactory::new(config).await?;
    println!("Embedding service created: {:?}", factory);
    
    println!("Generating embedding...");
    let embedding = factory.embed("test").await?;
    println!("Embedding dim: {}", embedding.as_vec().len());
    println!("First 5 values: {:?}", &embedding.as_vec()[..5]);
    
    Ok(())
}
