# Web3 Storage Console UI

A React-based web interface for managing Web3 Storage, providing both File System and S3-compatible storage interfaces.

## Features

- **Dashboard**: Overview of storage usage and network status
- **Drives**: Create and manage File System drives
- **S3 Buckets**: Create and manage S3-compatible buckets
- **Upload**: Upload files to drives or buckets
- **Download**: Download files by CID, path, or object key
- **Explorer**: Browse storage contents
- **Accounts**: Manage signing accounts

## Tech Stack

- **React 19** - UI framework
- **Vite 7** - Build tool
- **TypeScript** - Type safety
- **Tailwind CSS 4** - Styling
- **Radix UI** - Accessible components
- **polkadot-api** - Blockchain interaction
- **RxJS** - Reactive state management

## Getting Started

### Prerequisites

- Node.js 18+
- pnpm (recommended) or npm
- Running parachain and storage provider (see main project README)

### Installation

```bash
# Install dependencies
pnpm install

# Generate chain types (requires running chain)
pnpm papi:generate

# Start development server
pnpm dev
```

### Development

```bash
# Start dev server
pnpm dev

# Build for production
pnpm build

# Preview production build
pnpm preview

# Lint code
pnpm lint
```

## Project Structure

```
console-ui/
├── src/
│   ├── components/          # Reusable UI components
│   │   ├── ui/             # Base components (Button, Card, etc.)
│   │   ├── Layout.tsx      # App layout with navigation
│   │   └── ConnectDialog.tsx # Network connection dialog
│   ├── hooks/              # React hooks
│   │   └── useChain.tsx    # Chain connection state
│   ├── lib/                # Utilities
│   │   └── utils.ts        # Helper functions
│   ├── pages/              # Page components
│   │   ├── Dashboard.tsx
│   │   ├── Drives.tsx
│   │   ├── Buckets.tsx
│   │   ├── Upload.tsx
│   │   ├── Download.tsx
│   │   ├── Explorer.tsx
│   │   └── Accounts.tsx
│   ├── styles/             # Global styles
│   │   └── index.css       # Tailwind config
│   ├── App.tsx             # Root component
│   └── main.tsx            # Entry point
├── public/                 # Static assets
├── index.html              # HTML template
├── package.json
├── tsconfig.json
└── vite.config.ts
```

## Configuration

### Network Endpoints

By default, the UI connects to:
- Chain WebSocket: `ws://127.0.0.1:2222`
- Provider HTTP: `http://127.0.0.1:3333`

These can be configured via the Connect dialog in the UI.

### Theme

The UI uses a dark theme by default. Colors can be customized in `src/styles/index.css`.

## Integration with SDKs

This UI is designed to work with the TypeScript SDKs:
- `@web3-storage/file-system-sdk` - File System operations
- `@web3-storage/s3-sdk` - S3-compatible operations

See `../sdk/typescript/` for SDK documentation.

## License

MIT
