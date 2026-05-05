import { Routes, Route } from "react-router-dom";
import { Toaster } from "@/components/ui/toaster";
import Layout from "@/components/Layout";
import DrivePage from "@/pages/DrivePage";

function App() {
  return (
    <>
      <Routes>
        <Route path="/" element={<Layout />}>
          <Route index element={<DrivePage />} />
        </Route>
      </Routes>
      <Toaster />
    </>
  );
}

export default App;
