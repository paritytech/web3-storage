import { useEffect } from 'react'
import { FileText } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/Table'
import { useAgreements, useAgreementRequests, useIsRegistered, loadProviderData } from '@/state/provider.state'
import { useSelectedAccount } from '@/state/wallet.state'
import { useBlockNumber } from '@/state/chain.state'
import { formatAddress, formatBytes, formatTokens, formatBlockNumber } from '@/utils/format'

export function Agreements() {
  const selectedAccount = useSelectedAccount()
  const isRegistered = useIsRegistered()
  const agreements = useAgreements()
  const agreementRequests = useAgreementRequests()
  const currentBlock = useBlockNumber()

  useEffect(() => {
    if (selectedAccount?.address) {
      loadProviderData(selectedAccount.address)
    }
  }, [selectedAccount?.address])

  if (!selectedAccount) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <FileText className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Connect Your Wallet</h2>
        <p className="text-gray-500">Connect a wallet to view your agreements</p>
      </div>
    )
  }

  if (!isRegistered) {
    return (
      <div className="flex flex-col items-center justify-center py-16">
        <FileText className="h-16 w-16 text-gray-600 mb-4" />
        <h2 className="text-xl font-semibold text-gray-300 mb-2">Not Registered</h2>
        <p className="text-gray-500">Register as a provider to view agreements</p>
      </div>
    )
  }

  const activeAgreements = agreements.filter((a) => a.status === 'active')
  const expiredAgreements = agreements.filter((a) => a.status !== 'active')

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Storage Agreements</h1>
        <p className="text-gray-400">Manage your active and historical storage agreements</p>
      </div>

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Active</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{activeAgreements.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Pending Requests</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">{agreementRequests.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Capacity</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {formatBytes(
                Number(activeAgreements.reduce((sum, a) => sum + a.maxBytes, 0n))
              )}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-gray-400">Total Value</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold">
              {formatTokens(
                activeAgreements.reduce(
                  (sum, a) => sum + a.pricePerByte * a.maxBytes * BigInt(a.endBlock - a.startBlock),
                  0n
                )
              )}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Pending Requests */}
      <Card>
        <CardHeader>
          <CardTitle>Pending Requests</CardTitle>
          <CardDescription>Agreement requests awaiting acceptance</CardDescription>
        </CardHeader>
        <CardContent>
          {agreementRequests.length === 0 ? (
            <p className="text-gray-500 text-center py-8">No pending requests</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Bucket</TableHead>
                  <TableHead>Requester</TableHead>
                  <TableHead>Size</TableHead>
                  <TableHead>Payment</TableHead>
                  <TableHead>Duration</TableHead>
                  <TableHead>Expires</TableHead>
                  <TableHead>Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {agreementRequests.map((req) => {
                  const isExpired = currentBlock > 0 && req.expiresAt > 0 && currentBlock > req.expiresAt
                  return (
                    <TableRow key={`req-${req.bucketId}`}>
                      <TableCell className="font-mono">#{req.bucketId}</TableCell>
                      <TableCell>
                        <span className="font-mono">{formatAddress(req.requester)}</span>
                      </TableCell>
                      <TableCell>{formatBytes(Number(req.maxBytes))}</TableCell>
                      <TableCell>{formatTokens(req.paymentLocked)}</TableCell>
                      <TableCell>{formatBlockNumber(req.duration)} blocks</TableCell>
                      <TableCell>#{formatBlockNumber(req.expiresAt)}</TableCell>
                      <TableCell>
                        {isExpired ? (
                          <Badge variant="destructive">Expired</Badge>
                        ) : (
                          <Badge variant="warning">Pending</Badge>
                        )}
                      </TableCell>
                    </TableRow>
                  )
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* Active Agreements */}
      <Card>
        <CardHeader>
          <CardTitle>Active Agreements</CardTitle>
          <CardDescription>Current storage commitments</CardDescription>
        </CardHeader>
        <CardContent>
          {activeAgreements.length === 0 ? (
            <p className="text-gray-500 text-center py-8">No active agreements</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Bucket</TableHead>
                  <TableHead>User</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Size</TableHead>
                  <TableHead>Price</TableHead>
                  <TableHead>Duration</TableHead>
                  <TableHead>Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {activeAgreements.map((agreement) => (
                  <TableRow key={agreement.id}>
                    <TableCell className="font-mono">#{agreement.bucketId}</TableCell>
                    <TableCell>
                      <span className="font-mono">{formatAddress(agreement.user)}</span>
                    </TableCell>
                    <TableCell>
                      <Badge variant={agreement.isPrimary ? 'default' : 'secondary'}>
                        {agreement.isPrimary ? 'Primary' : 'Replica'}
                      </Badge>
                    </TableCell>
                    <TableCell>{formatBytes(Number(agreement.maxBytes))}</TableCell>
                    <TableCell>{formatTokens(agreement.pricePerByte)}/byte</TableCell>
                    <TableCell>
                      {formatBlockNumber(agreement.startBlock)} -{' '}
                      {formatBlockNumber(agreement.endBlock)}
                    </TableCell>
                    <TableCell>
                      <Badge variant="success">Active</Badge>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* Historical Agreements */}
      {expiredAgreements.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Historical Agreements</CardTitle>
            <CardDescription>Past and expired agreements</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Bucket</TableHead>
                  <TableHead>User</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Size</TableHead>
                  <TableHead>Duration</TableHead>
                  <TableHead>Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {expiredAgreements.map((agreement) => (
                  <TableRow key={agreement.id}>
                    <TableCell className="font-mono">#{agreement.bucketId}</TableCell>
                    <TableCell>
                      <span className="font-mono">{formatAddress(agreement.user)}</span>
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">
                        {agreement.isPrimary ? 'Primary' : 'Replica'}
                      </Badge>
                    </TableCell>
                    <TableCell>{formatBytes(Number(agreement.maxBytes))}</TableCell>
                    <TableCell>
                      {formatBlockNumber(agreement.startBlock)} -{' '}
                      {formatBlockNumber(agreement.endBlock)}
                    </TableCell>
                    <TableCell>
                      <Badge
                        variant={agreement.status === 'expired' ? 'secondary' : 'destructive'}
                      >
                        {agreement.status === 'expired' ? 'Expired' : 'Terminated'}
                      </Badge>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
