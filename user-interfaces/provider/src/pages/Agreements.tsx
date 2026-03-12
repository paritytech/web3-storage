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
import { useAgreements } from '@/state/provider.state'
import { RequireProvider } from '@/components/RequireProvider'
import { formatAddress, formatBytes, formatTokens, formatBlockNumber } from '@/utils/format'

export function Agreements() {
  return (
    <RequireProvider icon={FileText} pageName="agreements">
      <AgreementsContent />
    </RequireProvider>
  )
}

function AgreementsContent() {
  const agreements = useAgreements()
  const activeAgreements = agreements.filter((a) => a.status === 'active')
  const expiredAgreements = agreements.filter((a) => a.status !== 'active')

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold">Storage Agreements</h1>
        <p className="text-gray-400">Manage your active and historical storage agreements</p>
      </div>

      {/* Stats */}
      <div className="grid gap-4 md:grid-cols-3">
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
