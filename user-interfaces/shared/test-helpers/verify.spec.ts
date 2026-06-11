import { test, expect } from "@playwright/test";
import { Ferdie, cleanProviderRegistry, registerProviderViaApi } from "./src/index";
import { getApi } from "./src/chain-api";

test("forceRemoveProvider wipes via sudo kill_storage", async () => {
  test.setTimeout(180_000);
  const api = getApi();
  await cleanProviderRegistry([]);
  expect(await api.query.StorageProvider.Providers.getValue(Ferdie.address)).toBeUndefined();
  await registerProviderViaApi(Ferdie);
  expect(await api.query.StorageProvider.Providers.getValue(Ferdie.address)).toBeDefined();
  await cleanProviderRegistry([]);
  expect(await api.query.StorageProvider.Providers.getValue(Ferdie.address)).toBeUndefined();
});
