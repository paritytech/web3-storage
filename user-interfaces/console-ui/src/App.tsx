import { Routes, Route } from "react-router-dom";
import { Toaster } from "@/components/ui/toaster";
import { ChainProvider } from "@/hooks/useChain";
import { StorageProvider } from "@/hooks/useStorage";
import Layout from "@/components/Layout";
import Dashboard from "@/pages/Dashboard";
import Storage from "@/pages/Storage";
import Explorer from "@/pages/Explorer";
import Accounts from "@/pages/Accounts";

function App() {
  return (
    <ChainProvider>
      <StorageProvider>
        <Routes>
          <Route path="/" element={<Layout />}>
            <Route index element={<Dashboard />} />
            <Route path="storage" element={<Storage />} />
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
