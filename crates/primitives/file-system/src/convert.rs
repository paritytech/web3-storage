// SPDX-License-Identifier: Apache-2.0

//! Conversions between SCALE and protobuf types plus protobuf serialization
//! helpers (std only).

use prost::Message;
use sp_runtime::BoundedVec;

use crate::{
    cid_to_string, proto, string_to_cid, DirectoryEntry, DirectoryNode, DriveId, EntryType,
    FileChunk, FileManifest, FileSystemError, MetadataEntry,
};

impl From<EntryType> for proto::EntryType {
    fn from(entry_type: EntryType) -> Self {
        match entry_type {
            EntryType::File => proto::EntryType::File,
            EntryType::Directory => proto::EntryType::Directory,
        }
    }
}

impl From<proto::EntryType> for EntryType {
    fn from(entry_type: proto::EntryType) -> Self {
        match entry_type {
            proto::EntryType::File => EntryType::File,
            proto::EntryType::Directory => EntryType::Directory,
        }
    }
}

impl From<&DirectoryEntry> for proto::DirectoryEntry {
    fn from(entry: &DirectoryEntry) -> Self {
        Self {
            name: entry.name_str(),
            r#type: proto::EntryType::from(entry.entry_type) as i32,
            cid: cid_to_string(&entry.cid),
            size: entry.size,
            mtime: entry.mtime,
        }
    }
}

impl TryFrom<&proto::DirectoryEntry> for DirectoryEntry {
    type Error = FileSystemError;

    fn try_from(entry: &proto::DirectoryEntry) -> Result<Self, Self::Error> {
        let entry_type = match entry.r#type {
            0 => EntryType::File,
            1 => EntryType::Directory,
            _ => EntryType::File,
        };
        Ok(Self {
            name: BoundedVec::try_from(entry.name.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
            entry_type,
            cid: string_to_cid(&entry.cid)?,
            size: entry.size,
            mtime: entry.mtime,
        })
    }
}

impl From<&DirectoryNode> for proto::DirectoryNode {
    fn from(node: &DirectoryNode) -> Self {
        Self {
            drive_id: node.drive_id.to_string(),
            children: node
                .children
                .iter()
                .map(proto::DirectoryEntry::from)
                .collect(),
            metadata: node
                .metadata
                .iter()
                .map(|m| {
                    (
                        String::from_utf8_lossy(&m.key).into_owned(),
                        String::from_utf8_lossy(&m.value).into_owned(),
                    )
                })
                .collect(),
        }
    }
}

impl TryFrom<&proto::DirectoryNode> for DirectoryNode {
    type Error = FileSystemError;

    fn try_from(node: &proto::DirectoryNode) -> Result<Self, Self::Error> {
        let drive_id: DriveId = node
            .drive_id
            .parse()
            .map_err(|_| FileSystemError::InvalidPath)?;
        let children: Result<Vec<DirectoryEntry>, _> =
            node.children.iter().map(DirectoryEntry::try_from).collect();
        let metadata: Result<Vec<MetadataEntry>, _> = node
            .metadata
            .iter()
            .map(|(k, v)| {
                Ok(MetadataEntry {
                    key: BoundedVec::try_from(k.clone().into_bytes())
                        .map_err(|_| FileSystemError::InvalidPath)?,
                    value: BoundedVec::try_from(v.clone().into_bytes())
                        .map_err(|_| FileSystemError::InvalidPath)?,
                })
            })
            .collect();

        Ok(Self {
            drive_id,
            children: BoundedVec::try_from(children?).map_err(|_| FileSystemError::InvalidPath)?,
            metadata: BoundedVec::try_from(metadata?).map_err(|_| FileSystemError::InvalidPath)?,
        })
    }
}

impl From<&FileManifest> for proto::FileManifest {
    fn from(manifest: &FileManifest) -> Self {
        Self {
            drive_id: manifest.drive_id.to_string(),
            mime_type: manifest.mime_type_str(),
            total_size: manifest.total_size,
            chunks: manifest
                .chunks
                .iter()
                .map(|c| proto::FileChunk {
                    cid: cid_to_string(&c.cid),
                    sequence: c.sequence,
                })
                .collect(),
            encryption_params: String::from_utf8_lossy(&manifest.encryption_params).into_owned(),
        }
    }
}

impl TryFrom<&proto::FileManifest> for FileManifest {
    type Error = FileSystemError;

    fn try_from(manifest: &proto::FileManifest) -> Result<Self, Self::Error> {
        let drive_id: DriveId = manifest
            .drive_id
            .parse()
            .map_err(|_| FileSystemError::InvalidPath)?;
        let chunks: Result<Vec<FileChunk>, _> = manifest
            .chunks
            .iter()
            .map(|c| {
                Ok(FileChunk {
                    cid: string_to_cid(&c.cid)?,
                    sequence: c.sequence,
                })
            })
            .collect();

        Ok(Self {
            drive_id,
            mime_type: BoundedVec::try_from(manifest.mime_type.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
            total_size: manifest.total_size,
            chunks: BoundedVec::try_from(chunks?).map_err(|_| FileSystemError::InvalidPath)?,
            encryption_params: BoundedVec::try_from(
                manifest.encryption_params.clone().into_bytes(),
            )
            .map_err(|_| FileSystemError::InvalidPath)?,
        })
    }
}

impl DirectoryNode {
    /// Serialize to protobuf bytes
    pub fn to_proto_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let proto_node = proto::DirectoryNode::from(self);
        let mut buf = Vec::new();
        proto_node
            .encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    pub fn from_proto_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        let proto_node = proto::DirectoryNode::decode(bytes)
            .map_err(|_| FileSystemError::DeserializationError)?;
        Self::try_from(&proto_node)
    }
}

impl FileManifest {
    /// Serialize to protobuf bytes
    pub fn to_proto_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let proto_manifest = proto::FileManifest::from(self);
        let mut buf = Vec::new();
        proto_manifest
            .encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    pub fn from_proto_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        let proto_manifest = proto::FileManifest::decode(bytes)
            .map_err(|_| FileSystemError::DeserializationError)?;
        Self::try_from(&proto_manifest)
    }
}

#[cfg(test)]
mod tests {
    use crate::{compute_cid, proto, DirectoryEntry, DirectoryNode, EntryType};
    use sp_runtime::BoundedVec;

    #[test]
    fn test_proto_conversion() {
        let mut dir = DirectoryNode::new_empty(456);
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(b"test.txt".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"content"),
            size: 512,
            mtime: 1000000,
        };
        dir.add_child(entry).unwrap();

        // Convert to proto and back
        let proto_node = proto::DirectoryNode::from(&dir);
        let converted_back = DirectoryNode::try_from(&proto_node).unwrap();

        assert_eq!(dir.drive_id, converted_back.drive_id);
        assert_eq!(dir.children.len(), converted_back.children.len());
        assert_eq!(
            dir.children[0].name_str(),
            converted_back.children[0].name_str()
        );
    }

    #[test]
    fn test_proto_bytes_serialization() {
        let mut dir = DirectoryNode::new_empty(789);
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(b"doc.pdf".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"pdf_content"),
            size: 4096,
            mtime: 2000000,
        };
        dir.add_child(entry).unwrap();

        // Serialize to proto bytes and back
        let proto_bytes = dir.to_proto_bytes().unwrap();
        let decoded = DirectoryNode::from_proto_bytes(&proto_bytes).unwrap();

        assert_eq!(dir.drive_id, decoded.drive_id);
        assert_eq!(dir.children.len(), decoded.children.len());
    }
}
