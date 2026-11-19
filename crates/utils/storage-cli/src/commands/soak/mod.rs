// SPDX-License-Identifier: Apache-2.0

//! `soak` subcommands: long-running correctness tests of the provider's
//! background services. They attach to a running chain and provider.

pub mod gc;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};

use crate::common::{account_ss58, resolve_suri, signer_from_suri};

#[derive(Debug, Subcommand)]
pub enum Soak {
    /// Garbage collector: prune cycles and bucket teardowns until the provider
    /// is empty again, repeated until the deadline.
    Gc(gc::GcArgs),
}

/// Client accounts, one lane per value. Each lane opens and pays for its own
/// buckets, so parallel lanes never share a transaction nonce.
#[derive(Debug, Args)]
#[group(required = true, multiple = true)]
pub struct LaneKeys {
    /// Secret URI (SURI) of a client account, e.g. "//Bob". Repeat for more lanes.
    #[arg(long = "client-suri", value_name = "SURI")]
    pub client_suris: Vec<String>,

    /// File whose contents are a client account's SURI. Repeat for more lanes.
    #[arg(long = "client-keyfile", value_name = "FILE")]
    pub client_keyfiles: Vec<PathBuf>,
}

impl LaneKeys {
    /// One SURI per lane: the inline SURIs, then the keyfile contents. Rejects
    /// two lanes that derive the same account.
    pub fn suris(&self) -> Result<Vec<String>> {
        let mut suris = self.client_suris.clone();
        for path in &self.client_keyfiles {
            suris.push(resolve_suri(None, Some(path))?);
        }
        let mut seen = HashMap::new();
        for (lane, suri) in suris.iter().enumerate() {
            let account = account_ss58(&signer_from_suri(suri)?);
            if let Some(first) = seen.insert(account.clone(), lane) {
                bail!("lanes {first} and {lane} use the same account {account}");
            }
        }
        Ok(suris)
    }
}
