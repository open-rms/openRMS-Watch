<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import * as Card from "#lib/components/ui/card/index.ts";
  import { archOf, mb, GITHUB_URL } from "#lib/release.ts";
  import { docs } from "#lib/docs.ts";
  import Radio from "@lucide/svelte/icons/radio";
  import ScanSearch from "@lucide/svelte/icons/scan-search";
  import ListChecks from "@lucide/svelte/icons/list-checks";
  import Share2 from "@lucide/svelte/icons/share-2";
  import Laptop from "@lucide/svelte/icons/laptop";
  import Server from "@lucide/svelte/icons/server";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Download from "@lucide/svelte/icons/download";
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  let { data } = $props();
  const rel = $derived(data.release);

  const steps = [
    {
      icon: Radio,
      title: "Listen to whole bands",
      body: "One SDR captures several HF band segments at once, for example 40, 30, 20 and 17 m on a HackRF. No list of frequencies needed.",
    },
    {
      icon: ScanSearch,
      title: "Catch Mercury connections",
      body: "It spots bursts shaped like Mercury's connect frames and points a decoder at each, replaying a few seconds of audio so the first call isn't missed.",
    },
    {
      icon: ListChecks,
      title: "Log who called whom",
      body: "The client's callsign, the RMS behind the call, the frequency and the signal strength go into a local database, with a page to browse, filter and export it.",
    },
    {
      icon: Share2,
      title: "Share it, if you want",
      body: "Optionally send what you hear to openRMS Atlas: a live feed, proof that listed gateways are on the air, and credit in the contributor list.",
    },
  ];

  const features = [
    { icon: Laptop, title: "Desktop or headless", body: "A desktop app with a tray icon, or a service on a server or Raspberry Pi with everything set from the web page." },
    { icon: Server, title: "Any SDR SoapySDR knows", body: "HackRF, RTL-SDR, Pluto, Airspy, SDRplay and more. It picks the sample rate and centre frequency from the bands you tick." },
    { icon: ShieldCheck, title: "Local first", body: "Contributing is off until you turn it on. The page only listens on your own computer unless you set a password." },
  ];
</script>

<svelte:head>
  <title>openRMS Watch: hear which Mercury gateways are on the air</title>
  <meta name="description" content="openRMS Watch scans HF bands with an SDR, decodes Mercury radio-email connections and logs which gateways are really on the air. Free and open source, for Linux." />
</svelte:head>

<section class="mx-auto max-w-6xl px-4 pt-16 pb-10">
  <p class="text-primary mb-3 text-sm font-semibold tracking-wide uppercase">openRMS Watch</p>
  <h1 class="max-w-3xl text-4xl leading-tight font-semibold tracking-tight sm:text-5xl">
    Hear which Mercury gateways are really on the air.
  </h1>
  <p class="text-muted-foreground mt-5 max-w-2xl text-lg">
    A passive watcher for radio email. It scans HF bands with an SDR, decodes Mercury connections, and logs
    the stations it hears, for you, and if you like, for everyone through openRMS Atlas.
  </p>
  <div class="mt-8 flex flex-wrap gap-3">
    <Button href="#download"><Download class="size-4" /> Download</Button>
    <Button variant="outline" href="/docs/install/">Install guide <ArrowRight class="size-4" /></Button>
  </div>
  <div class="bg-card mt-8 flex max-w-2xl items-start gap-3 rounded-lg border p-4 text-sm">
    <TriangleAlert class="text-stale mt-0.5 size-4 shrink-0" />
    <p class="my-0">
      <strong>Alpha.</strong> It decodes recorded and synthetic Mercury exchanges on every build, but hasn't been
      checked against a real on-air station yet. Reports from people with a good HF antenna are very welcome.
    </p>
  </div>
</section>

<section class="mx-auto max-w-6xl px-4 py-10" aria-labelledby="how">
  <h2 id="how" class="text-2xl font-semibold tracking-tight">How it works</h2>
  <div class="mt-6 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
    {#each steps as s, i}
      <Card.Root>
        <Card.Header>
          <div class="bg-primary-soft text-primary mb-2 flex size-9 items-center justify-center rounded-md">
            <s.icon class="size-5" />
          </div>
          <Card.Title class="text-base">{i + 1}. {s.title}</Card.Title>
        </Card.Header>
        <Card.Content class="text-muted-foreground text-sm">{s.body}</Card.Content>
      </Card.Root>
    {/each}
  </div>
  <div class="mt-6 grid gap-4 sm:grid-cols-3">
    {#each features as f}
      <div class="flex gap-3">
        <f.icon class="text-primary mt-0.5 size-5 shrink-0" />
        <div>
          <p class="my-0 font-semibold">{f.title}</p>
          <p class="text-muted-foreground my-1 text-sm">{f.body}</p>
        </div>
      </div>
    {/each}
  </div>
</section>

<section class="mx-auto max-w-6xl px-4 py-10" aria-labelledby="download" id="download">
  <h2 class="text-2xl font-semibold tracking-tight">Download</h2>
  {#if rel}
    <p class="text-muted-foreground mt-2">
      {rel.tag}{rel.prerelease ? " (pre-release)" : ""}, published {new Date(rel.published).toISOString().slice(0, 10)}.
      Linux only: x86_64, ARM and RISC-V. <a class="text-primary" href={rel.url}>Release notes</a>
      {#if rel.sums}· <a class="text-primary" href={rel.sums.url}>SHA256SUMS</a>{/if}
    </p>
    <div class="bg-card mt-5 divide-y rounded-lg border">
      {#each rel.downloads as d}
        <div class="flex flex-col gap-2 p-4 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <p class="my-0 font-semibold">{d.label}</p>
            <p class="text-muted-foreground my-0 font-mono text-xs">{d.detail}</p>
          </div>
          <div class="flex flex-wrap gap-2">
            {#each d.assets as a}
              <Button variant="outline" size="sm" href={a.url} title={a.name}>
                {archOf(a.name)} <span class="text-muted-foreground text-xs">{mb(a.size)}</span>
              </Button>
            {/each}
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <p class="text-muted-foreground mt-2">
      See the <a class="text-primary" href="{GITHUB_URL}/releases">releases page</a>.
    </p>
  {/if}
  <p class="text-muted-foreground mt-4 text-sm">
    You'll also need SoapySDR and your radio's module, and above all an HF antenna. See
    <a class="text-primary" href="/docs/install/">Install</a> and
    <a class="text-primary" href="/docs/hardware/">Choosing an SDR and antenna</a>.
  </p>
</section>

<section class="mx-auto max-w-6xl px-4 py-10" aria-labelledby="docs">
  <h2 id="docs" class="text-2xl font-semibold tracking-tight">Documentation</h2>
  <div class="mt-5 grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
    {#each docs as d}
      <a href="/docs/{d.slug}/" class="bg-card hover:border-primary block rounded-lg border p-4 no-underline">
        <p class="text-foreground my-0 font-semibold">{d.title}</p>
        <p class="text-muted-foreground my-1 text-sm">{d.description}</p>
      </a>
    {/each}
  </div>
</section>

<section class="mx-auto max-w-6xl px-4 py-10" aria-labelledby="network">
  <h2 id="network" class="text-2xl font-semibold tracking-tight">The watcher network</h2>
  <p class="text-muted-foreground mt-2 max-w-2xl">
    Watchers that contribute feed openRMS Atlas, the open gateway registry. See what's being heard right now, and
    who's listening.
  </p>
  <div class="mt-5 flex flex-wrap gap-3">
    <Button variant="outline" href="https://atlas.openrms.dev/live">Live feed <ArrowRight class="size-4" /></Button>
    <Button variant="outline" href="https://atlas.openrms.dev/contributors">Contributors <ArrowRight class="size-4" /></Button>
    <Button variant="outline" href="https://atlas.openrms.dev/docs">Atlas API docs <ArrowRight class="size-4" /></Button>
  </div>
</section>
