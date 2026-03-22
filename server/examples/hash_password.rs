//! Tiny helper: reads a password from stdin (or argv) and prints its argon2 hash.
//!
//! Prefer stdin to avoid exposing the password in process listings:
//!   echo "mypassword" | hash_password
//!
//! Argv is also supported for scripting convenience:
//!   hash_password mypassword
use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};

fn main() {
    let password = if let Some(arg) = std::env::args().nth(1) {
        arg
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .expect("failed to read password from stdin");
        line.trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string()
    };

    if password.is_empty() {
        eprintln!("usage: hash_password <password>  OR  echo <password> | hash_password");
        std::process::exit(1);
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("failed to hash password")
        .to_string();
    print!("{hash}");
}
