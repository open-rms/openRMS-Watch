// The download table, filled in at build time from the newest GitHub release
// (pre-releases included, while the project is alpha). A rebuild picks up a new tag.
export const REPO = "open-rms/openRMS-Watch";
export const GITHUB_URL = `https://github.com/${REPO}`;

export type Asset = { name: string; url: string; size: number };
export type Download = { label: string; detail: string; assets: Asset[] };
export type Release = { tag: string; url: string; published: string; prerelease: boolean; downloads: Download[]; sums?: Asset };

const groups: { label: string; detail: string; test: RegExp }[] = [
  { label: "Desktop: Debian, Ubuntu, Mint", detail: "sudo apt install ./<file>", test: /desktop.*\.deb$/ },
  { label: "Desktop: Fedora, openSUSE", detail: "sudo dnf install ./<file>", test: /desktop.*\.rpm$/ },
  { label: "Desktop: any distribution", detail: "chmod +x, then run it", test: /desktop.*\.AppImage$/ },
  { label: "Headless or Raspberry Pi", detail: "unpack, then ./install.sh --headless", test: /^openrms-watch-[\d.]+-linux-[a-z0-9_]+\.tar\.gz$/ },
  { label: "Alpine Linux (musl)", detail: "unpack, then ./install.sh --headless", test: /musl\.tar\.gz$/ },
];

const ARCH_ORDER = ["x86_64", "amd64", "aarch64", "arm64", "armv7l", "riscv64"];
function byArch(a: Asset, b: Asset): number {
  const k = (n: string) => ARCH_ORDER.findIndex((x) => n.includes(x));
  return k(a.name) - k(b.name);
}

export async function latestRelease(fetchFn: typeof fetch): Promise<Release | null> {
  try {
    const r = await fetchFn(`https://api.github.com/repos/${REPO}/releases?per_page=5`, {
      headers: { accept: "application/vnd.github+json", "user-agent": "openrms-watch-site" },
    });
    if (!r.ok) return null;
    const list = (await r.json()) as any[];
    const rel = list.find((x) => !x.draft && x.assets?.length);
    if (!rel) return null;
    const assets: Asset[] = rel.assets.map((a: any) => ({ name: a.name, url: a.browser_download_url, size: a.size }));
    return {
      tag: rel.tag_name,
      url: rel.html_url,
      published: rel.published_at,
      prerelease: rel.prerelease,
      sums: assets.find((a) => a.name === "SHA256SUMS"),
      downloads: groups
        .map((g) => ({ label: g.label, detail: g.detail, assets: assets.filter((a) => g.test.test(a.name)).sort(byArch) }))
        .filter((g) => g.assets.length),
    };
  } catch {
    return null;
  }
}

/** "x86_64", "arm64" ... from a file name, for the button label. */
export function archOf(name: string): string {
  const m = name.match(/(x86_64|amd64|aarch64|arm64|armv7l|riscv64)/);
  const a = m ? m[1] : name;
  return { amd64: "x86_64", aarch64: "arm64 (aarch64)", arm64: "arm64 (aarch64)", armv7l: "armv7 (32-bit Pi)" }[a] ?? a;
}

export function mb(n: number): string {
  return `${(n / 1048576).toFixed(n > 20971520 ? 0 : 1)} MB`;
}
