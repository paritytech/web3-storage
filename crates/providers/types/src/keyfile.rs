// SPDX-License-Identifier: Apache-2.0

//! Keyfile format and reading: a BIP39 mnemonic and the identity it
//! derives, stored as JSON in a file only its owner can read.
//!
//! `storage-cli provider generate-key` writes the file; the provider node
//! and `storage-cli` read it with [`read_keyfile`].

use std::collections::BTreeMap;
use std::path::Path;

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

/// The secret read from a keyfile by [`read_keyfile`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyfileSecret {
    /// The seed phrase or derivation path.
    pub seed: String,
    /// The `keyScheme` a JSON keyfile records, if any. `None` for a plain
    /// seed file and for a JSON keyfile without the field.
    pub key_scheme: Option<String>,
}

/// Why a keyfile could not be read.
#[derive(Debug, thiserror::Error)]
pub enum KeyfileError {
    /// The contents are not a JSON keyfile.
    #[error("not valid JSON: {0}")]
    Json(String),

    /// Opening or reading the file failed.
    #[error("cannot read keyfile {path}: {source}")]
    Read {
        /// The file.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// The file is group- or world-readable.
    #[error("keyfile {path} has insecure permissions {mode:o}; run: chmod 600 {path}")]
    InsecurePermissions {
        /// The file.
        path: String,
        /// Its permission bits.
        mode: u32,
    },

    /// The file contains only whitespace.
    #[error("keyfile {0} is empty")]
    Empty(String),

    /// The JSON keyfile is malformed, or its `secretPhrase` is empty.
    #[error("keyfile {path}: {reason}")]
    Invalid {
        /// The file.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
}

/// Read the secret from `path`: the JSON keyfile `storage-cli provider generate-key` writes
/// when the contents start with `{`, else a plain seed phrase or derivation
/// path. The contents are trimmed. On Unix the file must not be group- or
/// world-readable; the check runs on the open handle, so the file checked is
/// the file read.
pub fn read_keyfile(path: &Path) -> Result<KeyfileSecret, KeyfileError> {
    use std::io::Read;

    let display = || path.display().to_string();
    let read = |source| KeyfileError::Read {
        path: display(),
        source,
    };

    let file = std::fs::File::open(path).map_err(read)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file.metadata().map_err(read)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(KeyfileError::InsecurePermissions {
                path: display(),
                mode: mode & 0o777,
            });
        }
    }
    let mut contents = String::new();
    std::io::BufReader::new(file)
        .read_to_string(&mut contents)
        .map_err(read)?;

    let contents = contents.trim();
    if contents.is_empty() {
        return Err(KeyfileError::Empty(display()));
    }
    if !contents.starts_with('{') {
        return Ok(KeyfileSecret {
            seed: contents.to_string(),
            key_scheme: None,
        });
    }
    let invalid = |reason: String| KeyfileError::Invalid {
        path: display(),
        reason,
    };
    let keyfile = Keyfile::from_json(contents).map_err(|e| invalid(e.to_string()))?;
    if keyfile.secret_phrase.trim().is_empty() {
        return Err(invalid("empty secretPhrase".to_string()));
    }
    Ok(KeyfileSecret {
        seed: keyfile.secret_phrase,
        key_scheme: keyfile.key_scheme,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyfile() -> Keyfile {
        Keyfile {
            secret_phrase: "bottom drive obey lake curtain smoke basket hold race lonely fit walk"
                .to_string(),
            key_scheme: Some("sr25519".to_string()),
            identity: BTreeMap::from([
                ("ss58Address".to_string(), "5abc".into()),
                ("ss58Prefix".to_string(), 42.into()),
            ]),
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

    /// A JSON keyfile reads back as its secret and scheme; a plain seed
    /// file reads as the seed with no scheme.
    #[test]
    fn reads_json_and_plain_keyfiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");
        let keyfile = keyfile();
        write_secret(&path, &keyfile.to_json());
        assert_eq!(
            read_keyfile(&path).unwrap(),
            KeyfileSecret {
                seed: keyfile.secret_phrase,
                key_scheme: Some("sr25519".to_string()),
            }
        );

        let plain = dir.path().join("seed");
        write_secret(&plain, "  //Alice\n");
        assert_eq!(
            read_keyfile(&plain).unwrap(),
            KeyfileSecret {
                seed: "//Alice".to_string(),
                key_scheme: None,
            }
        );

        // subkey's output has no keyScheme.
        write_secret(
            &plain,
            r#"{"secretPhrase": "//Dave", "networkId": "substrate"}"#,
        );
        assert_eq!(read_keyfile(&plain).unwrap().key_scheme, None);
    }

    #[test]
    fn rejects_unreadable_empty_and_malformed_keyfiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seed");

        let err = read_keyfile(&path).unwrap_err();
        assert!(matches!(err, KeyfileError::Read { .. }), "{err}");

        write_secret(&path, "  \n");
        let err = read_keyfile(&path).unwrap_err();
        assert!(matches!(err, KeyfileError::Empty(_)), "{err}");

        write_secret(&path, r#"{"secretPhrase": "  "}"#);
        let err = read_keyfile(&path).unwrap_err();
        assert!(err.to_string().contains("empty secretPhrase"), "{err}");

        write_secret(&path, r#"{"keyScheme": "sr25519"}"#);
        let err = read_keyfile(&path).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_group_or_world_readable_keyfile() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seed");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_keyfile(&path).unwrap_err();
        assert!(
            matches!(err, KeyfileError::InsecurePermissions { mode: 0o644, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("chmod 600"), "{err}");
    }

    /// Write `contents` with the permissions `read_keyfile` requires.
    fn write_secret(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
