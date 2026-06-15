import { useState } from "react";
import { Archive, Plug, User } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  useIsConnected,
  useSignerAddress,
  useBuckets,
  useSelectedBucket,
  useCreations,
  dismissCreation,
  type CreationStatus,
} from "@/state";
import ObjectBrowser from "@/components/ObjectBrowser";
import EmptyState from "@/components/EmptyState";
import NewBucketDialog from "@/components/NewBucketDialog";
import ConnectDialog from "@/components/ConnectDialog";
import AccountDialog from "@/components/AccountDialog";

export default function S3Page() {
  const connected = useIsConnected();
  const signerAddress = useSignerAddress();
  const buckets = useBuckets();
  const selectedBucket = useSelectedBucket();
  const creations = useCreations();
  const [showConnect, setShowConnect] = useState(false);
  const [showAccount, setShowAccount] = useState(false);
  const [showNewBucket, setShowNewBucket] = useState(false);

  if (!connected) {
    return (
      <>
        <EmptyState
          icon={<Plug className="h-12 w-12" />}
          title="Connect to Chain"
          description="Connect to your parachain node to start using S3 Storage."
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
          description="Choose a development account to access your buckets."
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

  if (buckets.length === 0 && !selectedBucket) {
    return (
      <>
        <EmptyState
          icon={<Archive className="h-12 w-12" />}
          title="No Buckets Yet"
          description="Create your first S3 bucket to start storing objects."
          action={
            <Button data-testid="create-first-bucket" onClick={() => setShowNewBucket(true)}>
              <Archive className="mr-2 h-4 w-4" />
              Create Your First Bucket
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

        <NewBucketDialog open={showNewBucket} onOpenChange={setShowNewBucket} />
      </>
    );
  }

  if (!selectedBucket) {
    return (
      <EmptyState
        icon={<Archive className="h-12 w-12" />}
        title="Select a Bucket"
        description="Choose a bucket from the sidebar to browse objects."
      />
    );
  }

  return <ObjectBrowser />;
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
