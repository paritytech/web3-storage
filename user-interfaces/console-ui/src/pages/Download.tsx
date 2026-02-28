import { useState } from "react";
import {
  Download as DownloadIcon,
  Search,
  File,
  Loader2,
  CheckCircle,
  AlertCircle,
  Copy,
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
import { toast } from "@/components/ui/toaster";
import { formatBytes, truncateHash } from "@/lib/utils";

type DownloadSource = "cid" | "path";

interface DownloadResult {
  cid: string;
  size: number;
  contentType: string;
  data?: Uint8Array;
}

export default function Download() {
  const { connected } = useChain();
  const [downloadSource, setDownloadSource] = useState<DownloadSource>("cid");
  const [cidInput, setCidInput] = useState("");
  const [driveName, setDriveName] = useState("");
  const [filePath, setFilePath] = useState("");
  const [bucketName, setBucketName] = useState("");
  const [objectKey, setObjectKey] = useState("");
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<DownloadResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleDownloadByCid = async () => {
    if (!cidInput.trim()) {
      toast({
        title: "Error",
        description: "Please enter a CID",
        variant: "destructive",
      });
      return;
    }

    setLoading(true);
    setError(null);
    setResult(null);

    try {
      // TODO: Call SDK to download by CID
      await new Promise((r) => setTimeout(r, 1000));

      // Mock result
      setResult({
        cid: cidInput,
        size: 1024 * Math.floor(Math.random() * 100 + 1),
        contentType: "application/octet-stream",
      });

      toast({ title: "Success", description: "File retrieved successfully" });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Download failed");
      toast({
        title: "Error",
        description: "Failed to download file",
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  };

  const handleDownloadByPath = async () => {
    if (!driveName.trim() || !filePath.trim()) {
      toast({
        title: "Error",
        description: "Please enter drive name and file path",
        variant: "destructive",
      });
      return;
    }

    setLoading(true);
    setError(null);
    setResult(null);

    try {
      // TODO: Call SDK to download from drive
      await new Promise((r) => setTimeout(r, 1000));

      const mockCid = `0x${Array.from({ length: 64 }, () =>
        Math.floor(Math.random() * 16).toString(16)
      ).join("")}`;

      setResult({
        cid: mockCid,
        size: 1024 * Math.floor(Math.random() * 100 + 1),
        contentType: "text/plain",
      });

      toast({ title: "Success", description: "File retrieved successfully" });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Download failed");
      toast({
        title: "Error",
        description: "Failed to download file",
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  };

  const handleDownloadFromBucket = async () => {
    if (!bucketName.trim() || !objectKey.trim()) {
      toast({
        title: "Error",
        description: "Please enter bucket name and object key",
        variant: "destructive",
      });
      return;
    }

    setLoading(true);
    setError(null);
    setResult(null);

    try {
      // TODO: Call SDK to download from S3 bucket
      await new Promise((r) => setTimeout(r, 1000));

      const mockCid = `0x${Array.from({ length: 64 }, () =>
        Math.floor(Math.random() * 16).toString(16)
      ).join("")}`;

      setResult({
        cid: mockCid,
        size: 1024 * Math.floor(Math.random() * 100 + 1),
        contentType: "application/json",
      });

      toast({ title: "Success", description: "Object retrieved successfully" });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Download failed");
      toast({
        title: "Error",
        description: "Failed to download object",
        variant: "destructive",
      });
    } finally {
      setLoading(false);
    }
  };

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
    toast({ title: "Copied", description: "CID copied to clipboard" });
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

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Download</h1>
        <p className="text-muted-foreground">Download files from storage</p>
      </div>

      {/* Download by CID */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Search className="h-5 w-5" />
            Download by CID
          </CardTitle>
          <CardDescription>
            Download a file directly using its content identifier (CID)
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex gap-3">
            <Input
              placeholder="0x..."
              value={cidInput}
              onChange={(e) => setCidInput(e.target.value)}
              className="font-mono"
            />
            <Button onClick={handleDownloadByCid} disabled={loading}>
              {loading ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <DownloadIcon className="mr-2 h-4 w-4" />
              )}
              Download
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Download from Drive */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <File className="h-5 w-5" />
            Download from Drive
          </CardTitle>
          <CardDescription>
            Download a file from a File System drive by path
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-2">
              <label className="text-sm font-medium">Drive Name</label>
              <Input
                placeholder="my-drive"
                value={driveName}
                onChange={(e) => setDriveName(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">File Path</label>
              <Input
                placeholder="/documents/file.txt"
                value={filePath}
                onChange={(e) => setFilePath(e.target.value)}
              />
            </div>
            <div className="flex items-end">
              <Button
                onClick={handleDownloadByPath}
                disabled={loading}
                className="w-full"
              >
                {loading ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <DownloadIcon className="mr-2 h-4 w-4" />
                )}
                Download
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Download from S3 Bucket */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <File className="h-5 w-5" />
            Download from S3 Bucket
          </CardTitle>
          <CardDescription>
            Download an object from an S3-compatible bucket
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-2">
              <label className="text-sm font-medium">Bucket Name</label>
              <Input
                placeholder="my-bucket"
                value={bucketName}
                onChange={(e) => setBucketName(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <label className="text-sm font-medium">Object Key</label>
              <Input
                placeholder="uploads/file.txt"
                value={objectKey}
                onChange={(e) => setObjectKey(e.target.value)}
              />
            </div>
            <div className="flex items-end">
              <Button
                onClick={handleDownloadFromBucket}
                disabled={loading}
                className="w-full"
              >
                {loading ? (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <DownloadIcon className="mr-2 h-4 w-4" />
                )}
                Download
              </Button>
            </div>
          </div>
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
                <Button>
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
