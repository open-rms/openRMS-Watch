// The docs live in the repository's docs/ folder (readable on GitHub as well); this
// renders them at build time. Front matter: title, description, order.
import { Marked } from "marked";

const files = import.meta.glob("../../../docs/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export type Doc = { slug: string; title: string; description: string; order: number; html: string; headings: { id: string; text: string }[] };

function slugify(s: string): string {
  return s.toLowerCase().replace(/<[^>]+>/g, "").replace(/[^a-z0-9 -]/g, "").trim().replace(/\s+/g, "-");
}

function parse(slug: string, raw: string): Doc {
  let meta: Record<string, string> = {};
  let body = raw;
  const m = raw.match(/^---\n([\s\S]*?)\n---\n/);
  if (m) {
    for (const line of m[1].split("\n")) {
      const i = line.indexOf(":");
      if (i > 0) meta[line.slice(0, i).trim()] = line.slice(i + 1).trim();
    }
    body = raw.slice(m[0].length);
  }
  const headings: { id: string; text: string }[] = [];
  const marked = new Marked({
    gfm: true,
    renderer: {
      heading({ tokens, depth }) {
        const text = this.parser.parseInline(tokens);
        const id = slugify(text);
        if (depth === 2) headings.push({ id, text: text.replace(/<[^>]+>/g, "") });
        // the page shows the title itself
        if (depth === 1) return "";
        return `<h${depth} id="${id}"><a class="anchor" href="#${id}">${text}</a></h${depth}>\n`;
      },
      link({ href, title, tokens }) {
        const text = this.parser.parseInline(tokens);
        // docs/foo.md#bar -> /docs/foo/#bar; external links open in place but are marked
        let h = href;
        const md = href.match(/^([a-z0-9-]+)\.md(#.*)?$/);
        if (md) h = `/docs/${md[1]}/${md[2] ?? ""}`;
        const ext = /^https?:/.test(h);
        const t = title ? ` title="${title}"` : "";
        return `<a href="${h}"${t}${ext ? ' rel="noopener"' : ""}>${text}</a>`;
      },
    },
  });
  const html = marked.parse(body) as string;
  return {
    slug,
    title: meta.title ?? slug,
    description: meta.description ?? "",
    order: Number(meta.order ?? 99),
    html,
    headings,
  };
}

export const docs: Doc[] = Object.entries(files)
  .map(([path, raw]) => parse(path.split("/").pop()!.replace(/\.md$/, ""), raw))
  .sort((a, b) => a.order - b.order);

export function doc(slug: string): Doc | undefined {
  return docs.find((d) => d.slug === slug);
}
