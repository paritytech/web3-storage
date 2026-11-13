// SPDX-License-Identifier: GPL-3.0-only

//! End-to-end test of the `helper` subcommand against a running dev chain.
//!
//! It runs the built `storage-provider-node` binary the way an operator does:
//! generate a keyfile, fund the new account, register, update the settings,
//! and announce deregistration. Every step is checked twice: the command's
//! exit code and output, and the provider record the chain stores afterwards.
//!
//! The test needs a chain at `E2E_CHAIN_WS` (default `ws://127.0.0.1:2222`,
//! what `just start-e2e-chain` serves) whose `//Bob` account is funded, so
//! it is ignored by default. Run it with:
//!
//! ```text
//! cargo test -p storage-provider-node --test provider_helper_e2e -- --ignored --nocapture
//! ```
//!
//! `--nocapture` shows the provider the run used (keyfile path, address,
//! stake, multiaddr), for looking the account up on the chain afterwards.
//!
//! The chain's `MinProviderStake`, `MinStakePerByte` and `SS58Prefix` are read
//! from the runtime, so the test does not depend on one runtime's values.

use std::path::{Path, PathBuf};
use std::process::Command;

use storage_subxt::api;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ProviderInfo;
use storage_subxt::subxt::utils::{AccountId32, MultiAddress, H256};
use storage_subxt::subxt::{OnlineClient, PolkadotConfig};
use storage_subxt::subxt_signer::sr25519::dev;

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";

/// Funds sent to the generated account: the stake the test registers with,
/// plus the storage deposit and the fees of the three transactions that
/// land. Bob is funded at genesis far beyond this.
const FUNDING_FACTOR: u128 = 2;

fn chain_ws() -> String {
    std::env::var("E2E_CHAIN_WS").unwrap_or_else(|_| DEFAULT_CHAIN_WS.to_string())
}

/// The `helper` subcommand of the binary this test crate builds, bound to one
/// keyfile and the chain under test.
struct Helper {
    keyfile: PathBuf,
    chain_ws: String,
}

/// What a helper run printed, with the exit status.
struct Run {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Run {
    fn assert_ok(&self, step: &str) -> &Self {
        assert!(
            self.success,
            "{step} failed\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    fn assert_rejected(&self, step: &str, reason: &str) -> &Self {
        assert!(
            !self.success,
            "{step} must fail\nstdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        assert!(
            self.stderr.contains(reason),
            "{step}: stderr lacks {reason:?}\nstderr:\n{}",
            self.stderr
        );
        self
    }

    fn assert_prints(&self, step: &str, text: &str) -> &Self {
        assert!(
            self.stdout.contains(text),
            "{step}: stdout lacks {text:?}\nstdout:\n{}",
            self.stdout
        );
        self
    }

    /// The block the helper reported its transaction finalized in.
    fn finalized_in(&self) -> H256 {
        let hash = self
            .stdout
            .split_once("finalized in block ")
            .and_then(|(_, rest)| rest.split(')').next())
            .unwrap_or_else(|| panic!("no finalized block in stdout:\n{}", self.stdout));
        H256::from(array_bytes::hex2array::<_, 32>(hash).expect("block hash is 32 bytes"))
    }
}

impl Helper {
    fn run(&self, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_storage-provider-node"))
            .arg("helper")
            .arg("--chain-rpc")
            .arg(&self.chain_ws)
            .arg("--keyfile")
            .arg(&self.keyfile)
            .args(args)
            .output()
            .expect("the provider binary runs");
        Run {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Transfer `amount` from `//Bob` to `dest` and wait for finality. Bob, not
/// Alice: the CI job's provider node signs as Alice while this test runs,
/// and two signers of one account race for the nonce.
async fn fund(api: &OnlineClient<PolkadotConfig>, dest: AccountId32, amount: u128) {
    let tx = api::tx()
        .balances()
        .transfer_keep_alive(MultiAddress::Id(dest), amount);
    api.at_current_block()
        .await
        .expect("current block")
        .transactions()
        .sign_and_submit_then_watch_default(&tx, &dev::bob())
        .await
        .expect("transfer submitted")
        .wait_for_finalized_success()
        .await
        .expect("transfer finalized");
}

/// The provider record at the current finalized block, read once that block
/// is at or after `since`. The helper waits for its own transaction to be
/// finalized, but this client learns of new finalized blocks on its own
/// schedule, so a read right after the helper returns could still see the
/// block before the transaction.
async fn provider(
    api: &OnlineClient<PolkadotConfig>,
    since: Option<H256>,
    account: &AccountId32,
) -> Option<ProviderInfo> {
    let since_number = match since {
        Some(hash) => api
            .at_block(hash)
            .await
            .expect("reported block")
            .block_number(),
        None => 0,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let at = loop {
        let at = api.at_current_block().await.expect("current block");
        if at.block_number() >= since_number {
            break at;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "finalized block {since_number} not seen within 30s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    };
    let address = api::storage().storage_provider().providers().unvalidated();
    at.storage()
        .try_fetch(address, (*account,))
        .await
        .expect("provider read")
        .map(|value| value.decode().expect("provider decodes"))
}

fn keyfile_json(path: &Path) -> serde_json::Value {
    let contents = std::fs::read_to_string(path).expect("keyfile readable");
    serde_json::from_str(&contents).expect("keyfile is JSON")
}

#[tokio::test]
#[ignore = "needs a dev chain at E2E_CHAIN_WS (just start-e2e-chain)"]
async fn helper_manages_a_generated_provider_account() {
    let chain_ws = chain_ws();
    let api = OnlineClient::<PolkadotConfig>::from_url(&chain_ws)
        .await
        .expect("chain reachable");
    let at = api.at_current_block().await.expect("current block");
    let constants = at.constants();
    let ss58_prefix: u16 = constants
        .entry(api::constants().system().ss58_prefix())
        .expect("SS58Prefix");
    let min_stake: u128 = constants
        .entry(api::constants().storage_provider().min_provider_stake())
        .expect("MinProviderStake");
    let min_stake_per_byte: u128 = constants
        .entry(api::constants().storage_provider().min_stake_per_byte())
        .expect("MinStakePerByte");

    let dir = tempfile::tempdir().unwrap();
    let helper = Helper {
        keyfile: dir.path().join("provider.json"),
        chain_ws,
    };

    // generate-key: the keyfile is written once and carries the chain's prefix.
    let run = helper.run(&["generate-key"]);
    run.assert_ok("generate-key");
    let printed: serde_json::Value = serde_json::from_str(&run.stdout).expect("JSON on stdout");
    assert_eq!(printed["keyScheme"], "sr25519");
    assert_eq!(printed["ss58Prefix"], ss58_prefix);
    assert!(
        printed.get("secretPhrase").is_none(),
        "secret printed: {printed}"
    );
    let keyfile = keyfile_json(&helper.keyfile);
    assert_eq!(
        keyfile["secretPhrase"].as_str().unwrap().split(' ').count(),
        12
    );
    assert_eq!(keyfile["ss58Address"], printed["ss58Address"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&helper.keyfile)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
    }
    helper
        .run(&["generate-key"])
        .assert_rejected("second generate-key", "exists");
    assert_eq!(
        keyfile_json(&helper.keyfile),
        keyfile,
        "keyfile overwritten"
    );
    helper
        .run(&["--key-scheme", "ecdsa", "generate-key"])
        .assert_rejected("ecdsa generate-key", "only sr25519");

    let ss58_address = printed["ss58Address"].as_str().unwrap().to_string();
    let account_hex = printed["accountId"].as_str().unwrap();
    let account = AccountId32(array_bytes::hex2array(account_hex).expect("accountId is 32 bytes"));
    let public_key: Vec<u8> =
        array_bytes::hex2bytes(printed["publicKey"].as_str().unwrap()).unwrap();
    assert!(
        provider(&api, None, &account).await.is_none(),
        "fresh account is registered"
    );

    // register: the record the chain stores is what the helper submitted.
    let stake = min_stake * 2;
    let multiaddr = "/ip4/127.0.0.1/tcp/3399";
    println!(
        "provider under test:\n  keyfile: {}\n  ss58Address: {ss58_address}\n  accountId: {account_hex}\n  stake: {stake}\n  multiaddr: {multiaddr}\n  funded from //Bob: {}",
        helper.keyfile.display(),
        stake * FUNDING_FACTOR
    );
    fund(&api, account, stake * FUNDING_FACTOR).await;
    let run = helper.run(&[
        "register",
        "--multiaddr",
        multiaddr,
        "--stake",
        &stake.to_string(),
    ]);
    run.assert_ok("register").assert_prints(
        "register",
        &format!("Registered provider {ss58_address} at {multiaddr} with stake {stake}"),
    );
    let record = provider(&api, Some(run.finalized_in()), &account)
        .await
        .expect("registered");
    assert_eq!(record.multiaddr.0, multiaddr.as_bytes());
    assert_eq!(record.public_key.0, public_key);
    assert_eq!(record.stake, stake);
    assert_eq!(record.committed_bytes, 0);
    assert_eq!(record.deregister_at, None);

    helper
        .run(&[
            "register",
            "--multiaddr",
            multiaddr,
            "--stake",
            &stake.to_string(),
        ])
        .assert_rejected("second register", "already registered");

    // update-settings: the merged settings land on chain; a repeat is a no-op;
    // what the pallet rejects is rejected before submission.
    let max_capacity = stake / min_stake_per_byte;
    let capacity_arg = max_capacity.to_string();
    let update = [
        "update-settings",
        "--min-duration",
        "10",
        "--max-duration",
        "100",
        "--price-per-byte",
        "7",
        "--max-capacity",
        &capacity_arg,
        "--no-replicas",
    ];
    let run = helper.run(&update);
    run.assert_ok("update-settings")
        .assert_prints(
            "update-settings",
            &format!("Updated settings of provider {ss58_address}"),
        )
        .assert_prints("update-settings", "replica_sync_price: none");
    let updated_in = run.finalized_in();
    let settings = provider(&api, Some(updated_in), &account)
        .await
        .expect("registered")
        .settings;
    assert_eq!(settings.min_duration, 10);
    assert_eq!(settings.max_duration, 100);
    assert_eq!(settings.price_per_byte, 7);
    assert_eq!(settings.replica_sync_price, None);
    assert_eq!(settings.max_capacity, max_capacity as u64);

    helper
        .run(&update)
        .assert_ok("repeated update-settings")
        .assert_prints(
            "repeated update-settings",
            "already match; nothing submitted",
        );

    let over_capacity = (max_capacity + 1).to_string();
    helper
        .run(&["update-settings", "--max-capacity", &over_capacity])
        .assert_rejected("over-capacity update-settings", "cannot back max_capacity");
    helper
        .run(&["update-settings", "--min-duration", "200"])
        .assert_rejected("inverted-duration update-settings", "exceeds max_duration");
    assert_eq!(
        provider(&api, Some(updated_in), &account)
            .await
            .expect("registered")
            .settings,
        settings,
        "a rejected update changed the chain"
    );

    // deregister: announced once; afterwards neither settings nor a second
    // announcement are accepted.
    let run = helper.run(&["deregister"]);
    run.assert_ok("deregister").assert_prints(
        "deregister",
        &format!("Announced deregistration of provider {ss58_address}"),
    );
    let record = provider(&api, Some(run.finalized_in()), &account)
        .await
        .expect("still registered");
    assert!(record.deregister_at.is_some(), "deregister_at unset");
    assert!(!record.settings.accepting_primary);

    helper
        .run(&["deregister"])
        .assert_rejected("second deregister", "has announced deregistration");
    helper
        .run(&["update-settings", "--price-per-byte", "8"])
        .assert_rejected(
            "update-settings after deregister",
            "has announced deregistration",
        );
}

/// Without a chain, `generate-key` still writes the keyfile and says which
/// prefix it fell back to; the chain operations fail before anything is
/// written.
#[tokio::test]
async fn helper_without_a_chain() {
    let dir = tempfile::tempdir().unwrap();
    let helper = Helper {
        keyfile: dir.path().join("provider.json"),
        chain_ws: "ws://127.0.0.1:1".to_string(),
    };
    let run = helper.run(&["generate-key"]);
    run.assert_ok("offline generate-key");
    assert!(run.stderr.contains("prefix 42"), "stderr:\n{}", run.stderr);
    assert_eq!(keyfile_json(&helper.keyfile)["ss58Prefix"], 42);

    helper
        .run(&[
            "register",
            "--multiaddr",
            "/ip4/127.0.0.1/tcp/3399",
            "--stake",
            "1",
        ])
        .assert_rejected("offline register", "Error:");
}
