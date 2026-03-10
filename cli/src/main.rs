use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "boop", about = "Boopmark CLI — manage your bookmarks", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
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
    /// Upgrade boop to the latest version
    Upgrade,
    /// Configure CLI
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
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
            let resp = client.get(&format!("/bookmarks?search={query}")).await?;
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

        Commands::Upgrade => upgrade().await,
    }
}

fn detect_target() -> Result<String, String> {
    let target = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        (arch, os) => return Err(format!("Unsupported platform: {arch}-{os}")),
    };
    Ok(target.to_string())
}

async fn upgrade() -> Result<(), String> {
    let target = detect_target()?;
    let url = format!(
        "https://github.com/foundra-build/boopmark/releases/latest/download/boop-{target}"
    );

    println!("Downloading latest boop for {target}...");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    let exe_path = std::env::current_exe().map_err(|e| format!("Cannot find current exe: {e}"))?;
    let staging_path = exe_path.with_extension(format!("tmp.{}", std::process::id()));

    std::fs::write(&staging_path, &bytes)
        .map_err(|e| format!("Failed to write staging file: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let _ = Command::new("xattr")
            .args(["-cr", staging_path.to_str().unwrap_or("")])
            .output();
        let _ = Command::new("codesign")
            .args(["--force", "--sign", "-", staging_path.to_str().unwrap_or("")])
            .output();
    }

    std::fs::rename(&staging_path, &exe_path)
        .map_err(|e| format!("Failed to replace binary: {e}"))?;

    println!("Successfully upgraded boop!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn test_cli_version_flag() {
        let result = Cli::try_parse_from(["boop", "--version"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn test_cli_short_version_flag() {
        let result = Cli::try_parse_from(["boop", "-V"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn test_cli_upgrade_recognized() {
        let cli = Cli::try_parse_from(["boop", "upgrade"]).unwrap();
        assert!(matches!(cli.command, Commands::Upgrade));
    }

    #[test]
    fn test_detect_target() {
        let result = detect_target();
        assert!(result.is_ok());
        let target = result.unwrap();
        assert!(target.contains(std::env::consts::ARCH));
    }
}
