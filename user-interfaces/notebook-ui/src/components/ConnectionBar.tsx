// SPDX-License-Identifier: GPL-3.0-only

import type { ChangeEvent, FormEvent } from "react";

const DEV_SEEDS = ["//Alice", "//Bob", "//Charlie", "//Dave", "//Eve", "//Ferdie"];

interface Props {
  chainWs: string;
  providerUrl: string;
  signerSeed: string;
  providerSeed: string;
  contractAddress: string;
  s3BucketId: string;
  status: string;
  busy: boolean;
  connected: boolean;
  onChainWsChange: (v: string) => void;
  onProviderUrlChange: (v: string) => void;
  onSignerSeedChange: (v: string) => void;
  onProviderSeedChange: (v: string) => void;
  onContractAddressChange: (v: string) => void;
  onS3BucketIdChange: (v: string) => void;
  onDeploy: () => void;
  onAttach: () => void;
  onDisconnect: () => void;
}

export function ConnectionBar(props: Props) {
  const handleDeploy = (e: FormEvent) => {
    e.preventDefault();
    props.onDeploy();
  };
  const handleAttach = (e: FormEvent) => {
    e.preventDefault();
    props.onAttach();
  };

  return (
    <div className="border-b border-border bg-card p-4">
      <div className="flex flex-wrap items-end gap-3">
        <Field label="Chain WS" value={props.chainWs} onChange={props.onChainWsChange} width="w-56" />
        <Field label="Provider URL" value={props.providerUrl} onChange={props.onProviderUrlChange} width="w-56" />
        <SeedField
          label="Signer (you)"
          value={props.signerSeed}
          onChange={props.onSignerSeedChange}
        />
        <SeedField
          label="Provider seed"
          value={props.providerSeed}
          onChange={props.onProviderSeedChange}
        />
        {!props.connected ? (
          <>
            <button
              type="submit"
              onClick={handleDeploy}
              disabled={props.busy}
              className="rounded bg-primary px-4 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
            >
              Deploy notebook
            </button>
            <span className="text-muted-foreground">or</span>
            <Field
              label="Contract address"
              value={props.contractAddress}
              onChange={props.onContractAddressChange}
              width="w-72"
            />
            <Field
              label="Bucket ID"
              value={props.s3BucketId}
              onChange={props.onS3BucketIdChange}
              width="w-24"
            />
            <button
              type="submit"
              onClick={handleAttach}
              disabled={props.busy || !props.contractAddress || !props.s3BucketId}
              className="rounded border border-border bg-card px-4 py-2 text-sm font-medium disabled:opacity-50"
            >
              Attach
            </button>
          </>
        ) : (
          <div className="flex items-center gap-2">
            <span className="rounded bg-muted px-2 py-1 font-mono text-xs">
              {props.contractAddress.slice(0, 8)}… · bucket {props.s3BucketId}
            </span>
            <button
              type="button"
              onClick={props.onDisconnect}
              className="rounded border border-border px-3 py-2 text-sm"
            >
              Disconnect
            </button>
          </div>
        )}
      </div>
      {props.status && (
        <div className="mt-2 text-xs text-muted-foreground">{props.status}</div>
      )}
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  width = "w-40",
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  width?: string;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      <input
        className={`${width} rounded border border-border bg-background px-2 py-1 text-sm font-mono`}
        value={value}
        onChange={(e: ChangeEvent<HTMLInputElement>) => onChange(e.target.value)}
      />
    </label>
  );
}

function SeedField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      <select
        className="w-28 rounded border border-border bg-background px-2 py-1 text-sm font-mono"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {DEV_SEEDS.map((s) => (
          <option key={s} value={s}>
            {s}
          </option>
        ))}
      </select>
    </label>
  );
}
