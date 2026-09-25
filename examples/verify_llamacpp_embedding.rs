use memory_platform::services::embedding::{
    EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let defaults = memory_platform::Config::default();
    let config = EmbeddingConfig {
        model: "nvidia".to_string(),
        nvidia_api_url: None,
        nvidia_api_key: None,
        nvidia_embedding_model: "nvidia/nemotron-3-embed-1b".to_string(),
        llama_cpp_url: defaults.llama_cpp_url,
        llama_cpp_model_name: defaults.llama_cpp_model_name,
        expected_dimension: defaults.embedding_dim,
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
