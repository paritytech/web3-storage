// SPDX-License-Identifier: GPL-3.0-only

import { Route, Routes } from "react-router-dom";
import { Toaster } from "@/components/ui/toaster";
import Layout from "@/components/Layout";
import S3Page from "@/pages/S3Page";

export default function App() {
  return (
    <>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<S3Page />} />
        </Route>
      </Routes>
      <Toaster />
    </>
  );
}
