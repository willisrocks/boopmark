//! Run the deterministic tag normalizer over a tag/count list.
//!
//! Reads a JSON array of `[tag, count]` pairs from stdin and prints a JSON
//! object with the groups at each normalization level:
//!   cargo run -q -p boopmark-server --example tag_lab < counts.json
#[path = "../src/domain/tag_normalize.rs"]
mod tag_normalize;

use tag_normalize::{Level, group_tags};

fn main() {
    let tags: Vec<(String, i64)> = serde_json::from_reader(std::io::stdin())
        .expect("stdin must be a JSON array of [tag, count]");
    let mut out = serde_json::Map::new();
    for (name, level) in [
        ("case", Level::Case),
        ("compact", Level::Compact),
        ("plural", Level::Plural),
    ] {
        let groups: Vec<serde_json::Value> = group_tags(&tags, level)
            .into_iter()
            .map(|g| serde_json::json!({"canonical": g.canonical, "members": g.members}))
            .collect();
        out.insert(name.to_string(), serde_json::Value::Array(groups));
    }
    println!("{}", serde_json::Value::Object(out));
}
