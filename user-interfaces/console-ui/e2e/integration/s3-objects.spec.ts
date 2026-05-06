/**
 * S3 object operations spec.
 *
 * One bucket created via api in beforeAll (faster than walking the UI flow);
 * tests upload / download bytes / delete reuse it.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupBuckets,
  createBucketViaApi,
  registerProviderViaApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

let bucketName: string;

test.beforeAll(async () => {
  test.setTimeout(120_000);
  await registerProviderViaApi(Alice);
  bucketName = `objects-${Date.now()}`;
  await createBucketViaApi(Alice, { name: bucketName });
});

test.afterAll(async () => {
  await cleanupBuckets(Alice);
});

test.beforeEach(async ({ localPage }) => {
  await localPage.getByTestId("nav-storage").click();
  // Select the bucket we created.
  await localPage.getByTestId("s3-bucket-selector").selectOption({ label: bucketName });
});

test("upload object appears in objects table", async ({ localPage }) => {
  await localPage.getByTestId("s3-upload-button").click();

  const fileInput = localPage.getByTestId("s3-upload-file-input");
  await fileInput.setInputFiles({
    name: "alpha.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("alpha-content"),
  });
  // Auto-fill should have populated the key as `alpha.txt`. Submit.
  await localPage.getByTestId("s3-upload-submit").click();

  await expect(localPage.getByTestId("s3-object-row-alpha.txt")).toBeVisible({
    timeout: 60_000,
  });
});

test("download bytes match upload", async ({ localPage }) => {
  // Upload a fresh file with known content.
  const content = `bytes-${Date.now()}`;
  await localPage.getByTestId("s3-upload-button").click();
  await localPage.getByTestId("s3-upload-file-input").setInputFiles({
    name: "round.txt",
    mimeType: "text/plain",
    buffer: Buffer.from(content),
  });
  await localPage.getByTestId("s3-upload-submit").click();
  await expect(localPage.getByTestId("s3-object-row-round.txt")).toBeVisible({
    timeout: 60_000,
  });

  const downloadPromise = localPage.waitForEvent("download");
  await localPage.getByTestId("s3-download-round.txt").click();
  const download = await downloadPromise;
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  expect(Buffer.concat(chunks).toString("utf-8")).toBe(content);
});

test("delete object removes the row", async ({ localPage }) => {
  // Upload first so we have something to delete.
  await localPage.getByTestId("s3-upload-button").click();
  await localPage.getByTestId("s3-upload-file-input").setInputFiles({
    name: "delete-me.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("bye"),
  });
  await localPage.getByTestId("s3-upload-submit").click();
  await expect(localPage.getByTestId("s3-object-row-delete-me.txt")).toBeVisible({
    timeout: 60_000,
  });

  await localPage.getByTestId("s3-delete-object-delete-me.txt").click();

  await expect(localPage.getByTestId("s3-object-row-delete-me.txt")).toBeHidden({
    timeout: 60_000,
  });
});
