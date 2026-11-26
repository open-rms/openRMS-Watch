<script lang="ts">
  import "../app.css";
  import { GITHUB_URL } from "#lib/release.ts";
  let { children } = $props();

  const nav = [
    { href: "/#download", label: "Download" },
    { href: "/docs/", label: "Docs" },
    { href: "https://atlas.openrms.dev/live", label: "Live feed" },
    { href: GITHUB_URL, label: "GitHub" },
  ];
</script>

<a href="#main" class="bg-primary text-primary-foreground sr-only rounded px-3 py-2 focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50">Skip to content</a>

<header class="bg-background/85 sticky top-0 z-40 border-b backdrop-blur">
  <div class="mx-auto flex min-h-14 max-w-6xl flex-wrap items-center gap-x-6 gap-y-1 px-4 py-2">
    <a class="text-foreground flex items-center gap-2 text-[1.05rem] no-underline" href="/">
      <img src="/icon.png" alt="" width="26" height="26" class="rounded" />
      <span class="font-semibold tracking-tight">openRMS <span class="text-primary">Watch</span></span>
    </a>
    <nav aria-label="Site" class="flex flex-1 flex-wrap justify-end gap-x-5 gap-y-1 text-sm">
      {#each nav as n}
        <a href={n.href} class="text-muted-foreground hover:text-foreground no-underline">{n.label}</a>
      {/each}
    </nav>
  </div>
</header>

<main id="main">
  {@render children()}
</main>

<footer class="mt-24 border-t">
  <div class="text-muted-foreground mx-auto flex max-w-6xl flex-col gap-2 px-4 py-8 text-sm sm:flex-row sm:justify-between">
    <p class="my-0">
      openRMS Watch is AGPL-3.0-or-later; the bundled Mercury is GPL-3.0-or-later. Docs CC-BY-4.0.
    </p>
    <p class="my-0">Part of <a class="text-primary" href="https://openrms.dev">openRMS</a>: open modes for radio email.</p>
  </div>
</footer>
