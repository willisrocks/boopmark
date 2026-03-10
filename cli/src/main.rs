use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn parse_uuid(s: &str) -> Result<uuid::Uuid, String> {
    s.parse::<uuid::Uuid>()
        .map_err(|_| format!("invalid bookmark ID: {s:?} (expected a UUID)"))
}

#[derive(Parser)]
#[command(name = "boop", about = "Boopmark CLI — manage your bookmarks")]
struct Cli {
    /// Output format: json or plain (default: plain)
    #[arg(long, short, global = true, default_value = "plain")]
    output: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    Json,
    Plain,
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
    /// Get a bookmark by ID
    Get { id: String },
    /// Update a bookmark
    Update {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// Delete a bookmark
    Delete { id: String },
    /// List all tags
    Tags,
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
        let dir = if let Ok(custom) = std::env::var("BOOP_CONFIG_DIR") {
            PathBuf::from(custom)
        } else {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("boop")
        };
        std::fs::create_dir_all(&dir).ok();
        dir.join("config.toml")
    }

    fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!(
                        "Warning: config file {} contains invalid TOML, using defaults: {e}",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    fn save(&self) -> Result<(), String> {
        let content =
            toml::to_string_pretty(self).map_err(|e| format!("Failed to serialize config: {e}"))?;
        std::fs::write(Self::path(), content)
            .map_err(|e| format!("Failed to write config to {}: {e}", Self::path().display()))
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

    async fn put_json(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::Response, String> {
        self.client
            .put(self.url(path))
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

async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, String> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("HTTP {status}: {body}"))
    }
}

#[derive(Serialize)]
struct CreateBookmarkRequest {
    url: String,
    title: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Serialize)]
struct UpdateBookmarkRequest {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(Deserialize, Serialize)]
struct Bookmark {
    id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
    url: String,
    title: Option<String>,
    description: Option<String>,
    domain: Option<String>,
    image_url: Option<String>,
    tags: Vec<String>,
    created_at: String,
    updated_at: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Tag {
    name: String,
    count: i64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn print_bookmark_plain(bm: &Bookmark) {
    println!(
        "{} | {} | [{}]",
        bm.title.as_deref().unwrap_or("(no title)"),
        bm.url,
        bm.tags.join(", ")
    );
}

async fn run(cli: Cli) -> Result<(), String> {
    let output = cli.output;

    match cli.command {
        Commands::Config { action } => {
            let mut config = AppConfig::load();
            match action {
                ConfigAction::SetServer { url } => {
                    config.server_url = Some(url);
                    config.save()?;
                    println!("Server URL saved.");
                }
                ConfigAction::SetKey { key } => {
                    config.api_key = Some(key);
                    config.save()?;
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
            let resp = check_response(client.post_json("/bookmarks", &body).await?).await?;
            let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&bm).unwrap());
                }
                OutputFormat::Plain => {
                    println!("Added: {} ({})", bm.title.as_deref().unwrap_or(&bm.url), bm.id);
                }
            }
            Ok(())
        }

        Commands::List { search, tags, sort } => {
            let client = AppConfig::load().client()?;
            let mut query = format!("?sort={}", url_encode(&sort));
            if let Some(s) = &search {
                query.push_str(&format!("&search={}", url_encode(s)));
            }
            if let Some(t) = &tags {
                query.push_str(&format!("&tags={}", url_encode(t)));
            }
            let resp = check_response(client.get(&format!("/bookmarks{query}")).await?).await?;
            let bookmarks: Vec<Bookmark> = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&bookmarks).unwrap());
                }
                OutputFormat::Plain => {
                    for bm in &bookmarks {
                        print_bookmark_plain(bm);
                    }
                    if bookmarks.is_empty() {
                        println!("No bookmarks found.");
                    }
                }
            }
            Ok(())
        }

        Commands::Search { query } => {
            let client = AppConfig::load().client()?;
            let resp = check_response(
                client
                    .get(&format!("/bookmarks?search={}", url_encode(&query)))
                    .await?,
            )
            .await?;
            let bookmarks: Vec<Bookmark> = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&bookmarks).unwrap());
                }
                OutputFormat::Plain => {
                    for bm in &bookmarks {
                        print_bookmark_plain(bm);
                    }
                    if bookmarks.is_empty() {
                        println!("No results.");
                    }
                }
            }
            Ok(())
        }

        Commands::Get { id } => {
            let id = parse_uuid(&id)?;
            let client = AppConfig::load().client()?;
            let resp = check_response(client.get(&format!("/bookmarks/{id}")).await?).await?;
            let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&bm).unwrap());
                }
                OutputFormat::Plain => {
                    print_bookmark_plain(&bm);
                }
            }
            Ok(())
        }

        Commands::Update {
            id,
            title,
            description,
            tags,
        } => {
            let id = parse_uuid(&id)?;
            let client = AppConfig::load().client()?;
            let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let body = UpdateBookmarkRequest {
                title,
                description,
                tags,
            };
            let resp =
                check_response(client.put_json(&format!("/bookmarks/{id}"), &body).await?).await?;
            let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&bm).unwrap());
                }
                OutputFormat::Plain => {
                    println!(
                        "Updated: {} ({})",
                        bm.title.as_deref().unwrap_or(&bm.url),
                        bm.id
                    );
                }
            }
            Ok(())
        }

        Commands::Delete { id } => {
            let id = parse_uuid(&id)?;
            let client = AppConfig::load().client()?;
            check_response(client.delete(&format!("/bookmarks/{id}")).await?).await?;
            match output {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({"deleted": id})).unwrap()
                    );
                }
                OutputFormat::Plain => {
                    println!("Deleted.");
                }
            }
            Ok(())
        }

        Commands::Tags => {
            let client = AppConfig::load().client()?;
            let resp = check_response(client.get("/bookmarks/tags").await?).await?;
            let tags: Vec<Tag> = resp.json().await.map_err(|e| e.to_string())?;
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&tags).unwrap());
                }
                OutputFormat::Plain => {
                    for tag in &tags {
                        println!("{} ({})", tag.name, tag.count);
                    }
                    if tags.is_empty() {
                        println!("No tags found.");
                    }
                }
            }
            Ok(())
        }
    }
}
