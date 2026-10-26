//! Re-encrypts every stored provider API key under a new LLM_SETTINGS_ENCRYPTION_KEY.
//!
//! All rows are decrypted with the old key, re-encrypted with the new key, and
//! verified against the new key inside one transaction. Any failure rolls back
//! and leaves the database untouched.
//!
//!   DATABASE_URL=... OLD_LLM_SETTINGS_ENCRYPTION_KEY=... \
//!   NEW_LLM_SETTINGS_ENCRYPTION_KEY=... rotate_encryption_key
#[path = "../src/app/secrets.rs"]
mod secrets;

use secrets::SecretBox;
use sqlx::Row;
use uuid::Uuid;

const COLUMNS: [&str; 3] = [
    "anthropic_api_key_encrypted",
    "openai_api_key_encrypted",
    "gemini_api_key_encrypted",
];

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("{name} is required");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let old_key = required_env("OLD_LLM_SETTINGS_ENCRYPTION_KEY");
    let new_key = required_env("NEW_LLM_SETTINGS_ENCRYPTION_KEY");
    if old_key.trim() == new_key.trim() {
        return Err("old and new keys are identical".into());
    }
    let old = SecretBox::new(&old_key);
    let new = SecretBox::new(&new_key);

    let pool = sqlx::PgPool::connect(&required_env("DATABASE_URL")).await?;
    let mut tx = pool.begin().await?;
    let mut rotated = 0;

    for column in COLUMNS {
        let rows = sqlx::query(&format!(
            "SELECT user_id, {column} FROM user_llm_settings WHERE {column} IS NOT NULL FOR UPDATE"
        ))
        .fetch_all(&mut *tx)
        .await?;

        for row in rows {
            let user_id: Uuid = row.get(0);
            let blob: Vec<u8> = row.get(1);
            let plaintext = old
                .decrypt(&blob)
                .map_err(|e| format!("{column} for user {user_id}: {e} (wrong old key?)"))?;
            let reencrypted = new.encrypt(&plaintext)?;
            if new.decrypt(&reencrypted)? != plaintext {
                return Err(format!("{column} for user {user_id}: verification failed").into());
            }

            sqlx::query(&format!(
                "UPDATE user_llm_settings SET {column} = $1 WHERE user_id = $2"
            ))
            .bind(&reencrypted)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            rotated += 1;
        }
    }

    tx.commit().await?;
    println!("re-encrypted {rotated} key(s)");
    Ok(())
}
