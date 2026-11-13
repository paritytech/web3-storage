// SPDX-License-Identifier: Apache-2.0

//! Keyfile generation: a BIP39 mnemonic and the identity it derives,
//! written as JSON to a file only its owner can read.
//!
//! A BIP39 English mnemonic is the secret format every supported chain
//! derives accounts from, so generation is chain-agnostic. Which account
//! and public key a mnemonic yields is the chain's concern; the chain
//! binary records them in [`Keyfile::identity`].

use std::collections::BTreeMap;
use std::path::Path;

/// Word count of a generated mnemonic when the caller does not choose one.
///
/// 12 words (128 bits of entropy) is what `sp_core::Pair::generate_with_phrase`
/// and `subkey generate` produce, so a generated keyfile matches what
/// operators of other Substrate tools expect.
pub const DEFAULT_MNEMONIC_WORDS: usize = 12;

/// Word counts BIP39 defines, in ascending order.
pub const MNEMONIC_WORD_COUNTS: [usize; 5] = [12, 15, 18, 21, 24];

/// Contents of a keyfile: the secret, the signing scheme it is used with,
/// and the identity the chain derives from it.
///
/// Serialized as JSON with the field names `subkey generate --output-type
/// json` uses (`secretPhrase`, `publicKey`, `accountId`, `ss58Address`),
/// so the file reads like the output of the tools operators already know.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyfile {
    /// The BIP39 mnemonic. The only secret in the file.
    pub secret_phrase: String,
    /// Name of the signing scheme the provider registers with, e.g.
    /// `sr25519`; the chain binary parses it. Absent in files other tools
    /// write, such as `subkey generate --output-type json`; the chain
    /// binary then applies its default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_scheme: Option<String>,
    /// Chain-specific public identity fields, e.g. `ss58Address`,
    /// `accountId`, `publicKey`. Written for the operator's reference; the
    /// chain binary derives them from `secret_phrase` again when it runs.
    #[serde(flatten)]
    pub identity: BTreeMap<String, serde_json::Value>,
}

impl Keyfile {
    /// Parse a keyfile's JSON contents.
    pub fn from_json(contents: &str) -> Result<Self, KeyfileError> {
        serde_json::from_str(contents).map_err(|e| KeyfileError::Json(e.to_string()))
    }

    /// The JSON written to the keyfile, one field per line.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a Keyfile is a plain JSON object")
    }
}

/// Why a keyfile could not be generated or read.
#[derive(Debug, thiserror::Error)]
pub enum KeyfileError {
    /// The requested word count is not one of [`MNEMONIC_WORD_COUNTS`].
    #[error("invalid mnemonic word count {0}: BIP39 defines 12, 15, 18, 21 or 24")]
    WordCount(usize),

    /// The mnemonic library failed for another reason.
    #[error("mnemonic generation failed: {0}")]
    Mnemonic(String),

    /// The contents are not a JSON keyfile.
    #[error("not valid JSON: {0}")]
    Json(String),

    /// The target file exists. It is never overwritten: it may contain the
    /// only copy of a registered provider's secret.
    #[error("keyfile {0} exists; choose another path or remove it first")]
    Exists(String),

    /// Creating or writing the file failed.
    #[error("cannot write keyfile {path}: {source}")]
    Io {
        /// The target path.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// A new BIP39 English mnemonic of `words` words from the operating
/// system's randomness: the same `bip39::Mnemonic::generate` call
/// `sp_core::Pair::generate_with_phrase` and `subkey generate` make.
pub fn generate_mnemonic(words: usize) -> Result<String, KeyfileError> {
    bip39::Mnemonic::generate(words)
        .map(|mnemonic| mnemonic.to_string())
        .map_err(|e| match e {
            bip39::Error::BadWordCount(count) => KeyfileError::WordCount(count),
            other => KeyfileError::Mnemonic(other.to_string()),
        })
}

/// Write `keyfile` as JSON to `path`, which must not exist. On Unix the
/// file is created with mode 0600, the permission the node's `--keyfile`
/// check requires. A file whose write failed is removed again, so a retry
/// does not see a half-written file as an existing secret.
pub fn write_keyfile(path: &Path, keyfile: &Keyfile) -> Result<(), KeyfileError> {
    use std::io::Write;

    let io = |source| KeyfileError::Io {
        path: path.display().to_string(),
        source,
    };

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            KeyfileError::Exists(path.display().to_string())
        } else {
            io(e)
        }
    })?;
    let written = file
        .write_all(keyfile.to_json().as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    if written.is_err() {
        // Best effort: the write error is the one to report.
        let _ = std::fs::remove_file(path);
    }
    written.map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyfile() -> Keyfile {
        Keyfile {
            secret_phrase: generate_mnemonic(DEFAULT_MNEMONIC_WORDS).unwrap(),
            key_scheme: Some("sr25519".to_string()),
            identity: BTreeMap::from([
                ("ss58Address".to_string(), "5abc".into()),
                ("ss58Prefix".to_string(), 42.into()),
            ]),
        }
    }

    #[test]
    fn generates_valid_mnemonics_of_every_bip39_length() {
        for words in MNEMONIC_WORD_COUNTS {
            let mnemonic = generate_mnemonic(words).unwrap();
            assert_eq!(mnemonic.split(' ').count(), words);
            bip39::Mnemonic::parse_in(bip39::Language::English, &mnemonic).unwrap();
        }
        assert_ne!(
            generate_mnemonic(12).unwrap(),
            generate_mnemonic(12).unwrap()
        );
    }

    #[test]
    fn rejects_other_word_counts() {
        for words in [0, 6, 13, 25] {
            assert!(matches!(
                generate_mnemonic(words),
                Err(KeyfileError::WordCount(w)) if w == words
            ));
        }
    }

    /// The JSON uses subkey's field names, with the identity fields at the
    /// top level, and parses back to the same value.
    #[test]
    fn keyfile_json_round_trips_with_subkey_field_names() {
        let keyfile = keyfile();
        let json = keyfile.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["secretPhrase"], keyfile.secret_phrase);
        assert_eq!(value["keyScheme"], "sr25519");
        assert_eq!(value["ss58Address"], "5abc");
        assert_eq!(value["ss58Prefix"], 42);
        assert_eq!(Keyfile::from_json(&json).unwrap(), keyfile);

        // subkey's output has no keyScheme; the secret is the only
        // required field.
        let subkey =
            Keyfile::from_json(r#"{"secretPhrase": "//Alice", "networkId": "substrate"}"#).unwrap();
        assert_eq!(subkey.key_scheme, None);
        assert_eq!(subkey.secret_phrase, "//Alice");
        assert!(!subkey.to_json().contains("keyScheme"));

        let err = Keyfile::from_json("//Alice").unwrap_err();
        assert!(matches!(err, KeyfileError::Json(_)), "{err}");
        let err = Keyfile::from_json(r#"{"keyScheme": "sr25519"}"#).unwrap_err();
        assert!(matches!(err, KeyfileError::Json(_)), "{err}");
    }

    #[test]
    fn writes_owner_only_file_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");
        let keyfile = keyfile();

        write_keyfile(&path, &keyfile).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, format!("{}\n", keyfile.to_json()));
        assert_eq!(Keyfile::from_json(&contents).unwrap(), keyfile);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
        }

        let other = Keyfile {
            secret_phrase: "other".to_string(),
            ..keyfile.clone()
        };
        let err = write_keyfile(&path, &other).unwrap_err();
        assert!(matches!(err, KeyfileError::Exists(_)), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            contents,
            "existing keyfile must be untouched"
        );
    }

    #[test]
    fn reports_unwritable_path() {
        let err =
            write_keyfile(Path::new("/nonexistent-dir/provider.json"), &keyfile()).unwrap_err();
        assert!(matches!(err, KeyfileError::Io { .. }), "{err}");
    }
}
