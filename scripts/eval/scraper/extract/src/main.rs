//! Runs the server's article code outside the server.
//!
//!   scraper-eval-extract extract <url> <html-file>   Readability on saved HTML
//!   scraper-eval-extract fetch <url>                 the server's guarded fetch
//!
//! Prints one JSON object: ok, text_chars, error, cf_challenge, elapsed_ms,
//! and the first 300 characters of the text (for the quality flag).
#![allow(dead_code, unused_imports)]

#[path = "../../../../../server/src/adapters/net.rs"]
mod srv_net;
#[path = "../../../../../server/src/adapters/metadata/html.rs"]
mod srv_html;
#[path = "../../../../../server/src/adapters/article/readability.rs"]
mod srv_readability;
#[path = "../../../../../server/src/domain/error.rs"]
mod srv_error;
#[path = "../../../../../server/src/domain/bookmark.rs"]
mod srv_bookmark;
#[path = "../../../../../server/src/domain/search.rs"]
mod srv_search;
#[path = "../../../../../server/src/domain/ports/article_fetcher.rs"]
mod srv_port_article;
#[path = "../../../../../server/src/domain/ports/metadata.rs"]
mod srv_port_metadata;

/// The module paths the server files use (`crate::adapters::net`, ...).
mod adapters {
    pub(crate) use crate::srv_net as net;
    pub mod metadata {
        pub(crate) use crate::srv_html as html;
    }
    pub mod article {
        pub(crate) use crate::srv_readability as readability;
    }
}
mod domain {
    pub(crate) use crate::srv_bookmark as bookmark;
    pub(crate) use crate::srv_error as error;
    pub(crate) use crate::srv_search as search;
    pub mod ports {
        pub(crate) use crate::srv_port_article as article_fetcher;
        pub(crate) use crate::srv_port_metadata as metadata;
    }
}

use domain::ports::article_fetcher::ArticleFetcher;
use std::time::Instant;

fn report(result: Result<String, String>, cf: bool, start: Instant) {
    let (ok, text, error) = match result {
        Ok(t) => (true, t, None),
        Err(e) => (false, String::new(), Some(e)),
    };
    let json = serde_json::json!({
        "ok": ok,
        "text_chars": text.chars().count(),
        "text_head": text.chars().take(300).collect::<String>(),
        "text_tail": text.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>(),
        "error": error,
        "cf_challenge": cf,
        "elapsed_ms": start.elapsed().as_millis() as u64,
    });
    println!("{json}");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let start = Instant::now();
    match args.get(1).map(String::as_str) {
        Some("extract") => {
            let html = String::from_utf8_lossy(&std::fs::read(&args[3]).expect("read html")).into_owned();
            let cf = srv_html::is_cloudflare_challenge(&html);
            let result = srv_readability::extract_text(&html, &args[2]).map_err(|e| e.to_string());
            report(result, cf, start);
        }
        Some("fetch") => {
            let fetcher = srv_readability::HtmlArticleFetcher::new(1);
            let result = fetcher.fetch(&args[2]).await.map(|a| a.text).map_err(|e| e.to_string());
            let cf = matches!(&result, Err(e) if e == domain::error::CF_CHALLENGE_MSG);
            report(result, cf, start);
        }
        _ => {
            eprintln!("usage: scraper-eval-extract extract <url> <html-file> | fetch <url>");
            std::process::exit(2);
        }
    }
}
