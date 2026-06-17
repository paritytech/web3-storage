// SPDX-License-Identifier: GPL-3.0-only

import { describe, expect, it } from "vitest";
import { Alice, devSigners, makeSigner, seedToKeypair } from "./signers.js";

describe("seedToKeypair", () => {
  it("derives dev accounts identically via //Name and the dev signers", () => {
    expect(seedToKeypair("//Alice").publicKey).toEqual(Alice.publicKey);
    expect(seedToKeypair("//Ferdie").publicKey).toEqual(devSigners.Ferdie.publicKey);
  });

  it("treats `mnemonic//path` and bare `//path` over DEV_PHRASE the same", () => {
    const viaDefault = seedToKeypair("//Alice").publicKey;
    const viaExplicit = seedToKeypair(
      "bottom drive obey lake curtain smoke basket hold race lonely fit walk//Alice",
    ).publicKey;
    expect(viaExplicit).toEqual(viaDefault);
  });
});

describe("makeSigner", () => {
  it("exposes address, publicKey, seed, and the raw keypair", () => {
    const s = makeSigner("//Bob");
    expect(s.seed).toBe("//Bob");
    expect(s.publicKey).toEqual(devSigners.Bob.publicKey);
    expect(s.address).toBe(devSigners.Bob.address);
    expect(s.keypair).toBeDefined();
    // The keypair signs raw bytes — required for provider auth headers.
    const sig = s.keypair!.sign(new Uint8Array([1, 2, 3]));
    expect(sig.length).toBe(64);
  });
});
