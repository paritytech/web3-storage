import { useState, useEffect } from "react";
import {
  Download as DownloadIcon,
  Search,
  File,
  Loader2,
  CheckCircle,
  AlertCircle,
  Copy,
  HardDrive,
  Archive,
} from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useChain } from "@/hooks/useChain";
import { useStorage } from "@/hooks/useStorage";
import { toast } from "@/components/ui/toaster";
import { formatBytes, truncateHash } from "@/lib/utils";
import type { DriveInfo, BucketInfo } from "@/lib/storage";

interface DownloadResult {
  cid: string;
  size: number;
  contentType: string;
  data: Uint8Array;
}

export default function Download() {
  const { connected } = useChain();
  const {
    signerAddress,
    drives,
    buckets,
    loading,
    refreshDrives,
    refreshBuckets,
    downloadFromDrive,
    getObject,
  } = useStorage();

  const [cidInput, setCidInput] = useState("");
  const [selectedDrive, setSelectedDrive] = useState<DriveInfo | null>(null);
  const [selectedBucket, setSelectedBucket] = useState<BucketInfo | null>(null);
  const [downloadLoading, setDownloadLoading] = useState(false);
  const [result, setResult] = useState<DownloadResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Refresh drives/buckets on mount
  useEffect(() => {
    if (signerAddress && connected) {
      refreshDrives();
      refreshBuckets();
    }
  }, [signerAddress, connected, refreshDrives, refreshBuckets]);

  const handleDownloadByCid = async (bucketId: bigint, source: "drive" | "bucket") => {
    if (!cidInput.trim()) {
      toast({
        title: "Error",
        description: "Please enter a CID",
        variant: "destructive",
      });
      return;
    }

    setDownloadLoading(true);
    setError(null);
    setResult(null);

    try {
      let data: Uint8Array;

      if (source === "drive") {
        data = await downloadFromDrive(bucketId, cidInput);
      } else {
        data = await getObject(bucketId, cidInput);
      }

      setResult({
        cid: cidInput,
        size: data.length,
        contentType: "application/octet-stream",
        data,
      });

      toast({ title: "Success", description: "File retrieved successfully" });
    } catch (err) {
      const message = err instanceof Error ? err.message : "Download failed";
      setError(message);
      toast({
        title: "Error",
        description: message,
        variant: "destructive",
      });
    } finally {
      setDownloadLoading(false);
    }
  };

  const handleDownloadFromDrive = async () => {
    if (!selectedDrive) {
      toast({
        title: "Error",
        description: "Please select a drive",
        variant: "destructive",
      });
      return;
    }
    await handleDownloadByCid(selectedDrive.bucketId, "drive");
  };

  const handleDownloadFromBucket = async () => {
    if (!selectedBucket) {
      toast({
        title: "Error",
        description: "Please select a bucket",
        variant: "destructive",
      });
      return;
    }
    await handleDownloadByCid(selectedBucket.layer0BucketId, "bucket");
  };

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
    toast({ title: "Copied", description: "CID copied to clipboard" });
  };

  const saveToDevice = () => {
    if (!result) return;

    // Create blob and download
    const blob = new Blob([result.data], { type: result.contentType });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `download-${result.cid.slice(0, 8)}`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);

    toast({ title: "Success", description: "File saved to device" });
  };

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Download</h1>
          <p className="text-muted-foreground">Download files from storage</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <DownloadIcon className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p className="text-muted-foreground">
              Connect to the network to download files
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!signerAddress) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Download</h1>
          <p className="text-muted-foreground">Download files from storage</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center">
            <AlertCircle className="mx-auto h-12 w-12 mb-4 text-yellow-500" />
            <p className="text-muted-foreground mb-2">No signer set</p>
            <p className="text-sm text-muted-foreground">
              Go to the Accounts page to set a signing account first
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Download</h1>
        <p className="text-muted-foreground">Download files from storage</p>
      </div>

      {/* CID Input */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Search className="h-5 w-5" />
            Download by CID
          </CardTitle>
          <CardDescription>
            Enter a content identifier (CID) to download
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Input
            placeholder="0x..."
            value={cidInput}
            onChange={(e) => setCidInput(e.target.value)}
            className="font-mono"
          />
        </CardContent>
      </Card>

      {/* Download from Drive */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <HardDrive className="h-5 w-5" />
            Download from Drive
          </CardTitle>
          <CardDescription>
            Select a drive and download content by CID
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex gap-3">
            <select
              className="flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
              value={selectedDrive?.driveId.toString() || ""}
              onChange={(e) => {
                const drive = drives.find(
                  (d) => d.driveId.toString() === e.target.value
                );
                setSelectedDrive(drive || null);
              }}
            >
              <option value="">Select a drive...</option>
              {drives.map((drive) => (
                <option key={drive.driveId.toString()} value={drive.driveId.toString()}>
                  {drive.name || `Drive ${drive.driveId.toString()}`}
                </option>
              ))}
            </select>
            <Button
              onClick={handleDownloadFromDrive}
              disabled={downloadLoading || !selectedDrive || !cidInput}
            >
              {downloadLoading ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <DownloadIcon className="mr-2 h-4 w-4" />
              )}
              Download
            </Button>
          </div>
          {drives.length === 0 && (
            <p className="mt-2 text-xs text-muted-foreground">
              No drives found. Create one in the Drives page.
            </p>
          )}
        </CardContent>
      </Card>

      {/* Download from S3 Bucket */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Archive className="h-5 w-5" />
            Download from S3 Bucket
          </CardTitle>
          <CardDescription>
            Select a bucket and download content by CID
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex gap-3">
            <select
              className="flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
              value={selectedBucket?.s3BucketId.toString() || ""}
              onChange={(e) => {
                const bucket = buckets.find(
                  (b) => b.s3BucketId.toString() === e.target.value
                );
                setSelectedBucket(bucket || null);
              }}
            >
              <option value="">Select a bucket...</option>
              {buckets.map((bucket) => (
                <option key={bucket.s3BucketId.toString()} value={bucket.s3BucketId.toString()}>
                  {bucket.name}
                </option>
              ))}
            </select>
            <Button
              onClick={handleDownloadFromBucket}
              disabled={downloadLoading || !selectedBucket || !cidInput}
            >
              {downloadLoading ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <DownloadIcon className="mr-2 h-4 w-4" />
              )}
              Download
            </Button>
          </div>
          {buckets.length === 0 && (
            <p className="mt-2 text-xs text-muted-foreground">
              No buckets found. Create one in the Buckets page.
            </p>
          )}
        </CardContent>
      </Card>

      {/* Result */}
      {(result || error) && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              {result ? (
                <CheckCircle className="h-5 w-5 text-green-500" />
              ) : (
                <AlertCircle className="h-5 w-5 text-destructive" />
              )}
              Result
            </CardTitle>
          </CardHeader>
          <CardContent>
            {error ? (
              <p className="text-destructive">{error}</p>
            ) : result ? (
              <div className="space-y-4">
                <div className="rounded-lg border p-4">
                  <div className="grid gap-4 md:grid-cols-3">
                    <div>
                      <p className="text-sm text-muted-foreground">CID</p>
                      <div className="flex items-center gap-2">
                        <p className="font-mono text-sm">
                          {truncateHash(result.cid, 10, 8)}
                        </p>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-6 w-6"
                          onClick={() => copyToClipboard(result.cid)}
                        >
                          <Copy className="h-3 w-3" />
                        </Button>
                      </div>
                    </div>
                    <div>
                      <p className="text-sm text-muted-foreground">Size</p>
                      <p className="font-medium">{formatBytes(result.size)}</p>
                    </div>
                    <div>
                      <p className="text-sm text-muted-foreground">Content Type</p>
                      <p className="font-medium">{result.contentType}</p>
                    </div>
                  </div>
                </div>

                {/* Preview for text files */}
                {result.size < 10000 && (
                  <div className="rounded-lg border p-4">
                    <p className="text-sm text-muted-foreground mb-2">Preview</p>
                    <pre className="text-sm bg-muted p-2 rounded overflow-auto max-h-48">
                      {new TextDecoder().decode(result.data)}
                    </pre>
                  </div>
                )}

                <Button onClick={saveToDevice}>
                  <DownloadIcon className="mr-2 h-4 w-4" />
                  Save to Device
                </Button>
              </div>
            ) : null}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
