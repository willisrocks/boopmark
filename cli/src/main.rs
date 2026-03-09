use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "boop", about = "Boopmark CLI — manage your bookmarks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a bookmark
    Add {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// List bookmarks
    List {
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, default_value = "newest")]
        sort: String,
    },
    /// Search bookmarks
    Search { query: String },
    /// Delete a bookmark
    Delete { id: String },
    /// Configure CLI
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set server URL
    SetServer { url: String },
    /// Set API key
    SetKey { key: String },
    /// Show current config
    Show,
}

#[derive(Serialize, Deserialize, Default)]
struct AppConfig {
    server_url: Option<String>,
    api_key: Option<String>,
}

impl AppConfig {
    fn path() -> PathBuf {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("boop");
        std::fs::create_dir_all(&dir).ok();
        dir.join("config.toml")
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let content = toml::to_string_pretty(self).unwrap();
        std::fs::write(Self::path(), content).ok();
    }

    fn client(&self) -> Result<ApiClient, String> {
        let server = self
            .server_url
            .as_deref()
            .ok_or("Server URL not configured. Run: boop config set-server <url>")?;
        let key = self
            .api_key
            .as_deref()
            .ok_or("API key not configured. Run: boop config set-key <key>")?;
        Ok(ApiClient {
            base_url: server.trim_end_matches('/').to_string(),
            api_key: key.to_string(),
            client: reqwest::Client::new(),
        })
    }
}

struct ApiClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl ApiClient {
    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url, path)
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, String> {
        self.client
            .get(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| e.to_string())
    }

    async fn post_json(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::Response, String> {
        self.client
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())
    }

    async fn delete(&self, path: &str) -> Result<reqwest::Response, String> {
        self.client
            .delete(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| e.to_string())
    }
}

#[derive(Serialize)]
struct CreateBookmarkRequest {
    url: String,
    title: Option<String>,
    tags: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct Bookmark {
    id: uuid::Uuid,
    url: String,
    title: Option<String>,
    description: Option<String>,
    domain: Option<String>,
    tags: Vec<String>,
    created_at: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Config { action } => {
            let mut config = AppConfig::load();
            match action {
                ConfigAction::SetServer { url } => {
                    config.server_url = Some(url);
                    config.save();
                    println!("Server URL saved.");
                }
                ConfigAction::SetKey { key } => {
                    config.api_key = Some(key);
                    config.save();
                    println!("API key saved.");
                }
                ConfigAction::Show => {
                    println!(
                        "Server: {}",
                        config.server_url.as_deref().unwrap_or("(not set)")
                    );
                    println!(
                        "API Key: {}",
                        config
                            .api_key
                            .as_deref()
                            .map(|k| format!("{}...", &k[..12.min(k.len())]))
                            .unwrap_or("(not set)".into())
                    );
                }
            }
            Ok(())
        }

        Commands::Add { url, title, tags } => {
            let client = AppConfig::load().client()?;
            let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let body = CreateBookmarkRequest { url, title, tags };
            let resp = client.post_json("/bookmarks", &body).await?;
            if resp.status().is_success() {
                let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
                println!("Added: {} ({})", bm.title.unwrap_or(bm.url), bm.id);
            } else {
                eprintln!("Failed: {}", resp.status());
            }
            Ok(())
        }

        Commands::List { search, tags, sort } => {
            let client = AppConfig::load().client()?;
            let mut query = format!("?sort={sort}");
            if let Some(s) = search {
                query.push_str(&format!("&search={s}"));
            }
            if let Some(t) = tags {
                query.push_str(&format!("&tags={t}"));
            }
            let resp = client.get(&format!("/bookmarks{query}")).await?;
            let bookmarks: Vec<Bookmark> = resp.json().await.map_err(|e| e.to_string())?;
            for bm in &bookmarks {
                println!(
                    "{} | {} | [{}]",
                    bm.title.as_deref().unwrap_or("(no title)"),
                    bm.url,
                    bm.tags.join(", ")
                );
            }
            if bookmarks.is_empty() {
                println!("No bookmarks found.");
            }
            Ok(())
        }

        Commands::Search { query } => {
            let client = AppConfig::load().client()?;
            let resp = client
                .get(&format!("/bookmarks?search={query}"))
                .await?;
            let bookmarks: Vec<Bookmark> = resp.json().await.map_err(|e| e.to_string())?;
            for bm in &bookmarks {
                println!(
                    "{} | {} | [{}]",
                    bm.title.as_deref().unwrap_or("(no title)"),
                    bm.url,
                    bm.tags.join(", ")
                );
            }
            if bookmarks.is_empty() {
                println!("No results.");
            }
            Ok(())
        }

        Commands::Delete { id } => {
            let client = AppConfig::load().client()?;
            let resp = client.delete(&format!("/bookmarks/{id}")).await?;
            if resp.status().is_success() {
                println!("Deleted.");
            } else {
                eprintln!("Failed: {}", resp.status());
            }
            Ok(())
        }
    }
}
