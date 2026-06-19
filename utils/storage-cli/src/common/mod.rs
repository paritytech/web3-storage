// SPDX-License-Identifier: Apache-2.0

use crate::cli::GlobalArgs;
use anyhow::{bail, Context, Result};

/// Resolve the SURI from either `--suri` or `--keyfile` (exactly one is required)
pub fn resolve_suri(global: &GlobalArgs) -> Result<String> {
    match (&global.suri, &global.keyfile) {
        (Some(suri), _) => Ok(suri.clone()),
        (None, Some(path)) => {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read keyfile {}", path.display()))?;
            let suri = contents.trim().to_string();
            if suri.is_empty() {
                bail!("keyfile {} is empty", path.display());
            }
            Ok(suri)
        }
        (None, None) => bail!("one of --suri or --keyfile is required"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Build `GlobalArgs` with placeholder connection flags and the given
    /// identity inputs — only `suri`/`keyfile` matter for `resolve_suri`.
    fn make_args(suri: Option<String>, keyfile: Option<PathBuf>) -> GlobalArgs {
        GlobalArgs {
            chain_rpc: "ws://127.0.0.1:2222".to_string(),
            provider_url: "http://127.0.0.1:3333".to_string(),
            suri,
            keyfile,
        }
    }

    #[test]
    fn returns_inline_suri() {
        let args = make_args(Some("//Alice".to_string()), None);
        assert_eq!(resolve_suri(&args).unwrap(), "//Alice");
    }

    #[test]
    fn reads_and_trims_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "  //Bob").unwrap();
        let args = make_args(None, Some(file.path().to_path_buf()));
        assert_eq!(resolve_suri(&args).unwrap(), "//Bob");
    }

    #[test]
    fn rejects_empty_keyfile() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "   ").unwrap();
        let args = make_args(None, Some(file.path().to_path_buf()));
        let err = resolve_suri(&args).unwrap_err().to_string();
        assert!(err.contains("is empty"), "unexpected error: {err}");
    }

    #[test]
    fn errors_on_missing_keyfile() {
        let path = std::env::temp_dir().join("storage-cli-resolve-suri-does-not-exist");
        let args = make_args(None, Some(path));
        let err = resolve_suri(&args).unwrap_err().to_string();
        assert!(
            err.contains("failed to read keyfile"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn errors_when_neither_provided() {
        let args = make_args(None, None);
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
        let args = make_args(
            Some("//Inline".to_string()),
            Some(file.path().to_path_buf()),
        );
        assert_eq!(resolve_suri(&args).unwrap(), "//Inline");
    }
}
