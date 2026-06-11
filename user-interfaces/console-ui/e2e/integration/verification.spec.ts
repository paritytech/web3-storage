/**
 * Download verification spec.
 *
 * The on-chain CID for a single-chunk object equals blake2b-256 of its bytes,
 * so the spec anchors metadata for a UI-uploaded object from the test side and
 * then exercises both verification outcomes: the honest provider path (toast
 * confirms "verified") and a tampering provider simulated via route
 * interception (download hard-fails with a CID-mismatch error).
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupBuckets,
  createBucketViaApi,
  getApi,
  submitExtrinsicBestBlock,
} from "@web3-storage/test-helpers";
import { computeCid } from "@web3-storage/sdk";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

const CONTENT = `verify-me-${Date.now()}`;
const KEY = "verified.txt";
let bucketName: string;
let s3BucketId: bigint;

test.beforeAll(async () => {
  test.setTimeout(120_000);
  bucketName = `verify-${Date.now()}`;
  const handle = await createBucketViaApi(Alice, { name: bucketName });
  s3BucketId = handle.s3BucketId;
});

test.afterAll(async () => {
  test.setTimeout(90_000);
  await cleanupBuckets(Alice);
});

test.beforeEach(async ({ localPage }) => {
  await localPage.getByTestId("nav-storage").click();
  await localPage.getByTestId("s3-bucket-selector").selectOption({ label: bucketName });
});

test("verified download shows the verified toast", async ({ localPage }) => {
  // Upload through the UI (provider computes the same single-chunk data_root
  // we compute locally), then anchor that CID on chain so getObject verifies.
  await localPage.getByTestId("s3-upload-button").click();
  await localPage.getByTestId("s3-upload-file-input").setInputFiles({
    name: KEY,
    mimeType: "text/plain",
    buffer: Buffer.from(CONTENT),
  });
  await localPage.getByTestId("s3-upload-submit").click();
  await expect(localPage.getByTestId(`s3-object-row-${KEY}`)).toBeVisible({
    timeout: 60_000,
  });

  const bytes = new TextEncoder().encode(CONTENT);
  const api = getApi();
  await submitExtrinsicBestBlock(
    api.tx.S3Registry.put_object_metadata({
      s3_bucket_id: s3BucketId,
      key: new TextEncoder().encode(KEY),
      cid: `0x${Buffer.from(computeCid(bytes)).toString("hex")}`,
      size: BigInt(bytes.length),
      content_type: new TextEncoder().encode("text/plain"),
      user_metadata: [],
    }),
    Alice.signer,
  );

  const downloadPromise = localPage.waitForEvent("download");
  await localPage.getByTestId(`s3-download-${KEY}`).click();
  await downloadPromise;
  await expect(localPage.getByText("Downloaded (verified)").first()).toBeVisible({ timeout: 30_000 });
});

test("tampered download hard-fails with a CID mismatch", async ({ localPage }) => {
  // Simulate a malicious/corrupted provider: same object route, wrong bytes.
  await localPage.route(`**/s3/*/object?key=${KEY}`, async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({
      status: 200,
      contentType: "text/plain",
      body: "tampered bytes from a lying provider",
    });
  });

  await localPage.getByTestId(`s3-download-${KEY}`).click();
  await expect(localPage.getByText("Download failed").first()).toBeVisible({ timeout: 30_000 });
  await expect(localPage.getByText(/does not match its CID/).first()).toBeVisible();
});
