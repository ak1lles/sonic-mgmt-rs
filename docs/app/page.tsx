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
            A type-safe, high-performance Rust framework for managing SONiC
            network switches, testbeds, topologies, and automated testing.
          </p>
        </div>

        <div className="grid gap-4 sm:grid-cols-2 max-w-xl mx-auto">
          <Link
            href="/docs"
            className="inline-flex items-center justify-center rounded-lg bg-fd-primary px-6 py-3 text-sm font-medium text-fd-primary-foreground shadow transition-colors hover:bg-fd-primary/90"
          >
            Get Started
          </Link>
          <Link
            href="/docs/architecture"
            className="inline-flex items-center justify-center rounded-lg border border-fd-border px-6 py-3 text-sm font-medium text-fd-foreground shadow-sm transition-colors hover:bg-fd-accent hover:text-fd-accent-foreground"
          >
            Architecture
          </Link>
        </div>

        <div className="grid gap-6 sm:grid-cols-3 text-left pt-8">
          <div className="space-y-2 p-4 rounded-lg border border-fd-border">
            <h3 className="font-semibold">9 Modular Crates</h3>
            <p className="text-sm text-fd-muted-foreground">
              Clean separation of concerns across core, config, device, testbed,
              topology, testing, reporting, SDN, and CLI.
            </p>
          </div>
          <div className="space-y-2 p-4 rounded-lg border border-fd-border">
            <h3 className="font-semibold">13 Topology Types</h3>
            <p className="text-sm text-fd-muted-foreground">
              T0, T1, T2, Dualtor, PTF-only and more. Full topology generation
              with IP, VLAN, and VM allocation.
            </p>
          </div>
          <div className="space-y-2 p-4 rounded-lg border border-fd-border">
            <h3 className="font-semibold">Interactive CLI</h3>
            <p className="text-sm text-fd-muted-foreground">
              Guided wizards for testbed setup, device management, test
              execution, and configuration.
            </p>
          </div>
        </div>
      </div>
    </main>
  );
}
