import { themes as prismThemes } from "prism-react-renderer";
import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";

const downloadUrl = "https://github.com/Retrorerr/Portal/releases";
const repositoryUrl = "https://github.com/Retrorerr/Portal";

const config: Config = {
  title: "Portal | Debian desktop on Android",
  tagline: "A real Debian 13 and KDE Plasma 6 workspace, running rootlessly on Android.",
  favicon: "img/portal-icon.png",
  url: "https://retrorerr.github.io",
  baseUrl: "/Portal/",
  organizationName: "Retrorerr",
  projectName: "Portal",
  onBrokenLinks: "throw",
  onBrokenMarkdownLinks: "warn",

  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },

  presets: [
    [
      "classic",
      {
        docs: {
          sidebarPath: "./sidebars.ts",
          editUrl: "https://github.com/Retrorerr/Portal/tree/main/gh-pages/",
        },
        blog: {
          showReadingTime: true,
          feedOptions: { type: ["rss", "atom", "json"], xslt: true },
          editUrl: "https://github.com/Retrorerr/Portal/tree/main/gh-pages/",
          onInlineTags: "warn",
          onInlineAuthors: "warn",
          onUntruncatedBlogPosts: "warn",
        },
        theme: { customCss: "./src/css/custom.css" },
        sitemap: {
          changefreq: "weekly",
          priority: 0.5,
          ignorePatterns: ["/blog/archive", "/blog/authors", "/blog/tags", "/blog/tags/**"],
          filename: "sitemap.xml",
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    image: "img/portal-icon.png",
    metadata: [
      {
        name: "keywords",
        content: "Portal, Debian Android, KDE Plasma Android, Wayland Android, rootless Linux, ARM64 tablet",
      },
    ],
    navbar: {
      title: "Portal",
      logo: { alt: "Portal", src: "img/portal-icon.png" },
      items: [
        { type: "docSidebar", sidebarId: "userSidebar", position: "left", label: "Guide" },
        { type: "docSidebar", sidebarId: "developerSidebar", position: "left", label: "Architecture" },
        { to: "/blog", label: "Journal", position: "left" },
        { href: downloadUrl, label: "Download", position: "right" },
        { href: repositoryUrl, label: "GitHub", position: "right" },
      ],
    },
    footer: {
      style: "dark",
      links: [
        { label: "Get started", to: "/docs/user/getting-started" },
        { label: "How it works", to: "/docs/developer/how-it-works" },
        { label: "Releases", href: downloadUrl },
        { label: "Source", href: repositoryUrl },
      ],
      copyright: `Portal is free software under GPL-3.0 · ${new Date().getFullYear()}`,
    },
    prism: { theme: prismThemes.github, darkTheme: prismThemes.dracula },
    mermaid: { theme: { light: "neutral", dark: "dark" } },
  } satisfies Preset.ThemeConfig,

  plugins: [
    () => ({
      name: "@tailwindcss/postcss",
      configurePostCss(options) {
        options.plugins.push({ "@tailwindcss/postcss": {} });
        return options;
      },
    }),
  ],

  markdown: { mermaid: true },
  themes: ["@docusaurus/theme-mermaid"],
  customFields: { downloadUrl, repositoryUrl },
};

export default config;
