import { Routes, Route } from "react-router-dom";
import { Toaster } from "@/components/ui/toaster";
import { ChainProvider } from "@/hooks/useChain";
import { StorageProvider } from "@/hooks/useStorage";
import Layout from "@/components/Layout";
import Dashboard from "@/pages/Dashboard";
import Drives from "@/pages/Drives";
import Buckets from "@/pages/Buckets";
import Upload from "@/pages/Upload";
import Download from "@/pages/Download";
import Explorer from "@/pages/Explorer";
import Accounts from "@/pages/Accounts";

function App() {
  return (
    <ChainProvider>
      <StorageProvider>
        <Routes>
          <Route path="/" element={<Layout />}>
            <Route index element={<Dashboard />} />
            <Route path="drives" element={<Drives />} />
            <Route path="buckets" element={<Buckets />} />
            <Route path="upload" element={<Upload />} />
            <Route path="download" element={<Download />} />
            <Route path="explorer" element={<Explorer />} />
            <Route path="accounts" element={<Accounts />} />
          </Route>
        </Routes>
        <Toaster />
      </StorageProvider>
    </ChainProvider>
  );
}

export default App;
