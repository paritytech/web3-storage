import { Routes, Route } from "react-router-dom";
import { Toaster } from "@/components/ui/toaster";
import { ChainProvider } from "@/hooks/useChain";
import { DriveProvider } from "@/hooks/useDrive";
import Layout from "@/components/Layout";
import DrivePage from "@/pages/DrivePage";

function App() {
  return (
    <ChainProvider>
      <DriveProvider>
        <Routes>
          <Route path="/" element={<Layout />}>
            <Route index element={<DrivePage />} />
          </Route>
        </Routes>
        <Toaster />
      </DriveProvider>
    </ChainProvider>
  );
}

export default App;
