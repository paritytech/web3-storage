// SPDX-License-Identifier: Apache-2.0

use crate::cli::GlobalArgs;
use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use provider_types::KeyScheme;
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use storage_client::{ClientConfig, Signer};

/// Bucket identifier on chain (alias of `storage_primitives::BucketId`, a `u64`).
pub type BucketId = u64;

/// The secret resolved from `--suri` or `--keyfile`, and the scheme to sign
/// provider messages with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedKey {
    /// The seed phrase or derivation path.
    pub seed: String,
    /// The signing scheme: `--key-scheme` when given, else a JSON keyfile's
    /// `keyScheme`, else sr25519.
    pub scheme: KeyScheme,
}

/// Resolve the secret from either `--suri` or `--keyfile` (exactly one is
/// required). A keyfile is the JSON file `provider generate-key` writes or a
/// plain seed; it must not be group- or world-readable.
pub fn resolve_key(global: &GlobalArgs, scheme_flag: Option<KeyScheme>) -> Result<LoadedKey> {
    let (seed, file_scheme) = match (&global.suri, &global.keyfile) {
        (Some(suri), _) => (suri.clone(), None),
        (None, Some(path)) => {
            let secret = provider_helpers::read_keyfile(path)?;
            let scheme = secret
                .key_scheme
                .map(|name| {
                    KeyScheme::from_str(&name, true).map_err(|_| {
                        anyhow!("keyfile {} has unknown keyScheme {name:?}", path.display())
                    })
                })
                .transpose()?;
            (secret.seed, scheme)
        }
        (None, None) => bail!("one of --suri or --keyfile is required"),
    };
    Ok(LoadedKey {
        seed,
        scheme: scheme_flag.or(file_scheme).unwrap_or_default(),
    })
}

/// Resolve the SURI from either `--suri` or `--keyfile` (exactly one is required)
pub fn resolve_suri(global: &GlobalArgs) -> Result<String> {
    Ok(resolve_key(global, None)?.seed)
}

/// The name of `scheme` as `--key-scheme` and a keyfile's `keyScheme`
/// spell it: the clap value name, so what is written is what `resolve_key`
/// parses.
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

/// Derive the signer from `--suri`/`--keyfile`. It both identifies the account
/// being operated on and authenticates every provider request.
pub fn resolve_signer(global: &GlobalArgs) -> Result<Signer> {
    let suri = resolve_suri(global)?;
    Signer::from_seed(&suri).context("failed to derive keypair from SURI")
}

/// The SS58 address of the signer's account.
pub fn account_ss58(signer: &Signer) -> String {
    AccountId32::from(signer.keypair().public_key().0).to_ss58check()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Build `GlobalArgs` with placeholder connection flags and the given
    /// identity inputs — only `suri`/`keyfile` matter for `resolve_key`.
    pub(crate) fn global_args(suri: Option<String>, keyfile: Option<PathBuf>) -> GlobalArgs {
        GlobalArgs {
            chain_rpc: "ws://127.0.0.1:2222".to_string(),
            provider_url: "http://127.0.0.1:3333".to_string(),
            suri,
            keyfile,
            output: crate::metrics::OutputFormat::Text,
        }
    }

    /// Write `contents` with the permissions `--keyfile` requires.
    fn write_keyfile(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    /// A JSON keyfile supplies the seed and, unless `--key-scheme` is given,
    /// the scheme; `--suri` and a plain seed file default to sr25519.
    #[test]
    fn resolve_key_reads_scheme_from_json_keyfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");
        write_keyfile(
            &path,
            r#"{"secretPhrase": "//Dave", "keyScheme": "ecdsa", "ss58Address": "5abc"}"#,
        );
        assert_eq!(
            resolve_key(&global_args(None, Some(path.clone())), None).unwrap(),
            LoadedKey {
                seed: "//Dave".to_string(),
                scheme: KeyScheme::Ecdsa,
            }
        );
        assert_eq!(
            resolve_key(
                &global_args(None, Some(path.clone())),
                Some(KeyScheme::Ed25519)
            )
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
            resolve_key(&global_args(None, Some(path.clone())), None)
                .unwrap()
                .scheme,
            KeyScheme::Sr25519
        );

        write_keyfile(&path, r#"{"secretPhrase": "//Dave", "keyScheme": "rsa"}"#);
        let err = resolve_key(&global_args(None, Some(path)), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown keyScheme"), "unexpected error: {err}");

        assert_eq!(
            resolve_key(&global_args(Some("//Alice".to_string()), None), None).unwrap(),
            LoadedKey {
                seed: "//Alice".to_string(),
                scheme: KeyScheme::Sr25519,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_key_rejects_world_readable_keyfile() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seed");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = resolve_suri(&global_args(None, Some(path)))
            .unwrap_err()
            .to_string();
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

    #[test]
    fn returns_inline_suri() {
        let args = global_args(Some("//Alice".to_string()), None);
        assert_eq!(resolve_suri(&args).unwrap(), "//Alice");
    }

    #[test]
    fn reads_and_trims_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "  //Bob").unwrap();
        let args = global_args(None, Some(file.path().to_path_buf()));
        assert_eq!(resolve_suri(&args).unwrap(), "//Bob");
    }

    #[test]
    fn rejects_empty_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "   ").unwrap();
        let args = global_args(None, Some(file.path().to_path_buf()));
        let err = resolve_suri(&args).unwrap_err().to_string();
        assert!(err.contains("is empty"), "unexpected error: {err}");
    }

    #[test]
    fn errors_on_missing_keyfile() {
        let path = std::env::temp_dir().join("storage-cli-resolve-suri-does-not-exist");
        let args = global_args(None, Some(path));
        let err = resolve_suri(&args).unwrap_err().to_string();
        assert!(
            err.contains("cannot read keyfile"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn errors_when_neither_provided() {
        let args = global_args(None, None);
        let err = resolve_suri(&args).unwrap_err().to_string();
        assert!(
            err.contains("--suri or --keyfile"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn inline_suri_wins_over_keyfile() {
        // clap's `conflicts_with` blocks this at parse time; this only pins
        // the `(Some, _)` arm's precedence within the function itself.
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "//FromFile").unwrap();
        let args = global_args(
            Some("//Inline".to_string()),
            Some(file.path().to_path_buf()),
        );
        assert_eq!(resolve_suri(&args).unwrap(), "//Inline");
    }
}
