import { Toaster } from "@/components/ui/toaster";

export default function App() {
  return (
    <>
      <div className="flex min-h-screen items-center justify-center bg-background text-foreground">
        <p className="text-muted-foreground">S3 UI</p>
      </div>
      <Toaster />
    </>
  );
}
