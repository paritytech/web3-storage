// SPDX-License-Identifier: Apache-2.0

//! wasm-bindgen exports for TypeScript (`packages/storage-formats`): the tree and
//! the S3 objects over a blob store that JS implements. The provider HTTP
//! requests and their signatures stay in JS; this module runs the format, the
//! tree and the object mapping. The result types are generated as TypeScript
//! interfaces with `tsify`.

use crate::format::{Cid, DirectoryEntry, EntryType, MetadataEntry};
use crate::s3::{ListObjectsParams, Objects, PutObjectOptions, S3ClientError};
use crate::tree::{BlobLength, BlobStore, EmptyParents, Tree};
use crate::{FsClientError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use storage_primitives::{BucketId, Commitment, MmrLeaf, MmrProof};
use tsify_next::Tsify;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(typescript_custom_section)]
const BLOB_STORE: &'static str = r#"
/** The blob operations of one bucket. JS implements them over the provider's HTTP API. */
export interface BlobStore {
  /** Upload a blob; resolves to its CID (32 bytes). */
  putBlob(bytes: Uint8Array): Promise<Uint8Array>;
  /** Read the blob `cid`: exactly `length` bytes, or at most `length` bytes when `exact` is false. */
  getBlob(cid: Uint8Array, length: bigint, exact: boolean): Promise<Uint8Array>;
  /** Append CIDs to the bucket's MMR, in order. */
  commit(cids: Uint8Array[]): Promise<void>;
  /** The bucket's current MMR commitment, or `null` when it has none. */
  commitment(): Promise<StoreCommitment | null>;
  /** MMR leaf `leafIndex` of the bucket and its inclusion proof. */
  mmrProof(leafIndex: bigint): Promise<StoreMmrProof>;
}

/** A file's metadata and content. */
export interface FileRead extends FileInfo {
  bytes: Uint8Array;
}

/** An object's metadata and content. */
export interface GetObjectResponse extends HeadObjectResponse {
  data: Uint8Array;
}
"#;

#[wasm_bindgen]
extern "C" {
    /// A [`BlobStore`] implemented in JS.
    #[wasm_bindgen(typescript_type = "BlobStore")]
    pub type JsBlobStore;

    #[wasm_bindgen(method, catch, js_name = putBlob)]
    fn put_blob_js(
        this: &JsBlobStore,
        bytes: &[u8],
    ) -> std::result::Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = getBlob)]
    fn get_blob_js(
        this: &JsBlobStore,
        cid: &[u8],
        length: u64,
        exact: bool,
    ) -> std::result::Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = commit)]
    fn commit_js(
        this: &JsBlobStore,
        cids: js_sys::Array,
    ) -> std::result::Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = commitment)]
    fn commitment_js(this: &JsBlobStore) -> std::result::Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = mmrProof)]
    fn mmr_proof_js(
        this: &JsBlobStore,
        leaf_index: u64,
    ) -> std::result::Result<js_sys::Promise, JsValue>;
}

/// A bucket's MMR commitment, as the JS store returns it.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct StoreCommitment {
    #[tsify(type = "Uint8Array")]
    pub mmr_root: [u8; 32],
    #[tsify(type = "bigint")]
    pub start_seq: u64,
    #[tsify(type = "bigint")]
    pub leaf_count: u64,
}

/// An MMR leaf, as the JS store returns it.
#[derive(Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct StoreMmrLeaf {
    pub leaf_version: u8,
    #[tsify(type = "Uint8Array")]
    pub data_root: [u8; 32],
    #[tsify(type = "bigint")]
    pub data_size: u64,
    #[tsify(type = "bigint")]
    pub total_size: u64,
}

/// An MMR leaf and its inclusion proof, as the JS store returns it.
#[derive(Deserialize, Tsify)]
pub struct StoreMmrProof {
    pub leaf: StoreMmrLeaf,
    #[tsify(type = "Uint8Array[]")]
    pub items: Vec<[u8; 32]>,
}

impl From<StoreCommitment> for Commitment {
    fn from(commitment: StoreCommitment) -> Self {
        Commitment {
            mmr_root: commitment.mmr_root.into(),
            start_seq: commitment.start_seq,
            leaf_count: commitment.leaf_count,
        }
    }
}

impl From<StoreMmrProof> for MmrProof {
    fn from(proof: StoreMmrProof) -> Self {
        MmrProof {
            leaf: MmrLeaf {
                leaf_version: proof.leaf.leaf_version,
                data_root: proof.leaf.data_root.into(),
                data_size: proof.leaf.data_size,
                total_size: proof.leaf.total_size,
            },
            items: proof.items.into_iter().map(Cid::from).collect(),
        }
    }
}

/// Await a store call. A rejection keeps the JS error's message, so a
/// provider status such as `PUT /node: 403` reaches the caller.
async fn settle(call: std::result::Result<js_sys::Promise, JsValue>) -> Result<JsValue> {
    let promise = call.map_err(store_error)?;
    JsFuture::from(promise).await.map_err(store_error)
}

fn store_error(value: JsValue) -> FsClientError {
    let message = value
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .unwrap_or_else(|| format!("{value:?}"));
    FsClientError::Store(message)
}

fn from_store<T: for<'de> Deserialize<'de>>(value: JsValue) -> Result<T> {
    serde_wasm_bindgen::from_value(value)
        .map_err(|e| FsClientError::Store(format!("invalid store response: {e}")))
}

fn cid_from(value: &JsValue) -> Result<Cid> {
    let bytes = js_sys::Uint8Array::new(value).to_vec();
    if bytes.len() != 32 {
        return Err(FsClientError::Store(format!(
            "the store returned a {}-byte CID",
            bytes.len()
        )));
    }
    Ok(Cid::from_slice(&bytes))
}

#[async_trait::async_trait(?Send)]
impl BlobStore for JsBlobStore {
    async fn put_blob(&self, _bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        cid_from(&settle(self.put_blob_js(data)).await?)
    }

    async fn get_blob(
        &self,
        _bucket_id: BucketId,
        cid: Cid,
        length: BlobLength,
    ) -> Result<Vec<u8>> {
        let (length, exact) = match length {
            BlobLength::Exact(size) => (size, true),
            BlobLength::AtMost(max) => (max, false),
        };
        let bytes = settle(self.get_blob_js(cid.as_bytes(), length, exact)).await?;
        Ok(js_sys::Uint8Array::new(&bytes).to_vec())
    }

    async fn commit(&self, _bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        let cids: js_sys::Array = data_roots
            .iter()
            .map(|cid| JsValue::from(js_sys::Uint8Array::from(cid.as_bytes())))
            .collect();
        settle(self.commit_js(cids)).await.map(drop)
    }

    async fn commitment(&self, _bucket_id: BucketId) -> Result<Option<Commitment>> {
        let commitment: Option<StoreCommitment> = from_store(settle(self.commitment_js()).await?)?;
        Ok(commitment.map(Commitment::from))
    }

    async fn mmr_proof(&self, _bucket_id: BucketId, leaf_index: u64) -> Result<MmrProof> {
        let proof: StoreMmrProof = from_store(settle(self.mmr_proof_js(leaf_index)).await?)?;
        Ok(proof.into())
    }
}

/// A directory entry.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    /// Absolute path, e.g. `/docs/a.txt`.
    pub path: String,
    #[tsify(type = "\"file\" | \"directory\"")]
    pub entry_type: &'static str,
    /// File: manifest CID. Directory: directory node CID. 0x-hex.
    pub cid: String,
    /// Content bytes (0 for a directory).
    pub size: u64,
    /// Unix seconds.
    pub mtime: u64,
}

/// A file's metadata, read from its manifest.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    /// Unix seconds.
    pub mtime: u64,
    pub content_type: String,
    /// Root of the content's chunk tree (its CID), 0x-hex.
    pub content_root: String,
    /// CID of the file's manifest, 0x-hex.
    pub manifest_cid: String,
    #[tsify(type = "Record<string, string>")]
    pub user_metadata: BTreeMap<String, String>,
}

/// Result of `mkdir` and `delete`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct TreeWriteResult {
    /// CID of the new root directory, 0x-hex.
    pub root_cid: String,
}

/// Result of `putFile`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PutFileResult {
    /// CID of the new root directory, 0x-hex.
    pub root_cid: String,
    pub content_root: String,
    pub manifest_cid: String,
    pub size: f64,
}

/// Options of `putFile`.
#[derive(Deserialize, Default, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct PutFileOptions {
    pub content_type: Option<String>,
    /// Stored in the manifest's user metadata, sorted by key bytes.
    #[tsify(optional, type = "Record<string, string>")]
    pub user_metadata: Option<HashMap<String, String>>,
}

/// Options of `list`.
#[derive(Deserialize, Default, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct ListOptions {
    /// Every entry below the directory, depth-first, instead of its children.
    pub recursive: Option<bool>,
}

/// Options of `delete`.
#[derive(Deserialize, Default, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteOptions {
    /// Also remove parent directories that become empty (never the root).
    pub prune_empty_parents: Option<bool>,
}

/// Options of `putObject`.
#[derive(Deserialize, Default, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct PutObjectInput {
    pub content_type: Option<String>,
    /// User metadata, stored in the object's manifest. Keys are lowercased.
    #[tsify(optional, type = "Record<string, string>")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Result of `putObject`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PutObjectResult {
    /// CID of the object content (its `data_root`), 0x-hex.
    pub cid: String,
    /// Same as `cid`.
    pub etag: String,
    pub size: f64,
}

/// An object's metadata.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct HeadObjectResponse {
    pub key: String,
    /// Stored content type (`application/octet-stream` when none was given).
    pub content_type: String,
    pub size: f64,
    /// User metadata with lowercased keys.
    #[tsify(type = "Record<string, string>")]
    pub metadata: BTreeMap<String, String>,
    /// CID of the object content, 0x-hex.
    pub etag: String,
    /// Milliseconds since the epoch (stored with second precision).
    pub last_modified: f64,
}

/// One listed object.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub key: String,
    pub size: f64,
    /// Milliseconds since the epoch (stored with second precision).
    pub last_modified: f64,
    /// Not set by `listObjects`; use `headObject` for the content CID.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[tsify(optional)]
    pub etag: Option<String>,
}

/// Options of `listObjects`.
#[derive(Deserialize, Default, Tsify)]
#[serde(rename_all = "camelCase", default)]
pub struct ListObjectsOptions {
    pub prefix: Option<String>,
    /// Keys containing this after the prefix are grouped into `commonPrefixes`.
    pub delimiter: Option<String>,
    /// Return only keys after this one (S3 `start-after`).
    pub start_after: Option<String>,
    /// Maximum objects plus common prefixes to return (default and maximum 1000).
    pub max_keys: Option<u32>,
}

/// One page of `listObjects`.
#[derive(Serialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct ListObjectsResult {
    pub objects: Vec<ObjectSummary>,
    pub common_prefixes: Vec<String>,
    pub is_truncated: bool,
    /// When truncated: pass as `startAfter` to get the next page.
    pub next_start_after: Option<String>,
}

fn hex(cid: &Cid) -> String {
    format!("{cid:#x}")
}

fn now() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

/// Serialize with `u64` as `bigint`.
fn to_js<T: Serialize>(value: &T) -> std::result::Result<JsValue, JsError> {
    let serializer = serde_wasm_bindgen::Serializer::new()
        .serialize_large_number_types_as_bigints(true)
        .serialize_maps_as_objects(true);
    value
        .serialize(&serializer)
        .map_err(|e| JsError::new(&e.to_string()))
}

fn from_js<T: for<'de> Deserialize<'de> + Default>(
    value: JsValue,
) -> std::result::Result<T, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&e.to_string()))
}

/// A JS `Error` with the error's text as message and its variant name as
/// `code`.
fn js_error(error: impl std::fmt::Display + std::fmt::Debug) -> JsValue {
    let debug = format!("{error:?}");
    let code = debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default();
    let js = js_sys::Error::new(&error.to_string());
    let _ = js_sys::Reflect::set(&js, &"code".into(), &code.into());
    js.into()
}

fn s3_error(error: S3ClientError) -> JsValue {
    match error {
        S3ClientError::FileSystem(inner) => js_error(inner),
        other => js_error(other),
    }
}

fn entry_type(entry: &DirectoryEntry) -> &'static str {
    match entry.entry_type {
        EntryType::File => "file",
        EntryType::Directory => "directory",
    }
}

fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    }
}

fn metadata_map(entries: &[MetadataEntry]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|e| {
            (
                String::from_utf8_lossy(&e.key).into_owned(),
                String::from_utf8_lossy(&e.value).into_owned(),
            )
        })
        .collect()
}

/// File-system operations on one bucket.
#[wasm_bindgen(js_name = WasmTree)]
pub struct WasmTree {
    tree: Tree<JsBlobStore>,
}

#[wasm_bindgen(js_class = WasmTree)]
impl WasmTree {
    #[wasm_bindgen(constructor)]
    pub fn new(store: JsBlobStore, bucket_id: u64) -> WasmTree {
        WasmTree {
            tree: Tree::with_clock(store, bucket_id, now),
        }
    }

    /// CID of the root directory, 0x-hex, or `null` for a bucket with no commits.
    #[wasm_bindgen(js_name = rootCid, unchecked_return_type = "string | null")]
    pub async fn root_cid(&self) -> std::result::Result<JsValue, JsValue> {
        let root = self.tree.load_root().await.map_err(js_error)?;
        Ok(root
            .cid
            .as_ref()
            .map(|cid| JsValue::from(hex(cid)))
            .unwrap_or(JsValue::NULL))
    }

    /// The entries of the directory at `path`, sorted by name bytes; with
    /// `recursive`, every entry below it, depth-first.
    #[wasm_bindgen(unchecked_return_type = "TreeEntry[]")]
    pub async fn list(
        &self,
        path: String,
        #[wasm_bindgen(unchecked_param_type = "ListOptions | undefined")] options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let options: ListOptions = from_js(options)?;
        let entries = if options.recursive.unwrap_or(false) {
            self.tree.entries_under(&path).await.map_err(js_error)?
        } else {
            let children = self.tree.list(&path).await.map_err(js_error)?;
            children.into_iter().map(|e| (e.name_str(), e)).collect()
        };
        let entries: Vec<TreeEntry> = entries
            .iter()
            .map(|(rel, e)| TreeEntry {
                name: e.name_str(),
                path: child_path(&path, rel),
                entry_type: entry_type(e),
                cid: hex(&e.cid),
                size: e.size,
                mtime: e.mtime,
            })
            .collect();
        to_js(&entries).map_err(JsValue::from)
    }

    /// The file at `path`: its metadata and content.
    #[wasm_bindgen(js_name = getFile, unchecked_return_type = "FileRead")]
    pub async fn get_file(&self, path: String) -> std::result::Result<JsValue, JsValue> {
        let file = self.tree.get_file(&path).await.map_err(js_error)?;
        let info = FileInfo {
            path,
            size: file.stat.size(),
            mtime: file.stat.mtime(),
            content_type: file.stat.content_type(),
            content_root: hex(&file.stat.content_root()),
            manifest_cid: hex(&file.stat.entry.cid),
            user_metadata: metadata_map(&file.stat.manifest.user_metadata),
        };
        let value = to_js(&info)?;
        js_sys::Reflect::set(
            &value,
            &"bytes".into(),
            &js_sys::Uint8Array::from(&file.data[..]),
        )?;
        Ok(value)
    }

    /// Write the file at `path`, creating the directories above it.
    #[wasm_bindgen(js_name = putFile, unchecked_return_type = "PutFileResult")]
    pub async fn put_file(
        &self,
        path: String,
        bytes: Vec<u8>,
        #[wasm_bindgen(unchecked_param_type = "PutFileOptions | undefined")] options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let options: PutFileOptions = from_js(options)?;
        let metadata = options
            .user_metadata
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| MetadataEntry::try_new(k.as_bytes(), v.as_bytes()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(js_error)?;
        let content_type = options.content_type.unwrap_or_default();
        let (root, stat) = self
            .tree
            .put_file(&path, &bytes, &content_type, metadata)
            .await
            .map_err(js_error)?;
        to_js(&PutFileResult {
            root_cid: hex(&root),
            content_root: hex(&stat.content_root()),
            manifest_cid: hex(&stat.entry.cid),
            size: bytes.len() as f64,
        })
        .map_err(JsValue::from)
    }

    /// Create the directory at `path`, and the directories above it.
    #[wasm_bindgen(unchecked_return_type = "TreeWriteResult")]
    pub async fn mkdir(&self, path: String) -> std::result::Result<JsValue, JsValue> {
        let root = self.tree.mkdir(&path).await.map_err(js_error)?;
        to_js(&TreeWriteResult {
            root_cid: hex(&root),
        })
        .map_err(JsValue::from)
    }

    /// Delete the file or empty directory at `path`.
    #[wasm_bindgen(unchecked_return_type = "TreeWriteResult")]
    pub async fn delete(
        &self,
        path: String,
        #[wasm_bindgen(unchecked_param_type = "DeleteOptions | undefined")] options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let options: DeleteOptions = from_js(options)?;
        let empty_parents = if options.prune_empty_parents.unwrap_or(false) {
            EmptyParents::Remove
        } else {
            EmptyParents::Keep
        };
        let root = self
            .tree
            .delete(&path, empty_parents)
            .await
            .map_err(js_error)?;
        to_js(&TreeWriteResult {
            root_cid: hex(&root),
        })
        .map_err(JsValue::from)
    }
}

/// S3-style objects of one bucket, stored in its tree: key `k` is the file `/k`.
#[wasm_bindgen(js_name = WasmObjects)]
pub struct WasmObjects {
    objects: Objects<JsBlobStore>,
}

#[wasm_bindgen(js_class = WasmObjects)]
impl WasmObjects {
    #[wasm_bindgen(constructor)]
    pub fn new(store: JsBlobStore, bucket_id: u64) -> WasmObjects {
        WasmObjects {
            objects: Objects::new(Tree::with_clock(store, bucket_id, now)),
        }
    }

    /// Store `data` under `key`, replacing an object with the same key.
    #[wasm_bindgen(js_name = putObject, unchecked_return_type = "PutObjectResult")]
    pub async fn put_object(
        &self,
        key: String,
        data: Vec<u8>,
        #[wasm_bindgen(unchecked_param_type = "PutObjectInput | undefined")] options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let options: PutObjectInput = from_js(options)?;
        let put = self
            .objects
            .put_object(
                &key,
                &data,
                PutObjectOptions {
                    content_type: options.content_type,
                    metadata: options.metadata.unwrap_or_default(),
                },
            )
            .await
            .map_err(s3_error)?;
        to_js(&PutObjectResult {
            cid: hex(&put.cid),
            etag: put.etag,
            size: put.size as f64,
        })
        .map_err(JsValue::from)
    }

    /// The object `key`: its metadata and content.
    #[wasm_bindgen(js_name = getObject, unchecked_return_type = "GetObjectResponse")]
    pub async fn get_object(&self, key: String) -> std::result::Result<JsValue, JsValue> {
        let object = self.objects.get_object(&key).await.map_err(s3_error)?;
        let head = HeadObjectResponse {
            key,
            content_type: object.content_type,
            size: object.size as f64,
            metadata: object.metadata.into_iter().collect(),
            etag: object.etag,
            last_modified: object.last_modified as f64 * 1000.0,
        };
        let value = to_js(&head)?;
        js_sys::Reflect::set(
            &value,
            &"data".into(),
            &js_sys::Uint8Array::from(&object.data[..]),
        )?;
        Ok(value)
    }

    /// The metadata of object `key`, without its content.
    #[wasm_bindgen(js_name = headObject, unchecked_return_type = "HeadObjectResponse")]
    pub async fn head_object(&self, key: String) -> std::result::Result<JsValue, JsValue> {
        let head = self.objects.head_object(&key).await.map_err(s3_error)?;
        to_js(&HeadObjectResponse {
            key,
            content_type: head.content_type,
            size: head.size as f64,
            metadata: head.metadata.into_iter().collect(),
            etag: head.etag,
            last_modified: head.last_modified as f64 * 1000.0,
        })
        .map_err(JsValue::from)
    }

    /// Delete the object `key`, and the directories it leaves empty.
    #[wasm_bindgen(js_name = deleteObject)]
    pub async fn delete_object(&self, key: String) -> std::result::Result<(), JsValue> {
        self.objects.delete_object(&key).await.map_err(s3_error)
    }

    /// One page of objects, sorted by key bytes.
    #[wasm_bindgen(js_name = listObjects, unchecked_return_type = "ListObjectsResult")]
    pub async fn list_objects(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ListObjectsOptions | undefined")] options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let options: ListObjectsOptions = from_js(options)?;
        let page = self
            .objects
            .list_objects_v2(ListObjectsParams {
                prefix: options.prefix,
                delimiter: options.delimiter,
                start_after: options.start_after,
                max_keys: options.max_keys,
            })
            .await
            .map_err(s3_error)?;
        to_js(&ListObjectsResult {
            objects: page
                .contents
                .into_iter()
                .map(|o| ObjectSummary {
                    key: o.key,
                    size: o.size as f64,
                    last_modified: o.last_modified as f64 * 1000.0,
                    etag: o.etag,
                })
                .collect(),
            common_prefixes: page.common_prefixes,
            is_truncated: page.is_truncated,
            next_start_after: page.next_start_after,
        })
        .map_err(JsValue::from)
    }
}
