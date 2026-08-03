// SPDX-License-Identifier: Apache-2.0

/**
 * Tune the storage-backed protocol parameters of a running chain via
 * `sudo(System.set_storage)`, no runtime upgrade needed.
 *
 * Both runtimes declare the pallet-storage-provider parameters as
 * `parameter_types! { pub storage X: T = default; }` (see
 * `runtimes/<runtime>/src/storage.rs`): each lives at the well-known unhashed key
 * `twox128(":X:")` and falls back to the compiled default while the key is
 * absent. Setting the key changes the effective value from the next block;
 * deleting it restores the default. All durations are RELAY chain blocks (6s
 * each) — the pallet measures time on the relay-chain clock. Built for
 * compressing protocol timing in dev and stress-test runs
 * (paritytech/web3-storage#267).
 *
 * Usage:
 *   pnpm --filter web3-storage-papi-demo run lower-timeouts -- [options]
 *
 *   --chain-rpc <url>   chain WS endpoint            (default ws://127.0.0.1:2222)
 *   --suri <suri>       sudo account                 (default //Alice)
 *   --profile fast      minute-scale timing preset   (see FAST_PROFILE)
 *   --set Name=Value    set one parameter, repeatable (see PARAMS for names;
 *                       durations are RELAY chain blocks, amounts plancks,
 *                       plain integers)
 *   --restore           delete all overrides → compiled defaults
 *   --list              print current overrides + effective values, then exit
 *   --dry-run           print what would be submitted without submitting
 */

import { parseArgs } from "node:util";

// PAPI itself has no public twox128; the hasher and integer codecs are the
// only things taken from substrate-bindings, per the repo JS/TS guidelines.
import { Twox128, u16, u32, u128 } from "@polkadot-api/substrate-bindings";
import {
  connect,
  disconnect,
  makeSigner,
  sameAddress,
  submitTxFinalized,
  syncSs58Prefix,
  toHex,
  waitForChainReady,
  READ_OPTS,
  type ChainConnection,
  type ParachainApi,
} from "@web3-storage/sdk";

// ── Parameter table ──────────────────────────────────────────────────────────

/** Every `pub storage` parameter in `runtimes/<runtime>/src/storage.rs`, with its SCALE type. */
const PARAMS = {
  // BlockNumber (u32) in RELAY chain blocks (6s each, so 10 blocks = 1
  // minute): the pallet measures every duration on the relay-chain clock
  // (`Config::BlockNumberProvider`), not parachain blocks.
  ChallengeTimeout: "u32",
  MaxNonceAge: "u32",
  SettlementTimeout: "u32",
  RequestTimeout: "u32",
  DeregisterAnnouncementPeriod: "u32",
  // Balance (u128), plancks
  MinProviderStake: "u128",
  ChallengeDeposit: "u128",
  MinStakePerByte: "u128",
  MaxChallengesPerDeadline: "u16",
} as const;

type ParamName = keyof typeof PARAMS;
type ParamType = (typeof PARAMS)[ParamName];

const PARAM_NAMES = Object.keys(PARAMS) as ParamName[];

/**
 * Minute-scale timing for stress/dev runs, in relay blocks. Only timing
 * parameters; economic ones (stakes, deposits) keep their defaults unless
 * --set overrides them.
 *
 * Deliberately absent:
 * - RequestTimeout: the provider node signs /negotiate quotes with
 *   `valid_until = anchor + RequestTimeout` read from chain metadata at
 *   connect time, which does not see a live override. Shortening the
 *   on-chain value makes the pallet reject every later quote with
 *   TermsValidityTooLong; leave it at its default.
 * - DeregisterAnnouncementPeriod: must stay > RequestTimeout (re-checked
 *   below), so with RequestTimeout at its 6h default it cannot be
 *   minute-scale.
 */
const FAST_PROFILE: Partial<Record<ParamName, bigint>> = {
  ChallengeTimeout: 100n, // 10 min
  SettlementTimeout: 100n, // 10 min
  // Roomy on purpose: the sign → build → broadcast → finalize choreography
  // must fit inside it or every commitment gets rejected as stale.
  MaxNonceAge: 600n, // 1 h
};

const PROFILES: Record<string, Partial<Record<ParamName, bigint>>> = {
  fast: FAST_PROFILE,
};

const MAX_VALUE: Record<ParamType, bigint> = {
  u16: 0xffffn,
  u32: 0xffff_ffffn,
  u128: (1n << 128n) - 1n,
};

// ── SCALE + key helpers ──────────────────────────────────────────────────────

/** `parameter_types!(pub storage X …)` stores under `twox128(":X:")`. */
function paramKey(name: ParamName): Uint8Array {
  return Twox128(new TextEncoder().encode(`:${name}:`));
}

function encodeValue(name: ParamName, value: bigint): Uint8Array {
  const ty = PARAMS[name];
  if (value < 0n || value > MAX_VALUE[ty]) {
    throw new Error(`${name}: ${value} out of range for ${ty}`);
  }
  if (ty === "u128") return u128.enc(value);
  return ty === "u32" ? u32.enc(Number(value)) : u16.enc(Number(value));
}

function decodeValue(name: ParamName, hex: string): bigint {
  const ty = PARAMS[name];
  if (ty === "u128") return u128.dec(hex);
  return BigInt(ty === "u32" ? u32.dec(hex) : u16.dec(hex));
}

/** Raw read of a parameter's override key; undefined = compiled default in effect. */
async function readOverride(
  { papi }: ChainConnection,
  name: ParamName,
): Promise<bigint | undefined> {
  const raw = await papi._request<string | null>("state_getStorage", [
    toHex(paramKey(name)),
  ]);
  return raw == null ? undefined : decodeValue(name, raw);
}

/** Effective value as the pallet sees it (compiled default or override). */
async function readEffective(
  { api }: ChainConnection,
  name: ParamName,
): Promise<bigint> {
  const constants = api.constants.StorageProvider as unknown as Record<
    ParamName,
    () => Promise<number | bigint>
  >;
  return BigInt(await constants[name]());
}

// ── Safety checks ────────────────────────────────────────────────────────────

/**
 * The runtime's `integrity_test` only runs at build time, so a bad override
 * would silently break the deregistration security assumptions. Re-check the
 * documented invariants here against the post-change effective values.
 */
function checkInvariants(effective: Map<ParamName, bigint>): void {
  const dereg = effective.get("DeregisterAnnouncementPeriod")!;
  const challenge = effective.get("ChallengeTimeout")!;
  const request = effective.get("RequestTimeout")!;
  if (dereg <= challenge) {
    throw new Error(
      `DeregisterAnnouncementPeriod (${dereg}) must be > ChallengeTimeout (${challenge})`,
    );
  }
  if (dereg <= request) {
    throw new Error(
      `DeregisterAnnouncementPeriod (${dereg}) must be > RequestTimeout (${request})`,
    );
  }
}

// ── CLI ──────────────────────────────────────────────────────────────────────

function parseCli() {
  const { values } = parseArgs({
    options: {
      "chain-rpc": { type: "string", default: "ws://127.0.0.1:2222" },
      suri: { type: "string", default: "//Alice" },
      profile: { type: "string" },
      set: { type: "string", multiple: true },
      restore: { type: "boolean", default: false },
      list: { type: "boolean", default: false },
      "dry-run": { type: "boolean", default: false },
      help: { type: "boolean", default: false },
    },
  });

  if (values.help) {
    console.log(USAGE);
    process.exit(0);
  }

  const changes = new Map<ParamName, bigint>();
  if (values.profile) {
    const profile = PROFILES[values.profile];
    if (!profile) {
      throw new Error(
        `unknown profile "${values.profile}" (available: ${Object.keys(PROFILES).join(", ")})`,
      );
    }
    for (const [name, value] of Object.entries(profile)) {
      changes.set(name as ParamName, value);
    }
  }
  for (const pair of values.set ?? []) {
    const eq = pair.indexOf("=");
    const name = pair.slice(0, eq) as ParamName;
    if (eq === -1 || !(name in PARAMS)) {
      throw new Error(
        `--set expects Name=Value with Name one of: ${PARAM_NAMES.join(", ")}`,
      );
    }
    const value = BigInt(pair.slice(eq + 1));
    if (value < 0n || value > MAX_VALUE[PARAMS[name]]) {
      throw new Error(`${name}: ${value} out of range for ${PARAMS[name]}`);
    }
    changes.set(name, value);
  }

  if (values.restore && changes.size > 0) {
    throw new Error("--restore cannot be combined with --profile/--set");
  }
  if (!values.restore && !values.list && changes.size === 0) {
    throw new Error(
      "nothing to do: pass --profile/--set, --restore, or --list (--help for usage)",
    );
  }

  return {
    chainRpc: values["chain-rpc"],
    suri: values.suri,
    changes,
    restore: values.restore,
    list: values.list,
    dryRun: values["dry-run"],
  };
}

const USAGE = `lower-timeouts: tune storage-backed protocol parameters via sudo setStorage

  --chain-rpc <url>   chain WS endpoint            (default ws://127.0.0.1:2222)
  --suri <suri>       sudo account                 (default //Alice)
  --profile fast      minute-scale timing preset
  --set Name=Value    set one parameter (repeatable)
  --restore           delete all overrides → compiled defaults
  --list              print current overrides + effective values
  --dry-run           print without submitting

Parameters: ${PARAM_NAMES.join(", ")}`;

// ── Main ─────────────────────────────────────────────────────────────────────

async function main() {
  const opts = parseCli();
  const conn = connect(opts.chainRpc);
  const { api } = conn;
  try {
    // PAPI's WS provider retries forever, so an unreachable endpoint would
    // hang waitForChainReady indefinitely; race it against a hard deadline.
    await Promise.race([
      waitForChainReady(api),
      new Promise((_, reject) => {
        setTimeout(
          () => reject(new Error(`no chain reachable at ${opts.chainRpc}`)),
          30_000,
        ).unref();
      }),
    ]);
    await syncSs58Prefix(api);

    if (opts.list) {
      for (const name of PARAM_NAMES) {
        const [effective, override] = await Promise.all([
          readEffective(conn, name),
          readOverride(conn, name),
        ]);
        const source = override === undefined ? "compiled default" : "override";
        console.log(`  ${name} = ${effective} (${source})`);
      }
      return;
    }

    if (opts.restore) {
      const keys = PARAM_NAMES.map((n) => paramKey(n));
      if (opts.dryRun) {
        console.log("would submit sudo(System.kill_storage) for keys:");
        for (const [i, n] of PARAM_NAMES.entries()) {
          console.log(`  ${n}: ${toHex(keys[i])}`);
        }
        return;
      }
      await submitSudo(
        conn,
        opts.suri,
        api.tx.System.kill_storage({ keys }).decodedCall,
      );
      console.log("✅ all overrides cleared, compiled defaults in effect");
      return;
    }

    // Post-change effective values: proposed change, else current raw
    // override (real-time), else the metadata constant. The constant is
    // evaluated at the block PAPI pinned on connect, so a value changed in
    // the last few blocks can read stale; a --profile run is immune (it
    // proposes all three invariant parameters together).
    const effective = new Map<ParamName, bigint>();
    for (const name of [
      "DeregisterAnnouncementPeriod",
      "ChallengeTimeout",
      "RequestTimeout",
    ] as const) {
      effective.set(
        name,
        opts.changes.get(name) ??
          (await readOverride(conn, name)) ??
          (await readEffective(conn, name)),
      );
    }
    checkInvariants(effective);

    const items: Array<[Uint8Array, Uint8Array]> = [];
    for (const [name, value] of opts.changes) {
      const before = await readEffective(conn, name);
      console.log(`  ${name}: ${before} → ${value}`);
      items.push([paramKey(name), encodeValue(name, value)]);
    }
    if (opts.dryRun) {
      console.log("dry run, would submit sudo(System.set_storage) with:");
      for (const [k, v] of items) console.log(`  ${toHex(k)} = ${toHex(v)}`);
      return;
    }

    await submitSudo(
      conn,
      opts.suri,
      api.tx.System.set_storage({ items }).decodedCall,
    );

    // Read the raw keys back: proves the overrides landed (the effective
    // values via metadata constants would come from this client's cached
    // runtime and can be stale within this process).
    for (const [name, value] of opts.changes) {
      const now = await readOverride(conn, name);
      if (now !== value) {
        throw new Error(`${name}: override readback ${now} != ${value}`);
      }
    }
    console.log(`✅ ${opts.changes.size} parameter(s) overridden (verify with --list)`);
  } finally {
    disconnect(conn);
  }
}

type SudoInnerCall = Parameters<ParachainApi["tx"]["Sudo"]["sudo"]>[0]["call"];

async function submitSudo(
  { papi, api }: ChainConnection,
  suri: string,
  call: SudoInnerCall,
): Promise<void> {
  const sudo = makeSigner(suri);
  const sudoKey = await api.query.Sudo.Key.getValue(READ_OPTS);
  if (!sudoKey || !sameAddress(sudoKey, sudo.address)) {
    throw new Error(
      `${sudo.address} (--suri) is not the sudo key (${sudoKey ?? "unset"})`,
    );
  }
  const tx = api.tx.Sudo.sudo({ call });
  const result = await submitTxFinalized(tx, sudo.signer, {
    label: "sudo(setStorage)",
    client: papi,
  });
  // submitTx only asserts the sudo extrinsic dispatched; the inner call's
  // outcome is in the Sudid event.
  const sudid = (result.events as Array<{ type: string; value: any }>).find(
    (e) => e.type === "Sudo" && e.value?.type === "Sudid",
  );
  if (!sudid || sudid.value.value?.sudo_result?.success !== true) {
    throw new Error(
      `inner call failed: ${JSON.stringify(sudid?.value?.value ?? "no Sudid event")}`,
    );
  }
}

main().then(
  // PAPI's teardown can leave the event loop alive after destroy(); exit
  // explicitly like the e2e runners do.
  () => process.exit(0),
  (err) => {
    console.error(`❌ ${err.message ?? err}`);
    process.exit(1);
  },
);
