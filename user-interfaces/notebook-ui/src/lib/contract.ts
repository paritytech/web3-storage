// SPDX-License-Identifier: GPL-3.0-only

/** Loads the MutableNotebook ABI + bytecode from the resolc build output. */

import combined from "../../../../examples/contracts/build/combined.json";
import { CONTRACT_KEY, hexToBytes } from "./notebook";
import type { Abi } from "viem";

interface ContractArtifact {
  abi: Abi;
  bin: string;
}

interface CombinedJson {
  contracts: Record<string, ContractArtifact>;
}

const entry = (combined as unknown as CombinedJson).contracts[CONTRACT_KEY];
if (!entry) {
  throw new Error(
    `combined.json missing ${CONTRACT_KEY} — run \`bash examples/contracts/build.sh\``,
  );
}

export const NOTEBOOK_ABI = entry.abi;
export const NOTEBOOK_BYTECODE = hexToBytes(entry.bin);
