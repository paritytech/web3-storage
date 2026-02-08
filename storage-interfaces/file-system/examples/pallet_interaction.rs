//! Drive Registry Pallet Interaction Example
//!
//! This example demonstrates how to interact with the DriveRegistry pallet
//! on-chain using extrinsics and storage queries.
//!
//! This shows the low-level pallet interactions that the FileSystemClient
//! would perform under the hood.
//!
//! Run with: `cargo run --example pallet_interaction`

use file_system_primitives::Cid;

fn main() {
    println!("=== Drive Registry Pallet Interaction ===\n");

    println!("This example demonstrates the on-chain operations for drive management.\n");

    show_extrinsics();
    println!();
    show_storage_queries();
    println!();
    show_events();
    println!();
    show_workflow_example();
}

fn show_extrinsics() {
    println!("Available Extrinsics:");
    println!("=====================\n");

    println!("1. create_drive(bucket_id, root_cid, name)");
    println!("   - Creates a new drive and returns a unique drive_id");
    println!("   - Emits: DriveCreated event");
    println!("   Example:");
    println!("   ```rust");
    println!("   let bucket_id = 42u64;");
    println!("   let root_cid = H256::zero(); // Empty root directory");
    println!("   let name = Some(\"My Personal Drive\".as_bytes().to_vec());");
    println!("   ");
    println!("   let tx = api.tx().drive_registry().create_drive(");
    println!("       bucket_id,");
    println!("       root_cid,");
    println!("       name,");
    println!("   );");
    println!("   let events = tx.sign_and_submit_then_watch_default(&signer)");
    println!("       .await?.wait_for_finalized_success().await?;");
    println!("   ```\n");

    println!("2. update_root_cid(drive_id, new_root_cid)");
    println!("   - Updates the root CID after file system changes");
    println!("   - Only the drive owner can call this");
    println!("   - Emits: RootCidUpdated event");
    println!("   Example:");
    println!("   ```rust");
    println!("   let drive_id = 1u64;");
    println!("   let new_root_cid = H256::from([1u8; 32]); // New root after changes");
    println!("   ");
    println!("   let tx = api.tx().drive_registry().update_root_cid(");
    println!("       drive_id,");
    println!("       new_root_cid,");
    println!("   );");
    println!("   ```\n");

    println!("3. delete_drive(drive_id)");
    println!("   - Removes a drive from the registry");
    println!("   - Only the drive owner can call this");
    println!("   - Emits: DriveDeleted event");
    println!("   Example:");
    println!("   ```rust");
    println!("   let drive_id = 1u64;");
    println!("   let tx = api.tx().drive_registry().delete_drive(drive_id);");
    println!("   ```\n");

    println!("4. update_drive_name(drive_id, name)");
    println!("   - Changes the name of a drive");
    println!("   - Only the drive owner can call this");
    println!("   - Emits: DriveNameUpdated event");
    println!("   Example:");
    println!("   ```rust");
    println!("   let drive_id = 1u64;");
    println!("   let new_name = Some(\"Work Drive\".as_bytes().to_vec());");
    println!("   let tx = api.tx().drive_registry().update_drive_name(drive_id, new_name);");
    println!("   ```");
}

fn show_storage_queries() {
    println!("\nStorage Queries:");
    println!("================\n");

    println!("1. Drives(drive_id) -> Option<DriveInfo>");
    println!("   - Get drive information by ID");
    println!("   Example:");
    println!("   ```rust");
    println!("   let drive_id = 1u64;");
    println!("   let drive_info = api.storage()");
    println!("       .drive_registry()");
    println!("       .drives(drive_id, None)");
    println!("       .await?;");
    println!("   ");
    println!("   if let Some(info) = drive_info {{");
    println!("       println!(\"Owner: {{}}\", info.owner);");
    println!("       println!(\"Bucket: {{}}\", info.bucket_id);");
    println!("       println!(\"Root CID: {{}}\", hex::encode(info.root_cid.as_bytes()));");
    println!("   }}");
    println!("   ```\n");

    println!("2. UserDrives(account_id) -> Vec<DriveId>");
    println!("   - Get all drive IDs owned by a user");
    println!("   Example:");
    println!("   ```rust");
    println!("   let account = AccountId32::from([1u8; 32]);");
    println!("   let drive_ids = api.storage()");
    println!("       .drive_registry()");
    println!("       .user_drives(account, None)");
    println!("       .await?;");
    println!("   ");
    println!("   println!(\"User has {{}} drives\", drive_ids.len());");
    println!("   ```\n");

    println!("3. NextDriveId() -> DriveId");
    println!("   - Get the next drive ID that will be assigned");
    println!("   Example:");
    println!("   ```rust");
    println!("   let next_id = api.storage()");
    println!("       .drive_registry()");
    println!("       .next_drive_id(None)");
    println!("       .await?;");
    println!("   println!(\"Next drive will have ID: {{}}\", next_id);");
    println!("   ```");
}

fn show_events() {
    println!("\nEvents:");
    println!("=======\n");

    println!("1. DriveCreated {{ drive_id, owner, bucket_id, root_cid }}");
    println!("   - Emitted when a new drive is created\n");

    println!("2. RootCidUpdated {{ drive_id, old_root_cid, new_root_cid }}");
    println!("   - Emitted when drive's root CID is updated\n");

    println!("3. DriveDeleted {{ drive_id, owner }}");
    println!("   - Emitted when a drive is deleted\n");

    println!("4. DriveNameUpdated {{ drive_id, name }}");
    println!("   - Emitted when a drive's name is changed");
}

fn show_workflow_example() {
    println!("\nComplete Workflow Example:");
    println!("==========================\n");

    println!("```rust");
    println!("use subxt::{{OnlineClient, PolkadotConfig}};");
    println!("use subxt::tx::PairSigner;");
    println!("use sp_keyring::AccountKeyring;");
    println!("use sp_core::H256;");
    println!();
    println!("#[tokio::main]");
    println!("async fn main() -> Result<(), Box<dyn std::error::Error>> {{");
    println!("    // Connect to parachain");
    println!("    let api = OnlineClient::<PolkadotConfig>::new().await?;");
    println!("    let signer = PairSigner::new(AccountKeyring::Alice.pair());");
    println!();
    println!("    // Step 1: Create empty root directory");
    println!("    let root_dir = DirectoryNode::new_empty(1); // drive_id = 1");
    println!("    let root_cid = root_dir.compute_cid();");
    println!("    let root_bytes = root_dir.to_scale_bytes();");
    println!();
    println!("    // Step 2: Upload root to Layer 0 storage");
    println!("    let bucket_id = 1u64;");
    println!("    storage_client.upload_node(bucket_id, &root_bytes).await?;");
    println!();
    println!("    // Step 3: Create drive on-chain");
    println!("    let create_tx = api.tx().drive_registry().create_drive(");
    println!("        bucket_id,");
    println!("        root_cid,");
    println!("        Some(b\"My Drive\".to_vec()),");
    println!("    );");
    println!();
    println!("    let events = create_tx");
    println!("        .sign_and_submit_then_watch_default(&signer)");
    println!("        .await?");
    println!("        .wait_for_finalized_success()");
    println!("        .await?;");
    println!();
    println!("    // Parse DriveCreated event to get drive_id");
    println!("    for event in events.iter() {{");
    println!("        if let Ok(Some(drive_created)) = event.as_event::<DriveCreated>() {{");
    println!("            println!(\"Drive created with ID: {{}}\", drive_created.drive_id);");
    println!("            break;");
    println!("        }}");
    println!("    }}");
    println!();
    println!("    // Step 4: Make changes to file system");
    println!("    // ... (upload files, modify directories, etc.)");
    println!();
    println!("    // Step 5: Update root CID on-chain");
    println!("    let new_root_cid = H256::from([1u8; 32]); // New root after changes");
    println!("    let update_tx = api.tx().drive_registry().update_root_cid(");
    println!("        drive_id,");
    println!("        new_root_cid,");
    println!("    );");
    println!();
    println!("    update_tx");
    println!("        .sign_and_submit_then_watch_default(&signer)");
    println!("        .await?");
    println!("        .wait_for_finalized_success()");
    println!("        .await?;");
    println!();
    println!("    println!(\"Root CID updated successfully\");");
    println!();
    println!("    Ok(())");
    println!("}}");
    println!("```");
}
