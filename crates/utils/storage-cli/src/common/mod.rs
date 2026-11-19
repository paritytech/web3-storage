// SPDX-License-Identifier: Apache-2.0

use crate::cli::GlobalArgs;
use anyhow::{bail, Context, Result};
use clap::Args;
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use std::path::{Path, PathBuf};
use storage_client::{ClientConfig, Signer};

/// Bucket identifier on chain (alias of `storage_primitives::BucketId`, a `u64`).
pub type BucketId = u64;

/// Secret of the client account: it owns or is a member of the target buckets,
/// pays for prepared agreements and signs every provider request.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct ClientKey {
    /// Secret URI (SURI) of the client account, e.g. "//Bob".
    #[arg(long, value_name = "SURI")]
    pub client_suri: Option<String>,

    /// File whose contents are the client account's SURI.
    #[arg(long, value_name = "FILE")]
    pub client_keyfile: Option<PathBuf>,
}

impl ClientKey {
    pub fn suri(&self) -> Result<String> {
        resolve_suri(self.client_suri.as_deref(), self.client_keyfile.as_deref())
    }
}

/// Secret of the provider account.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct ProviderKey {
    /// Secret URI (SURI) of the provider account, e.g. "//Alice".
    #[arg(long, value_name = "SURI")]
    pub provider_suri: Option<String>,

    /// File whose contents are the provider account's SURI: the file the
    /// provider node's `--keyfile` reads.
    #[arg(long, value_name = "FILE")]
    pub provider_keyfile: Option<PathBuf>,
}

impl ProviderKey {
    pub fn suri(&self) -> Result<String> {
        resolve_suri(
            self.provider_suri.as_deref(),
            self.provider_keyfile.as_deref(),
        )
    }
}

/// The SURI given inline, or else the trimmed contents of the keyfile.
pub fn resolve_suri(suri: Option<&str>, keyfile: Option<&Path>) -> Result<String> {
    match (suri, keyfile) {
        (Some(suri), _) => Ok(suri.to_string()),
        (None, Some(path)) => {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read keyfile {}", path.display()))?;
            let suri = contents.trim().to_string();
            if suri.is_empty() {
                bail!("keyfile {} is empty", path.display());
            }
            Ok(suri)
        }
        (None, None) => bail!("no SURI or keyfile given"),
    }
}

/// Build the client config (chain RPC + provider URL) from the global flags.
pub fn build_config(global: &GlobalArgs) -> ClientConfig {
    ClientConfig {
        chain_ws_url: global.chain_rpc.clone(),
        provider_urls: vec![global.provider_url.clone()],
        ..Default::default()
    }
}

/// The sr25519 account signer for a SURI.
pub fn signer_from_suri(suri: &str) -> Result<Signer> {
    Signer::from_seed(suri).context("failed to derive keypair from SURI")
}

/// The SS58 address of the signer's account.
pub fn account_ss58(signer: &Signer) -> String {
    AccountId32::from(signer.keypair().public_key().0).to_ss58check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn returns_inline_suri() {
        assert_eq!(resolve_suri(Some("//Alice"), None).unwrap(), "//Alice");
    }

    #[test]
    fn reads_and_trims_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "  //Bob").unwrap();
        assert_eq!(resolve_suri(None, Some(file.path())).unwrap(), "//Bob");
    }

    #[test]
    fn rejects_empty_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "   ").unwrap();
        let err = resolve_suri(None, Some(file.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("is empty"), "unexpected error: {err}");
    }

    #[test]
    fn errors_on_missing_keyfile() {
        let path = std::env::temp_dir().join("storage-cli-resolve-suri-does-not-exist");
        let err = resolve_suri(None, Some(&path)).unwrap_err().to_string();
        assert!(
            err.contains("failed to read keyfile"),
            "unexpected error: {err}"
        );
    }

    /// Each command requires exactly one of its SURI and keyfile flags, so a
    /// run never signs as an account the operator did not pick.
    #[test]
    fn key_flags_require_exactly_one_source() {
        use clap::Parser;

        #[derive(Debug, Parser)]
        struct Wrapper {
            #[command(flatten)]
            key: ProviderKey,
        }

        let parse = |args: &[&str]| Wrapper::try_parse_from([&["cmd"], args].concat());
        assert!(parse(&[]).is_err());
        assert!(parse(&["--provider-suri", "//Alice", "--provider-keyfile", "k"]).is_err());
        let wrapper = parse(&["--provider-suri", "//Alice"]).unwrap();
        assert_eq!(wrapper.key.suri().unwrap(), "//Alice");
    }
}
