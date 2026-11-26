# watch.openrms.dev

The openRMS Watch landing page, downloads and docs. Static SvelteKit 3 + Tailwind v4 +
shadcn-svelte, the same toolchain and theme as openRMS-Home.

- **Docs** come from `../docs/*.md` (front matter: `title`, `description`, `order`), rendered at
  build time. Edit them there; they read fine on GitHub too.
- **Downloads** come from the newest GitHub release at build time. Rebuild and deploy after
  tagging a release.

```sh
pnpm install
pnpm dev          # http://localhost:5176
pnpm check
pnpm build        # static site in build/
pnpm cf:deploy    # build + wrangler deploy (Worker openrms-watch-site, custom domain watch.openrms.dev)
```
