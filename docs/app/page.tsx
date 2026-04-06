import Link from "next/link";

export default function HomePage() {
  return (
    <main className="flex flex-1 flex-col items-center justify-center text-center px-4 py-16 min-h-screen bg-fd-background">
      <div className="max-w-3xl mx-auto space-y-8">
        <div className="space-y-4">
          <p className="text-sm font-mono tracking-wider text-fd-muted-foreground uppercase">
            UNH InterOperability Laboratory
          </p>
          <h1 className="text-5xl font-extrabold tracking-tight">
            sonic-mgmt-rs
          </h1>
          <p className="text-xl text-fd-muted-foreground max-w-2xl mx-auto">
            Rust framework for managing SONiC network switches, testbeds,
            topologies, and automated testing.
          </p>
        </div>

        <div className="grid gap-4 sm:grid-cols-2 max-w-xl mx-auto">
          <Link
            href="/docs"
            className="inline-flex items-center justify-center rounded-lg bg-fd-primary px-6 py-3 text-sm font-medium text-fd-primary-foreground shadow transition-colors hover:bg-fd-primary/90"
          >
            Crate Documentation
          </Link>
          <Link
            href="/api/sonic_core/index.html"
            className="inline-flex items-center justify-center rounded-lg border border-fd-border px-6 py-3 text-sm font-medium text-fd-foreground shadow-sm transition-colors hover:bg-fd-accent hover:text-fd-accent-foreground"
          >
            API Reference
          </Link>
        </div>
      </div>
    </main>
  );
}
