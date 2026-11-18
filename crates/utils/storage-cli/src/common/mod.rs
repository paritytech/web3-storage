// SPDX-License-Identifier: Apache-2.0

use crate::cli::GlobalArgs;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, ValueEnum};
use provider_types::KeyScheme;
use sp_core::crypto::Ss58Codec;
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

    /// File with the client account's SURI, or the JSON keyfile `provider
    /// generate-key` writes. It must not be group- or world-readable
    /// (permissions <= 0600 on Unix).
    #[arg(long, value_name = "FILE")]
    pub client_keyfile: Option<PathBuf>,
}

impl ClientKey {
    pub fn suri(&self) -> Result<String> {
        let (seed, _) =
            resolve_secret(self.client_suri.as_deref(), self.client_keyfile.as_deref())?;
        Ok(seed)
    }
}

/// Secret of the provider account.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct ProviderKey {
    /// Secret URI (SURI) of the provider account, e.g. "//Alice".
    #[arg(long, value_name = "SURI")]
    pub provider_suri: Option<String>,

    /// File with the provider account's SURI, or the JSON keyfile `provider
    /// generate-key` writes: the file the provider node's `--keyfile` reads.
    /// It must not be group- or world-readable (permissions <= 0600 on Unix).
    #[arg(long, value_name = "FILE")]
    pub provider_keyfile: Option<PathBuf>,
}

impl ProviderKey {
    /// The provider secret and the scheme to sign provider messages with:
    /// `scheme_flag` when given, else a JSON keyfile's `keyScheme`, else
    /// sr25519.
    pub fn load(&self, scheme_flag: Option<KeyScheme>) -> Result<LoadedKey> {
        let (seed, file_scheme) = resolve_secret(
            self.provider_suri.as_deref(),
            self.provider_keyfile.as_deref(),
        )?;
        Ok(LoadedKey {
            seed,
            scheme: scheme_flag.or(file_scheme).unwrap_or_default(),
        })
    }
}

/// The secret resolved from a SURI or keyfile flag, and the scheme to sign
/// provider messages with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedKey {
    /// The seed phrase or derivation path.
    pub seed: String,
    /// The signing scheme: `--key-scheme` when given, else a JSON keyfile's
    /// `keyScheme`, else sr25519.
    pub scheme: KeyScheme,
}

/// The SURI given inline, or else the secret in the keyfile and the
/// `keyScheme` it records. A keyfile is the JSON file `provider generate-key`
/// writes or a plain seed; it must not be group- or world-readable.
fn resolve_secret(
    suri: Option<&str>,
    keyfile: Option<&Path>,
) -> Result<(String, Option<KeyScheme>)> {
    match (suri, keyfile) {
        (Some(suri), _) => Ok((suri.to_string(), None)),
        (None, Some(path)) => {
            let secret = provider_types::read_keyfile(path)?;
            let scheme = secret
                .key_scheme
                .map(|name| {
                    KeyScheme::from_str(&name, true).map_err(|_| {
                        anyhow!("keyfile {} has unknown keyScheme {name:?}", path.display())
                    })
                })
                .transpose()?;
            Ok((secret.seed, scheme))
        }
        (None, None) => bail!("no SURI or keyfile given"),
    }
}

/// The name of `scheme` as `--key-scheme` and a keyfile's `keyScheme`
/// spell it: the clap value name, so what is written is what
/// `resolve_secret` parses.
pub fn key_scheme_name(scheme: KeyScheme) -> String {
    scheme
        .to_possible_value()
        .expect("every KeyScheme variant has a clap value")
        .get_name()
        .to_string()
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
    signer.account_id().to_ss58check()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A `ProviderKey` with only `--provider-keyfile` set.
    pub(crate) fn provider_keyfile(path: PathBuf) -> ProviderKey {
        ProviderKey {
            provider_suri: None,
            provider_keyfile: Some(path),
        }
    }

    /// Write `contents` with the permissions a keyfile flag requires.
    fn write_keyfile(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    /// A JSON keyfile supplies the seed and, unless `--key-scheme` is given,
    /// the scheme; `--provider-suri` and a plain seed file default to sr25519.
    #[test]
    fn provider_key_reads_scheme_from_json_keyfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");
        write_keyfile(
            &path,
            r#"{"secretPhrase": "//Dave", "keyScheme": "ecdsa", "ss58Address": "5abc"}"#,
        );
        assert_eq!(
            provider_keyfile(path.clone()).load(None).unwrap(),
            LoadedKey {
                seed: "//Dave".to_string(),
                scheme: KeyScheme::Ecdsa,
            }
        );
        assert_eq!(
            provider_keyfile(path.clone())
                .load(Some(KeyScheme::Ed25519))
                .unwrap()
                .scheme,
            KeyScheme::Ed25519
        );

        // No keyScheme (as subkey writes it): sr25519.
        write_keyfile(
            &path,
            r#"{"secretPhrase": "//Dave", "networkId": "substrate"}"#,
        );
        assert_eq!(
            provider_keyfile(path.clone()).load(None).unwrap().scheme,
            KeyScheme::Sr25519
        );

        write_keyfile(&path, r#"{"secretPhrase": "//Dave", "keyScheme": "rsa"}"#);
        let err = provider_keyfile(path).load(None).unwrap_err().to_string();
        assert!(err.contains("unknown keyScheme"), "unexpected error: {err}");

        let inline = ProviderKey {
            provider_suri: Some("//Alice".to_string()),
            provider_keyfile: None,
        };
        assert_eq!(
            inline.load(None).unwrap(),
            LoadedKey {
                seed: "//Alice".to_string(),
                scheme: KeyScheme::Sr25519,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_readable_keyfile() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seed");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = resolve_secret(None, Some(&path)).unwrap_err().to_string();
        assert!(
            err.contains("insecure permissions"),
            "unexpected error: {err}"
        );
    }

    /// The written scheme name is the clap value name, for every scheme.
    #[test]
    fn key_scheme_names_round_trip() {
        for scheme in [
            KeyScheme::Sr25519,
            KeyScheme::Ed25519,
            KeyScheme::Ecdsa,
            KeyScheme::Eth,
        ] {
            let name = key_scheme_name(scheme);
            assert_eq!(KeyScheme::from_str(&name, true).unwrap(), scheme);
        }
        assert_eq!(key_scheme_name(KeyScheme::Sr25519), "sr25519");
    }

    /// The printed address is the signer's account under the generic prefix.
    #[test]
    fn account_ss58_is_the_signer_account() {
        let signer = Signer::from_seed("//Alice").unwrap();
        assert_eq!(
            account_ss58(&signer),
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
        );
    }

    #[test]
    fn returns_inline_suri() {
        assert_eq!(resolve_secret(Some("//Alice"), None).unwrap().0, "//Alice");
    }

    #[test]
    fn reads_and_trims_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "  //Bob").unwrap();
        assert_eq!(resolve_secret(None, Some(file.path())).unwrap().0, "//Bob");
    }

    #[test]
    fn rejects_empty_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "   ").unwrap();
        let err = resolve_secret(None, Some(file.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("is empty"), "unexpected error: {err}");
    }

    #[test]
    fn errors_on_missing_keyfile() {
        let path = std::env::temp_dir().join("storage-cli-resolve-suri-does-not-exist");
        let err = resolve_secret(None, Some(&path)).unwrap_err().to_string();
        assert!(
            err.contains("cannot read keyfile"),
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
        assert_eq!(wrapper.key.load(None).unwrap().seed, "//Alice");
    }
}
