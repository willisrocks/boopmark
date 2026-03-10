//! Tiny helper: reads a password from argv and prints its argon2 hash.
use argon2::{Argon2, PasswordHasher, password_hash::{SaltString, rand_core::OsRng}};

fn main() {
    let password = std::env::args().nth(1).expect("usage: hash_password <password>");
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("failed to hash password")
        .to_string();
    print!("{hash}");
}
