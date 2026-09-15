// SPDX-License-Identifier: GPL-3.0-only

import { useState } from "react";
import { HardDrive, Plug, User } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  useIsConnected,
  useSignerAddress,
  useDrives,
  useSelectedDrive,
  useCreations,
  dismissCreation,
  type CreationStatus,
} from "@/state";
import FileBrowser from "@/components/FileBrowser";
import EmptyState from "@/components/EmptyState";
import NewDriveDialog from "@/components/NewDriveDialog";
import ConnectDialog from "@/components/ConnectDialog";
import AccountDialog from "@/components/AccountDialog";

export default function DrivePage() {
  const connected = useIsConnected();
  const signerAddress = useSignerAddress();
  const drives = useDrives();
  const selectedDrive = useSelectedDrive();
  const creations = useCreations();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);
  const [showNewDrive, setShowNewDrive] = useState(false);

  if (!connected) {
    return (
      <>
        <EmptyState
          icon={<Plug className="h-12 w-12" />}
          title="Connect to Chain"
          description="Connect to your parachain node to start using Web3 Drive."
          action={
            <Button data-testid="connect-empty" onClick={() => setShowConnect(true)}>
              <Plug className="mr-2 h-4 w-4" />
              Connect
            </Button>
          }
        />
        <ConnectDialog open={showConnect} onOpenChange={setShowConnect} />
      </>
    );
  }

  if (!signerAddress) {
    return (
      <>
        <EmptyState
          icon={<User className="h-12 w-12" />}
          title="Select an Account"
          description="Choose a development account to access your drives."
          action={
            <Button data-testid="select-account-empty" onClick={() => setShowAccount(true)}>
              <User className="mr-2 h-4 w-4" />
              Select Account
            </Button>
          }
        />
        <AccountDialog open={showAccount} onOpenChange={setShowAccount} />
      </>
    );
  }

  if (drives.length === 0 && !selectedDrive) {
    return (
      <>
        <EmptyState
          icon={<HardDrive className="h-12 w-12" />}
          title="No Drives Yet"
          description="Create your first decentralized drive to start storing files."
          action={
            <Button data-testid="create-first-drive" onClick={() => setShowNewDrive(true)}>
              <HardDrive className="mr-2 h-4 w-4" />
              Create Your First Drive
            </Button>
          }
        />

        {creations.length > 0 && (
          <div className="max-w-md mx-auto mt-6 space-y-2">
            {creations.map((item) => (
              <CreationCard key={item.id} item={item} onDismiss={dismissCreation} />
            ))}
          </div>
        )}

        <NewDriveDialog open={showNewDrive} onOpenChange={setShowNewDrive} />
      </>
    );
  }

  if (!selectedDrive) {
    return (
      <EmptyState
        icon={<HardDrive className="h-12 w-12" />}
        title="Select a Drive"
        description="Choose a drive from the sidebar to browse files."
      />
    );
  }

  return <FileBrowser />;
}

function CreationCard({
  item,
  onDismiss,
}: {
  item: CreationStatus;
  onDismiss: (id: string) => void;
}) {
  const isDismissible = item.stage === "ready" || item.stage === "failed";

  return (
    <div
      className={`rounded-lg border p-3 text-sm ${
        item.stage === "ready"
          ? "border-emerald-200 bg-emerald-50"
          : item.stage === "failed"
            ? "border-red-200 bg-red-50"
            : "border-indigo-200 bg-indigo-50"
      }`}
    >
      <div className="flex justify-between items-start">
        <div>
          <p className="font-medium">{item.name}</p>
          <p className="text-xs text-muted-foreground mt-0.5">
            {item.stage === "failed" ? item.error : item.stage}
          </p>
        </div>
        {isDismissible && (
          <button
            onClick={() => onDismiss(item.id)}
            className="text-xs text-muted-foreground hover:text-foreground"
          >
            Dismiss
          </button>
        )}
      </div>
    </div>
  );
}
