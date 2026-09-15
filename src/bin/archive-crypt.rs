//! Encrypt/decrypt the cold-archive bundle with `age` (ChaCha20-Poly1305) —
//! the one format that is genuinely portable across Windows/macOS/Linux
//! (decision #18, docs/WINDOWS_PORT_SYNTHESIS.md). Called from
//! `scripts/archive-documents.sh` / `scripts/restore-archive-documents.sh`,
//! gated behind `ARCHIVE_ENCRYPT=1`.
//!
//! Key custody today: the recipient (public key) comes from
//! `ARCHIVE_AGE_RECIPIENT`; the identity (private key) is read from a local
//! file named by `ARCHIVE_AGE_IDENTITY_FILE`, in the plain `age-keygen`
//! output format (a `# public key: age1...` comment line, then an
//! `AGE-SECRET-KEY-1...` line). Whether that file is placed by hand or
//! fetched from Keeper Secrets Manager is still an open question (§8 Q3/Q4,
//! docs/WINDOWS_PORT_SYNTHESIS.md) — this binary only does the crypto.

use age::x25519::{Identity, Recipient};
use age::{Decryptor, Encryptor};
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "archive-crypt",
    about = "Encrypt/decrypt cold-archive bundles with age"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Encrypt `input` to `output` for the recipient in ARCHIVE_AGE_RECIPIENT.
    Encrypt { input: PathBuf, output: PathBuf },
    /// Decrypt `input` to `output` using the identity in ARCHIVE_AGE_IDENTITY_FILE.
    Decrypt { input: PathBuf, output: PathBuf },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Encrypt { input, output } => encrypt_file(&input, &output),
        Command::Decrypt { input, output } => decrypt_file(&input, &output),
    }
}

fn load_recipient() -> Result<Recipient> {
    let raw = std::env::var("ARCHIVE_AGE_RECIPIENT")
        .context("ARCHIVE_AGE_RECIPIENT is required (an age public key, age1...)")?;
    raw.trim()
        .parse::<Recipient>()
        .map_err(|e| anyhow!("invalid ARCHIVE_AGE_RECIPIENT: {e}"))
}

/// Read an `age-keygen`-format identity file: comment lines starting with
/// `#` (including the public-key echo line) plus one `AGE-SECRET-KEY-1...`
/// line, in any order.
fn load_identity() -> Result<Identity> {
    let path = std::env::var("ARCHIVE_AGE_IDENTITY_FILE").context(
        "ARCHIVE_AGE_IDENTITY_FILE is required (path to an age identity/secret-key file)",
    )?;
    let contents =
        fs::read_to_string(&path).with_context(|| format!("reading identity file {path}"))?;
    let line = contents
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .ok_or_else(|| anyhow!("no identity line found in {path}"))?;
    line.trim()
        .parse::<Identity>()
        .map_err(|e| anyhow!("invalid identity in {path}: {e}"))
}

fn encrypt_bytes(plaintext: &[u8], recipient: &Recipient) -> Result<Vec<u8>> {
    let encryptor = Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
        .map_err(|e| anyhow!("failed to build age encryptor: {e}"))?;
    let mut encrypted = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| anyhow!("failed to start age stream: {e}"))?;
    writer.write_all(plaintext)?;
    writer
        .finish()
        .map_err(|e| anyhow!("failed to finalize age stream: {e}"))?;
    Ok(encrypted)
}

fn decrypt_bytes(ciphertext: &[u8], identity: &Identity) -> Result<Vec<u8>> {
    let decryptor = Decryptor::new(ciphertext).map_err(|e| anyhow!("not a valid age file: {e}"))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| anyhow!("failed to decrypt (wrong identity?): {e}"))?;
    let mut decrypted = Vec::new();
    reader.read_to_end(&mut decrypted)?;
    Ok(decrypted)
}

fn encrypt_file(input: &PathBuf, output: &PathBuf) -> Result<()> {
    let recipient = load_recipient()?;
    let plaintext = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let encrypted = encrypt_bytes(&plaintext, &recipient)?;
    fs::write(output, &encrypted).with_context(|| format!("writing {}", output.display()))?;
    Ok(())
}

fn decrypt_file(input: &PathBuf, output: &PathBuf) -> Result<()> {
    let identity = load_identity()?;
    let ciphertext = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let decrypted = decrypt_bytes(&ciphertext, &identity)?;
    fs::write(output, &decrypted).with_context(|| format!("writing {}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let identity = Identity::generate();
        let recipient = identity.to_public();
        let plaintext = b"memory-platform cold archive test bundle";

        let encrypted = encrypt_bytes(plaintext, &recipient).expect("encrypt");
        assert_ne!(encrypted, plaintext);
        let decrypted = decrypt_bytes(&encrypted, &identity).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_identity_fails() {
        let identity = Identity::generate();
        let recipient = identity.to_public();
        let wrong_identity = Identity::generate();
        let encrypted = encrypt_bytes(b"secret", &recipient).expect("encrypt");
        assert!(decrypt_bytes(&encrypted, &wrong_identity).is_err());
    }
}
