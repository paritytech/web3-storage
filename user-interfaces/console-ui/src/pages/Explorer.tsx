import { useState, useEffect, useCallback } from "react";
import {
  Search,
  Box,
  Hash,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  ChevronUp,
  RefreshCw,
  Zap,
  FileText,
  Database,
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
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useChain } from "@/hooks/useChain";
import { parachain } from "@polkadot-api/descriptors";
import { Binary, type HexString } from "polkadot-api";

// Pallets we highlight (storage-related)
const STORAGE_PALLETS = ["StorageProvider", "S3Registry"];

// Serialize PAPI values for JSON display (handles Binary, BigInt, Uint8Array)
function serializeArgs(obj: unknown): Record<string, unknown> {
  if (obj === null || obj === undefined) return {};
  if (typeof obj !== "object") return { value: obj };

  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (value && typeof value === "object" && "asHex" in value && typeof (value as any).asHex === "function") {
      result[key] = (value as any).asHex();
    } else if (typeof value === "bigint") {
      result[key] = value.toString();
    } else if (value instanceof Uint8Array) {
      result[key] = "0x" + Array.from(value).map((b) => b.toString(16).padStart(2, "0")).join("");
    } else if (Array.isArray(value)) {
      result[key] = value.map((v) =>
        v && typeof v === "object" && "asHex" in v ? (v as any).asHex() :
        typeof v === "bigint" ? v.toString() :
        v instanceof Uint8Array ? "0x" + Array.from(v).map((b) => b.toString(16).padStart(2, "0")).join("") :
        typeof v === "object" && v !== null ? serializeArgs(v) : v
      );
    } else if (typeof value === "object" && value !== null) {
      result[key] = serializeArgs(value);
    } else {
      result[key] = value;
    }
  }
  return result;
}

interface BlockInfo {
  number: number;
  hash: string;
  extrinsicsCount: number;
}

interface ExtrinsicInfo {
  index: number;
  pallet: string;
  call: string;
  args?: Record<string, unknown>;
  signer?: string;
}

interface EventInfo {
  index: number;
  extrinsicIndex?: number;
  pallet: string;
  name: string;
  data: Record<string, unknown>;
}

// SCALE compact encoding helpers for extrinsic parsing
function compactLen(bytes: Uint8Array, offset: number): number {
  const mode = bytes[offset]! & 0x03;
  if (mode === 0) return 1;
  if (mode === 1) return 2;
  if (mode === 2) return 4;
  return 1 + ((bytes[offset]! >> 2) + 4);
}

function extractCallData(hexExt: HexString): Uint8Array | null {
  try {
    const bytes = Binary.fromHex(hexExt);
    if (bytes.length < 3) return null;
    let offset = compactLen(bytes, 0);
    const preamble = bytes[offset]!;
    offset += 1;
    const version = preamble & 0x1f;

    if (version === 5) {
      const preambleType = (preamble >> 5) & 0x03;
      if (preambleType === 0) return bytes.slice(offset); // Bare
      if (preambleType === 2) {
        // General: compact(ext_len) ++ extension_bytes ++ call_data
        const extLenSize = compactLen(bytes, offset);
        const mode = bytes[offset]! & 0x03;
        let extLen = 0;
        if (mode === 0) extLen = bytes[offset]! >> 2;
        else if (mode === 1) extLen = ((bytes[offset + 1]! << 8) | bytes[offset]!) >> 2;
        else if (mode === 2) {
          extLen = ((bytes[offset + 3]! << 24) | (bytes[offset + 2]! << 16) | (bytes[offset + 1]! << 8) | bytes[offset]!) >>> 2;
        }
        offset += extLenSize + extLen;
        return bytes.slice(offset);
      }
      return null;
    }
    if (version === 4) {
      if ((preamble & 0x80) === 0) return bytes.slice(offset); // Unsigned
      return null;
    }
    return null;
  } catch {
    return null;
  }
}

function getSignedExtOffsetRange(hexExt: HexString): { bytes: Uint8Array; minOffset: number } | null {
  try {
    const bytes = Binary.fromHex(hexExt);
    if (bytes.length < 3) return null;
    let offset = compactLen(bytes, 0);
    const preamble = bytes[offset]!;
    offset += 1;
    const version = preamble & 0x1f;
    const isSigned = version === 4 ? (preamble & 0x80) !== 0 : ((preamble >> 5) & 0x03) !== 0;
    if (!isSigned) return null;
    const addrType = bytes[offset]!;
    offset += 1;
    if (addrType === 0) offset += 32; else return null;
    const sigType = bytes[offset]!;
    offset += 1 + (sigType === 2 ? 65 : 64);
    return { bytes, minOffset: offset };
  } catch {
    return null;
  }
}

export default function Explorer() {
  const { client, connected, blockNumber: currentBlockNumber } = useChain();

  const [selectedBlockNumber, setSelectedBlockNumber] = useState<number | null>(null);
  const [blockSearchInput, setBlockSearchInput] = useState("");
  const [recentBlocks, setRecentBlocks] = useState<BlockInfo[]>([]);
  const [isLoadingBlocks, setIsLoadingBlocks] = useState(false);
  const [selectedBlock, setSelectedBlock] = useState<BlockInfo | null>(null);
  const [blockExtrinsics, setBlockExtrinsics] = useState<ExtrinsicInfo[]>([]);
  const [blockEvents, setBlockEvents] = useState<EventInfo[]>([]);
  const [isLoadingBlock, setIsLoadingBlock] = useState(false);
  const [expandedExtrinsic, setExpandedExtrinsic] = useState<number | null>(null);
  const [detailsTab, setDetailsTab] = useState<"extrinsics" | "events">("extrinsics");

  const api = client ? client.getTypedApi(parachain) : null;

  // Load recent blocks
  const loadRecentBlocks = useCallback(async () => {
    if (!client) return;
    setIsLoadingBlocks(true);
    try {
      const bestBlocks = await client.getBestBlocks();
      const blocks: BlockInfo[] = await Promise.all(
        bestBlocks.map(async (b) => {
          try {
            const body = await client.getBlockBody(b.hash);
            return { number: b.number, hash: b.hash, extrinsicsCount: body.length };
          } catch {
            return { number: b.number, hash: b.hash, extrinsicsCount: 0 };
          }
        })
      );
      setRecentBlocks(blocks);
    } catch (err) {
      console.error("Failed to load recent blocks:", err);
    } finally {
      setIsLoadingBlocks(false);
    }
  }, [client]);

  // Load block details
  const loadBlockDetails = useCallback(async (blockNumber: number, knownHash?: string) => {
    if (!client) return;
    setIsLoadingBlock(true);
    setSelectedBlockNumber(blockNumber);
    setBlockExtrinsics([]);
    setBlockEvents([]);
    setExpandedExtrinsic(null);

    try {
      let hashHex = knownHash
        ?? recentBlocks.find((b) => b.number === blockNumber)?.hash
        ?? "";

      // Fall back to System.BlockHash
      if (!hashHex && api) {
        try {
          const blockHash = await api.query.System.BlockHash.getValue(blockNumber);
          if (blockHash) {
            const hex = typeof blockHash === "string" ? blockHash
              : typeof (blockHash as any).asHex === "function" ? (blockHash as any).asHex()
              : "";
            if (hex && !hex.match(/^0x0+$/)) hashHex = hex;
          }
        } catch { /* block not in storage */ }
      }

      let body: Array<string | Uint8Array> = [];
      try {
        if (hashHex) body = await client.getBlockBody(hashHex);
      } catch { /* unpinned block */ }

      setSelectedBlock({ number: blockNumber, hash: hashHex || "", extrinsicsCount: body.length });

      // Decode extrinsics
      const extrinsics: ExtrinsicInfo[] = await Promise.all(
        body.map(async (raw, index) => {
          if (!api) return { index, pallet: "", call: "" };
          // Normalize to hex string
          const hex: HexString = raw instanceof Uint8Array
            ? ("0x" + Array.from(raw).map((b: number) => b.toString(16).padStart(2, "0")).join("")) as HexString
            : raw as HexString;
          try {
            const callData = extractCallData(hex);
            if (callData) {
              try {
                const tx = await api.txFromCallData(callData);
                const pallet = tx.decodedCall.type;
                const callValue = tx.decodedCall.value as { type: string; value?: unknown };
                return { index, pallet, call: callValue.type, args: callValue.value ? serializeArgs(callValue.value) : undefined };
              } catch (e) {
                console.warn(`[Explorer] txFromCallData failed for ext #${index} (${callData.length} bytes):`, e);
              }
            }
            // Signed extrinsic fallback
            const range = getSignedExtOffsetRange(hex);
            if (range) {
              for (let i = range.bytes.length - 2; i >= range.minOffset; i--) {
                try {
                  const slice = range.bytes.slice(i);
                  const tx = await api.txFromCallData(slice);
                  const pallet = tx.decodedCall.type;
                  const callValue = tx.decodedCall.value as { type: string; value?: unknown };
                  let signer: string | undefined;
                  try {
                    const extBytes = Binary.fromHex(hex);
                    let off = compactLen(extBytes, 0) + 1;
                    if (extBytes[off] === 0) {
                      signer = "0x" + Array.from(extBytes.slice(off + 1, off + 33)).map((b: number) => b.toString(16).padStart(2, "0")).join("");
                    }
                  } catch { /* ignore */ }
                  return { index, pallet, call: callValue.type, args: callValue.value ? serializeArgs(callValue.value) : undefined, signer };
                } catch { /* try next offset */ }
              }
            }
          } catch { /* decoding failed */ }
          return { index, pallet: "", call: "" };
        })
      );
      setBlockExtrinsics(extrinsics);

      // Load events
      if (api && hashHex) {
        try {
          const rawEvents = await api.query.System.Events.getValue({ at: hashHex });
          // Unwrap PAPI { block, value } wrapper if present
          const eventsArray = Array.isArray(rawEvents) ? rawEvents : (rawEvents as any)?.value ?? [];

          const events: EventInfo[] = (eventsArray || []).map((event: any, idx: number) => {
            const pallet = event.event?.type || "Unknown";
            const eventValue = event.event?.value || {};
            const name = eventValue.type || "Unknown";
            const data = eventValue.value || {};
            const phase = event.phase;
            const extrinsicIndex = phase?.type === "ApplyExtrinsic" ? phase.value : undefined;
            return { index: idx, extrinsicIndex, pallet, name, data: serializeArgs(data) };
          });
          setBlockEvents(events);
        } catch {
          setBlockEvents([]);
        }
      }
    } catch (err) {
      console.error("Failed to load block details:", err);
      setSelectedBlock({ number: blockNumber, hash: "", extrinsicsCount: 0 });
    } finally {
      setIsLoadingBlock(false);
    }
  }, [api, client, recentBlocks]);

  // Refresh recent blocks on new block
  useEffect(() => {
    if (connected && currentBlockNumber > 0) loadRecentBlocks();
  }, [connected, currentBlockNumber, loadRecentBlocks]);

  // Auto-select first block
  useEffect(() => {
    if (recentBlocks.length > 0 && selectedBlockNumber === null) {
      loadBlockDetails(recentBlocks[0]!.number, recentBlocks[0]!.hash);
    }
  }, [recentBlocks, selectedBlockNumber, loadBlockDetails]);

  const handleBlockSearch = () => {
    const num = parseInt(blockSearchInput);
    if (!isNaN(num) && num >= 0) loadBlockDetails(num);
  };

  const handlePrevBlock = () => {
    if (selectedBlockNumber !== null && selectedBlockNumber > 0) loadBlockDetails(selectedBlockNumber - 1);
  };

  const handleNextBlock = () => {
    if (selectedBlockNumber !== null && selectedBlockNumber < currentBlockNumber) loadBlockDetails(selectedBlockNumber + 1);
  };

  // Highlight storage-related extrinsics/events
  const storageEvents = blockEvents.filter((e) => STORAGE_PALLETS.includes(e.pallet));
  const storageExtrinsics = blockExtrinsics.filter((e) => STORAGE_PALLETS.includes(e.pallet));

  if (!connected) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Block Explorer</h1>
          <p className="text-muted-foreground">Browse blocks, extrinsics, and events</p>
        </div>
        <Card>
          <CardContent className="py-8 text-center text-muted-foreground">
            <Box className="mx-auto h-12 w-12 mb-4 opacity-50" />
            <p>Connect to the network to browse blocks</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold tracking-tight">Block Explorer</h1>
        <p className="text-muted-foreground">Browse blocks, extrinsics, and events</p>
      </div>

      <div className="grid gap-6 lg:grid-cols-3">
        {/* Left Column: Search + Recent Blocks */}
        <div className="space-y-4">
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-lg">
                <Search className="h-5 w-5" />
                Search Block
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex gap-2">
                <Input
                  type="number"
                  placeholder="Block number"
                  value={blockSearchInput}
                  onChange={(e) => setBlockSearchInput(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleBlockSearch()}
                  min={0}
                />
                <Button onClick={handleBlockSearch} disabled={!blockSearchInput}>
                  Go
                </Button>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-3">
              <div className="flex items-center justify-between">
                <CardTitle className="flex items-center gap-2 text-lg">
                  <Box className="h-5 w-5" />
                  Recent Blocks
                </CardTitle>
                <Button variant="ghost" size="sm" onClick={loadRecentBlocks} disabled={isLoadingBlocks}>
                  <RefreshCw className={`h-4 w-4 ${isLoadingBlocks ? "animate-spin" : ""}`} />
                </Button>
              </div>
            </CardHeader>
            <CardContent>
              {recentBlocks.length === 0 ? (
                <p className="text-center text-muted-foreground py-8">
                  {isLoadingBlocks ? "Loading..." : "No blocks loaded"}
                </p>
              ) : (
                <div className="space-y-1">
                  {recentBlocks.map((block) => (
                    <button
                      key={block.number}
                      onClick={() => loadBlockDetails(block.number, block.hash)}
                      className={`w-full text-left p-3 rounded-md transition-colors ${
                        selectedBlockNumber === block.number
                          ? "bg-primary/10 border border-primary/20"
                          : "hover:bg-secondary"
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span className="font-mono font-medium">
                          #{block.number.toLocaleString()}
                        </span>
                        <Badge variant="secondary" className="text-xs">
                          {block.extrinsicsCount} txs
                        </Badge>
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* Right Column: Block Details */}
        <div className="lg:col-span-2">
          {isLoadingBlock ? (
            <Card>
              <CardContent className="flex justify-center py-16">
                <RefreshCw className="h-8 w-8 animate-spin text-muted-foreground" />
              </CardContent>
            </Card>
          ) : selectedBlock ? (
            <div className="space-y-4">
              {/* Block Header */}
              <Card>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div>
                      <CardTitle className="flex items-center gap-2">
                        <Hash className="h-5 w-5" />
                        Block #{selectedBlock.number.toLocaleString()}
                      </CardTitle>
                      <CardDescription className="font-mono text-xs mt-1 break-all">
                        {selectedBlock.hash || "Hash not available"}
                      </CardDescription>
                    </div>
                    <div className="flex gap-1">
                      <Button variant="outline" size="sm" onClick={handlePrevBlock} disabled={selectedBlockNumber === 0}>
                        <ChevronLeft className="h-4 w-4" />
                      </Button>
                      <Button variant="outline" size="sm" onClick={handleNextBlock} disabled={selectedBlockNumber === currentBlockNumber}>
                        <ChevronRight className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                </CardHeader>
                <CardContent>
                  <div className="grid grid-cols-3 gap-4">
                    <div className="space-y-1">
                      <p className="text-xs text-muted-foreground uppercase tracking-wide">Extrinsics</p>
                      <p className="font-mono text-lg font-medium">{blockExtrinsics.length}</p>
                    </div>
                    <div className="space-y-1">
                      <p className="text-xs text-muted-foreground uppercase tracking-wide">Events</p>
                      <p className="font-mono text-lg font-medium">{blockEvents.length}</p>
                    </div>
                    <div className="space-y-1">
                      <p className="text-xs text-muted-foreground uppercase tracking-wide">Storage Txs</p>
                      <p className="font-mono text-lg font-medium text-primary">{storageExtrinsics.length}</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Extrinsics & Events Tabs */}
              <Card>
                <Tabs value={detailsTab} onValueChange={(v) => setDetailsTab(v as "extrinsics" | "events")}>
                  <CardHeader className="pb-0">
                    <TabsList className="grid w-full grid-cols-2">
                      <TabsTrigger value="extrinsics" className="flex items-center gap-2">
                        <FileText className="h-4 w-4" />
                        Extrinsics ({blockExtrinsics.length})
                      </TabsTrigger>
                      <TabsTrigger value="events" className="flex items-center gap-2">
                        <Zap className="h-4 w-4" />
                        Events ({blockEvents.length})
                      </TabsTrigger>
                    </TabsList>
                  </CardHeader>

                  <CardContent className="pt-4">
                    <TabsContent value="extrinsics" className="mt-0">
                      {blockExtrinsics.length === 0 ? (
                        <p className="text-center text-muted-foreground py-4">No extrinsics in this block</p>
                      ) : (
                        <div className="space-y-2">
                          {blockExtrinsics.map((ext) => {
                            const isStorage = STORAGE_PALLETS.includes(ext.pallet);
                            const isExpanded = expandedExtrinsic === ext.index;
                            const extEvents = blockEvents.filter((e) => e.extrinsicIndex === ext.index);

                            return (
                              <div key={ext.index} className={`rounded-md border ${isStorage ? "border-primary/30 bg-primary/5" : "bg-secondary/50"}`}>
                                <button
                                  onClick={() => setExpandedExtrinsic(isExpanded ? null : ext.index)}
                                  className="w-full p-3 text-left flex items-center justify-between"
                                >
                                  <div className="flex items-center gap-2">
                                    <Badge variant={isStorage ? "default" : "outline"}>#{ext.index}</Badge>
                                    {isStorage && <Database className="h-4 w-4 text-primary" />}
                                    <span className="font-mono text-sm">
                                      {ext.pallet && ext.call ? `${ext.pallet}.${ext.call}` : "Unknown"}
                                    </span>
                                  </div>
                                  <div className="flex items-center gap-2">
                                    {extEvents.length > 0 && (
                                      <Badge variant="secondary" className="text-xs">{extEvents.length} events</Badge>
                                    )}
                                    {isExpanded ? <ChevronUp className="h-4 w-4 text-muted-foreground" /> : <ChevronDown className="h-4 w-4 text-muted-foreground" />}
                                  </div>
                                </button>

                                {isExpanded && (
                                  <div className="px-3 pb-3 space-y-3">
                                    {ext.signer && (
                                      <div>
                                        <p className="text-xs text-muted-foreground mb-1">Signer</p>
                                        <p className="font-mono text-xs break-all bg-background/50 p-2 rounded">{ext.signer}</p>
                                      </div>
                                    )}
                                    {ext.args && Object.keys(ext.args).length > 0 && (
                                      <div>
                                        <p className="text-xs text-muted-foreground mb-1">Arguments</p>
                                        <pre className="text-xs bg-background/50 p-2 rounded overflow-x-auto">
                                          {JSON.stringify(ext.args, null, 2)}
                                        </pre>
                                      </div>
                                    )}
                                    {extEvents.length > 0 && (
                                      <div>
                                        <p className="text-xs text-muted-foreground mb-1">Events</p>
                                        <div className="space-y-1">
                                          {extEvents.map((event) => (
                                            <div key={event.index} className={`text-xs p-2 rounded ${STORAGE_PALLETS.includes(event.pallet) ? "bg-primary/10 border border-primary/20" : "bg-background/50"}`}>
                                              <span className="font-mono font-medium">{event.pallet}.{event.name}</span>
                                              {Object.keys(event.data).length > 0 && (
                                                <pre className="mt-1 text-muted-foreground overflow-x-auto">{JSON.stringify(event.data, null, 2)}</pre>
                                              )}
                                            </div>
                                          ))}
                                        </div>
                                      </div>
                                    )}
                                  </div>
                                )}
                              </div>
                            );
                          })}
                        </div>
                      )}
                    </TabsContent>

                    <TabsContent value="events" className="mt-0">
                      {blockEvents.length === 0 ? (
                        <p className="text-center text-muted-foreground py-4">No events in this block</p>
                      ) : (
                        <div className="space-y-2">
                          {storageEvents.length > 0 && (
                            <div className="mb-4">
                              <p className="text-sm font-medium mb-2 flex items-center gap-2">
                                <Database className="h-4 w-4 text-primary" />
                                Storage Events ({storageEvents.length})
                              </p>
                              <div className="space-y-2">
                                {storageEvents.map((event) => (
                                  <div key={event.index} className="p-3 rounded-md border border-primary/30 bg-primary/5">
                                    <div className="flex items-center justify-between mb-2">
                                      <span className="font-mono text-sm font-medium">{event.pallet}.{event.name}</span>
                                      {event.extrinsicIndex !== undefined && (
                                        <Badge variant="outline" className="text-xs">ext #{event.extrinsicIndex}</Badge>
                                      )}
                                    </div>
                                    {Object.keys(event.data).length > 0 && (
                                      <pre className="text-xs bg-background/50 p-2 rounded overflow-x-auto">{JSON.stringify(event.data, null, 2)}</pre>
                                    )}
                                  </div>
                                ))}
                              </div>
                            </div>
                          )}

                          <div>
                            {storageEvents.length > 0 && (
                              <p className="text-sm font-medium mb-2">Other Events ({blockEvents.length - storageEvents.length})</p>
                            )}
                            <div className="space-y-2">
                              {blockEvents.filter((e) => !STORAGE_PALLETS.includes(e.pallet)).map((event) => (
                                <div key={event.index} className="p-3 rounded-md bg-secondary/50 border">
                                  <div className="flex items-center justify-between mb-2">
                                    <span className="font-mono text-sm">{event.pallet}.{event.name}</span>
                                    {event.extrinsicIndex !== undefined && (
                                      <Badge variant="outline" className="text-xs">ext #{event.extrinsicIndex}</Badge>
                                    )}
                                  </div>
                                  {Object.keys(event.data).length > 0 && (
                                    <pre className="text-xs bg-background/50 p-2 rounded overflow-x-auto max-h-32 overflow-y-auto">{JSON.stringify(event.data, null, 2)}</pre>
                                  )}
                                </div>
                              ))}
                            </div>
                          </div>
                        </div>
                      )}
                    </TabsContent>
                  </CardContent>
                </Tabs>
              </Card>
            </div>
          ) : (
            <Card>
              <CardContent className="flex flex-col items-center justify-center py-16 text-muted-foreground">
                <Box className="h-12 w-12 mb-4" />
                <p>Select a block to view details</p>
              </CardContent>
            </Card>
          )}
        </div>
      </div>
    </div>
  );
}
