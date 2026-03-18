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
        description: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        /// Use LLM to suggest missing title, description, and tags
        #[arg(long)]
        suggest: bool,
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
    /// Edit a bookmark
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        /// Use LLM to suggest title, description, and tags
        #[arg(long)]
        suggest: bool,
    },
    /// Get LLM suggestions for a URL without saving
    Suggest {
        url: String,
    },
    /// Delete a bookmark
    Delete { id: String },
    /// Export bookmarks to a file
    Export {
        /// Output format: jsonl (default) or csv
        #[arg(long, default_value = "jsonl")]
        format: String,
        /// Export mode: export (default, core fields) or backup (all fields)
        #[arg(long, default_value = "export")]
        mode: String,
        /// Write output to file (default: stdout). Format auto-detected from extension.
        #[arg(long, short)]
        output: Option<String>,
    },
    /// Import bookmarks from a file
    Import {
        /// Path to the file to import
        file: String,
        /// File format: jsonl (default) or csv. Auto-detected from extension if omitted.
        #[arg(long)]
        format: Option<String>,
        /// Import mode: import (default) or restore
        #[arg(long, default_value = "import")]
        mode: String,
        /// Conflict strategy: upsert (default) or skip
        #[arg(long, default_value = "upsert")]
        strategy: String,
    },
    /// Upgrade boop to the latest version
    Upgrade,
    /// Configure CLI
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage bookmark images
    Images {
        #[command(subcommand)]
        command: ImagesCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ImagesCommands {
    /// Fetch missing or broken bookmark images
    Fix,
}

#[derive(serde::Deserialize, Debug)]
struct FixProgress {
    checked: usize,
    total: usize,
    fixed: usize,
    failed: usize,
    done: bool,
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

    async fn post_multipart(
        &self,
        path: &str,
        file_bytes: Vec<u8>,
        filename: &str,
        mime: &str,
    ) -> Result<reqwest::Response, String> {
        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename.to_string())
            .mime_str(mime)
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        self.client
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())
    }
}

#[derive(Serialize)]
struct CreateBookmarkRequest {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Serialize)]
struct UpdateBookmarkRequest {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Serialize)]
struct SuggestRequest {
    url: String,
}

#[derive(Deserialize)]
struct SuggestResponse {
    title: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    #[allow(dead_code)]
    image_url: Option<String>,
    domain: Option<String>,
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

        Commands::Add { url, title, description, tags, suggest } => {
            let client = AppConfig::load().client()?;
            let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let body = CreateBookmarkRequest { url, title, description, tags };
            let path = if suggest { "/bookmarks?suggest=true" } else { "/bookmarks" };
            let resp = client.post_json(path, &body).await?;
            if resp.status().is_success() {
                let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
                println!("Added: {} ({})", bm.title.unwrap_or(bm.url), bm.id);
                if let Some(desc) = &bm.description {
                    println!("  {desc}");
                }
                if !bm.tags.is_empty() {
                    println!("  [{}]", bm.tags.join(", "));
                }
            } else {
                eprintln!("Failed: {}", resp.status());
            }
            Ok(())
        }

        Commands::Edit { id, title, description, tags, suggest } => {
            // Validate id is a valid UUID
            uuid::Uuid::parse_str(&id)
                .map_err(|_| format!("Invalid bookmark ID: '{id}' is not a valid UUID"))?;
            if !suggest && title.is_none() && description.is_none() && tags.is_none() {
                eprintln!("Nothing to update. Provide --title, --description, --tags, or --suggest.");
                return Ok(());
            }
            let client = AppConfig::load().client()?;
            let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let body = UpdateBookmarkRequest { title, description, tags };
            let path = if suggest {
                format!("/bookmarks/{id}?suggest=true")
            } else {
                format!("/bookmarks/{id}")
            };
            let resp = client.put_json(&path, &body).await?;
            if resp.status().is_success() {
                let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
                println!("Updated: {} ({})", bm.title.unwrap_or(bm.url), bm.id);
                if let Some(desc) = &bm.description {
                    println!("  {desc}");
                }
                if !bm.tags.is_empty() {
                    println!("  [{}]", bm.tags.join(", "));
                }
            } else {
                eprintln!("Failed: {}", resp.status());
            }
            Ok(())
        }

        Commands::Suggest { url } => {
            let client = AppConfig::load().client()?;
            let body = SuggestRequest { url };
            let resp = client.post_json("/bookmarks/suggest", &body).await?;
            if resp.status().is_success() {
                let s: SuggestResponse = resp.json().await.map_err(|e| e.to_string())?;
                if let Some(title) = &s.title {
                    println!("Title: {title}");
                }
                if let Some(desc) = &s.description {
                    println!("Description: {desc}");
                }
                if !s.tags.is_empty() {
                    println!("Tags: {}", s.tags.join(", "));
                }
                if let Some(domain) = &s.domain {
                    println!("Domain: {domain}");
                }
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

        Commands::Export { format, mode, output } => {
            let client = AppConfig::load().client()?;

            let format = if let Some(ref path) = output {
                if path.ends_with(".csv") { "csv".to_string() } else { format }
            } else {
                format
            };

            let url = format!("/bookmarks/export?format={format}&mode={mode}");
            let resp = client.get(&url).await?;
            if !resp.status().is_success() {
                return Err(format!("export failed: HTTP {}", resp.status()));
            }
            let body = resp.text().await.map_err(|e| e.to_string())?;

            match output {
                Some(path) => {
                    std::fs::write(&path, &body).map_err(|e| e.to_string())?;
                    eprintln!("Exported to {path}");
                }
                None => print!("{body}"),
            }
            Ok(())
        }

        Commands::Import { file, format, mode, strategy } => {
            let client = AppConfig::load().client()?;

            let format = format.unwrap_or_else(|| {
                if file.ends_with(".csv") { "csv".to_string() } else { "jsonl".to_string() }
            });
            let mime = if format == "csv" { "text/csv" } else { "application/x-ndjson" };

            let bytes = std::fs::read(&file).map_err(|e| format!("failed to read {file}: {e}"))?;
            let filename = std::path::Path::new(&file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let url = format!("/bookmarks/import?format={format}&mode={mode}&strategy={strategy}");
            let resp = client.post_multipart(&url, bytes, &filename, mime).await?;

            #[derive(serde::Deserialize)]
            struct ImportResult {
                created: usize,
                updated: usize,
                skipped: usize,
                errors: Vec<serde_json::Value>,
            }

            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("import failed: {body}"));
            }

            let result: ImportResult = resp.json().await.map_err(|e| e.to_string())?;
            println!(
                "Created: {}, Updated: {}, Skipped: {}, Errors: {}",
                result.created,
                result.updated,
                result.skipped,
                result.errors.len()
            );
            if !result.errors.is_empty() {
                for err in &result.errors {
                    eprintln!("  error: {err}");
                }
            }
            Ok(())
        }

        Commands::Upgrade => upgrade().await,

        Commands::Images { command } => match command {
            ImagesCommands::Fix => {
                let api = AppConfig::load().client()?;

                let response = api
                    .client
                    .post(api.url("/bookmarks/fix-images"))
                    .bearer_auth(&api.api_key)
                    .header("Accept", "text/event-stream")
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;

                if response.status() == reqwest::StatusCode::CONFLICT {
                    eprintln!("A fix-images job is already running for your account.");
                    std::process::exit(1);
                }

                if !response.status().is_success() {
                    eprintln!("Error: server returned {}", response.status());
                    std::process::exit(1);
                }

                use futures::StreamExt;
                let mut stream = response.bytes_stream();
                let mut buf = String::new();

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| e.to_string())?;
                    buf.push_str(&String::from_utf8_lossy(&chunk));

                    loop {
                        match buf.find('\n') {
                            None => break,
                            Some(pos) => {
                                let line = buf[..pos].trim().to_string();
                                buf.drain(..=pos);

                                if let Some(json_str) = line.strip_prefix("data: ")
                                    && let Ok(event) =
                                        serde_json::from_str::<FixProgress>(json_str)
                                    {
                                        if event.done {
                                            println!(
                                                "\nDone. Fixed {} images. {} failed (no image found).",
                                                event.fixed, event.failed
                                            );
                                            return Ok(());
                                        } else {
                                            print!(
                                                "\rChecking images: {} / {} — Fixed: {} — Failed: {}   ",
                                                event.checked,
                                                event.total,
                                                event.fixed,
                                                event.failed
                                            );
                                            use std::io::Write;
                                            std::io::stdout().flush().ok();
                                        }
                                    }
                            }
                        }
                    }
                }
                Ok(())
            }
        },
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

fn try_gh_download(asset_name: &str, staging_path: &std::path::Path) -> Result<(), String> {
    use std::process::Command;
    let output = Command::new("gh")
        .args([
            "release", "download", "--repo", "foundra-build/boopmark",
            "--pattern", asset_name, "--dir",
            staging_path.parent().unwrap().to_str().unwrap_or("."),
            "--clobber",
        ])
        .output()
        .map_err(|e| format!("gh not available: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    // gh downloads with the asset name; rename to staging path
    let downloaded = staging_path.parent().unwrap().join(asset_name);
    std::fs::rename(&downloaded, staging_path)
        .map_err(|e| format!("Failed to rename downloaded file: {e}"))?;
    Ok(())
}

async fn download_with_reqwest(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("boop/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let mut request = client.get(url);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.header("Authorization", format!("token {token}"));
    }

    let resp = request.send().await.map_err(|e| format!("Download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await.map_err(|e| format!("Failed to read response: {e}"))?;
    Ok(bytes.to_vec())
}

async fn upgrade() -> Result<(), String> {
    let target = detect_target()?;
    let asset_name = format!("boop-{target}");
    let exe_path = std::env::current_exe().map_err(|e| format!("Cannot find current exe: {e}"))?;
    let staging_path = exe_path.with_extension(format!("tmp.{}", std::process::id()));

    println!("Downloading latest boop for {target}...");

    // Try gh CLI first (handles private repo auth natively)
    if let Err(_gh_err) = try_gh_download(&asset_name, &staging_path) {
        // Fall back to reqwest (uses GITHUB_TOKEN env var if set)
        let url = format!(
            "https://github.com/foundra-build/boopmark/releases/latest/download/{asset_name}"
        );
        let bytes = download_with_reqwest(&url).await?;
        std::fs::write(&staging_path, &bytes)
            .map_err(|e| format!("Failed to write staging file: {e}"))?;
    }

    let result = (|| -> Result<(), String> {
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

        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&staging_path);
        return Err(e);
    }

    println!("Successfully upgraded boop! Run `boop --version` to verify.");
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

    #[test]
    fn test_cli_edit_recognized() {
        let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--suggest"]).unwrap();
        assert!(matches!(cli.command, Commands::Edit { suggest: true, .. }));
    }

    #[test]
    fn test_cli_edit_without_suggest() {
        let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--title", "New Title"]).unwrap();
        assert!(matches!(cli.command, Commands::Edit { suggest: false, .. }));
    }

    #[test]
    fn test_cli_suggest_recognized() {
        let cli = Cli::try_parse_from(["boop", "suggest", "https://example.com"]).unwrap();
        assert!(matches!(cli.command, Commands::Suggest { .. }));
    }

    #[test]
    fn test_cli_add_with_description() {
        let cli = Cli::try_parse_from(["boop", "add", "https://example.com", "--description", "A test"]).unwrap();
        assert!(matches!(cli.command, Commands::Add { .. }));
    }

    #[test]
    fn test_cli_add_with_suggest() {
        let cli = Cli::try_parse_from(["boop", "add", "https://example.com", "--suggest"]).unwrap();
        assert!(matches!(cli.command, Commands::Add { suggest: true, .. }));
    }

    #[test]
    fn test_cli_edit_with_description() {
        let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--description", "A desc"]).unwrap();
        assert!(matches!(cli.command, Commands::Edit { suggest: false, .. }));
    }

    #[test]
    fn test_cli_edit_with_tags() {
        let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--tags", "a,b"]).unwrap();
        assert!(matches!(cli.command, Commands::Edit { suggest: false, .. }));
    }

    #[test]
    fn test_cli_export_default() {
        let cli = Cli::try_parse_from(["boop", "export"]).unwrap();
        assert!(matches!(cli.command, Commands::Export { .. }));
    }

    #[test]
    fn test_cli_export_with_options() {
        let cli =
            Cli::try_parse_from(["boop", "export", "--format", "csv", "--mode", "backup", "-o", "out.csv"])
                .unwrap();
        match cli.command {
            Commands::Export { format, mode, output } => {
                assert_eq!(format, "csv");
                assert_eq!(mode, "backup");
                assert_eq!(output.as_deref(), Some("out.csv"));
            }
            _ => panic!("expected Export"),
        }
    }

    #[test]
    fn test_cli_import_with_file() {
        let cli = Cli::try_parse_from(["boop", "import", "bookmarks.jsonl"]).unwrap();
        assert!(matches!(cli.command, Commands::Import { .. }));
    }

    #[test]
    fn test_cli_import_with_all_options() {
        let cli = Cli::try_parse_from([
            "boop", "import", "data.csv", "--format", "csv", "--mode", "restore", "--strategy",
            "skip",
        ])
        .unwrap();
        match cli.command {
            Commands::Import { file, format, mode, strategy } => {
                assert_eq!(file, "data.csv");
                assert_eq!(format.as_deref(), Some("csv"));
                assert_eq!(mode, "restore");
                assert_eq!(strategy, "skip");
            }
            _ => panic!("expected Import"),
        }
    }
}
