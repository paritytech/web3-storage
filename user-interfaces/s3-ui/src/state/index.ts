// SPDX-License-Identifier: Apache-2.0

// Chain
export {
  useConnectionStatus,
  useIsConnected,
  useIsConnecting,
  useBlockNumber,
  useEndpoint,
  useConnectionError,
  connect,
  disconnect,
  reconnect,
  getClient,
  getApi,
  getEndpoint,
  isConnected,
  type ParachainApi,
} from "./chain.state";

// Network
export {
  useSelectedNetworkId,
  useSelectedNetwork,
  useParachainWs,
  useProviderHttp,
  useNetworkList,
  selectNetwork,
  selectCustomNetwork,
  getSelectedNetwork,
  getParachainWs,
  getProviderHttp,
} from "./network.state";

export type { NetworkId, NetworkConfig } from "@web3-storage/network-config";

// Wallet
export {
  useSignerAddress,
  useSignerName,
  useBalance,
  useIsSettingSigner,
  useHasSigner,
  setSigner,
  selectDevAccount,
  clearSigner,
  refreshBalance,
  restoreSigner,
  getSignerAddress,
  DEV_ACCOUNTS,
} from "./wallet.state";

// S3
export {
  useBuckets,
  useSelectedBucket,
  useCurrentPrefix,
  useObjects,
  useS3Loading,
  useUploading,
  useS3Error,
  useViewMode,
  useCreations,
  useEncryptionKey,
  refreshBuckets,
  selectBucket,
  navigateToPrefix,
  navigateUp,
  refreshObjects,
  setViewMode,
  uploadFiles,
  uploadObject,
  abortUpload,
  downloadObject,
  deleteObject,
  createBucket,
  retryCreation,
  canRetryCreation,
  dismissCreation,
  deleteBucket,
  listAvailableProviders,
  queryMatchingProviders,
  fetchMembers,
  addMember,
  removeMember,
  setEncryptionKey,
  clearEncryptionKey,
  getS3Client,
  type CreationStatus,
  type CreateBucketInput,
  type ViewMode,
} from "./s3.state";

// Checkpoint
export {
  useCheckpointInfo,
  useCheckpointDuty,
  useCheckpointLoading,
  useCheckpointStatus,
  refreshCheckpoint,
  triggerCheckpoint,
  clearCheckpointState,
  type CheckpointStatus,
} from "./checkpoint.state";

// Challenge
export {
  useActiveChallenge,
  useChallengeStatus,
  useOpenChallenges,
  useOpenChallengesLoading,
  useChallengeHistory,
  useChallengeHistoryLoading,
  useShowOutcomeModal,
  submitChallenge,
  refreshOpenChallenges,
  refreshChallengeHistory,
  clearChallengeState,
  dismissChallengeResult,
  clearChallengeHistory,
  type ChallengeStatus,
  type ActiveChallenge,
  type ChallengeDefenseDetails,
  type ChallengeSlashDetails,
  type ChallengeHistoryEntry,
} from "./challenge.state";
