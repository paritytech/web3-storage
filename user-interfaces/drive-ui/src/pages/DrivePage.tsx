import { useState } from "react";
import { HardDrive, Plug, User } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChain } from "@/hooks/useChain";
import { useDrive } from "@/hooks/useDrive";
import FileBrowser from "@/components/FileBrowser";
import EmptyState from "@/components/EmptyState";
import NewDriveDialog from "@/components/NewDriveDialog";
import ConnectDialog from "@/components/ConnectDialog";
import AccountDialog from "@/components/AccountDialog";

export default function DrivePage() {
  const { connected } = useChain();
  const { signerAddress, drives, selectedDrive, creations, dismissCreation } = useDrive();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);
  const [showNewDrive, setShowNewDrive] = useState(false);

  // State 1: Not connected
  if (!connected) {
    return (
      <>
        <EmptyState
          icon={<Plug className="h-12 w-12" />}
          title="Connect to Chain"
          description="Connect to your parachain node to start using Web3 Drive."
          action={
            <Button onClick={() => setShowConnect(true)}>
              <Plug className="mr-2 h-4 w-4" />
              Connect
            </Button>
          }
        />
        <ConnectDialog open={showConnect} onOpenChange={setShowConnect} />
      </>
    );
  }

  // State 2: No signer
  if (!signerAddress) {
    return (
      <>
        <EmptyState
          icon={<User className="h-12 w-12" />}
          title="Select an Account"
          description="Choose a development account to access your drives."
          action={
            <Button onClick={() => setShowAccount(true)}>
              <User className="mr-2 h-4 w-4" />
              Select Account
            </Button>
          }
        />
        <AccountDialog open={showAccount} onOpenChange={setShowAccount} />
      </>
    );
  }

  // State 3: No drives
  if (drives.length === 0 && !selectedDrive) {
    return (
      <>
        <EmptyState
          icon={<HardDrive className="h-12 w-12" />}
          title="No Drives Yet"
          description="Create your first decentralized drive to start storing files."
          action={
            <Button onClick={() => setShowNewDrive(true)}>
              <HardDrive className="mr-2 h-4 w-4" />
              Create Your First Drive
            </Button>
          }
        />

        {/* Show creation status cards inline */}
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

  // State 4: No drive selected (drives exist)
  if (!selectedDrive) {
    return (
      <EmptyState
        icon={<HardDrive className="h-12 w-12" />}
        title="Select a Drive"
        description="Choose a drive from the sidebar to browse files."
      />
    );
  }

  // State 5: Drive selected — show file browser
  return <FileBrowser />;
}

// Inline creation card for the empty state
function CreationCard({
  item,
  onDismiss,
}: {
  item: { id: string; name: string; stage: string; error?: string };
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
