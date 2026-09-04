import React, { type ReactNode } from "react";
import Link from "@docusaurus/Link";
import Head from "@docusaurus/Head";
import Layout from "@theme/Layout";
import Heading from "@theme/Heading";
import config from "@site/docusaurus.config";

const SIGNALS = [
  ["ARM64", "native target"],
  ["Debian 13", "Trixie guest"],
  ["Plasma 6", "Wayland session"],
  ["107", "host tests"],
];

const FEATURES = [
  {
    marker: "01",
    title: "Native Wayland",
    body: "Plasma talks directly to Portal's Rust compositor. No VNC stream, no X server in the middle.",
  },
  {
    marker: "02",
    title: "Rootless by design",
    body: "A complete Debian userspace lives inside one Android app—without unlocking or modifying the device.",
  },
  {
    marker: "03",
    title: "Tablet-first input",
    body: "Physical keyboard, trackpad, clipboard, audio, high-density displays, and Android lifecycle handling are built in.",
  },
  {
    marker: "04",
    title: "Recoverable runtime",
    body: "A/B runtime slots, diagnostics export, and a native Wayland recovery session keep failures inspectable.",
  },
];

export default function Home(): ReactNode {
  const downloadUrl = config.customFields.downloadUrl as string;
  const repositoryUrl = config.customFields.repositoryUrl as string;

  return (
    <Layout title="Debian desktop on Android" description={config.tagline}>
      <Head>
        <meta name="theme-color" content="#07090d" />
        <meta property="og:title" content="Portal — Debian desktop on Android" />
        <meta property="og:description" content={config.tagline} />
        <meta property="og:type" content="website" />
        <meta property="og:url" content="https://retrorerr.github.io/Portal/" />
        <meta property="og:image" content="https://retrorerr.github.io/Portal/img/portal-icon.png" />
        <meta name="twitter:card" content="summary_large_image" />
        <script type="application/ld+json">
          {JSON.stringify({
            "@context": "https://schema.org",
            "@type": "SoftwareApplication",
            name: "Portal",
            applicationCategory: "DeveloperApplication",
            operatingSystem: "Android",
            description: config.tagline,
            url: "https://retrorerr.github.io/Portal/",
            downloadUrl,
            codeRepository: repositoryUrl,
            license: `${repositoryUrl}/blob/main/LICENSE`,
            offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
          })}
        </script>
      </Head>

      <main className="portal-home">
        <section className="portal-hero">
          <div className="portal-grid" aria-hidden="true" />
          <div className="portal-glow" aria-hidden="true" />
          <div className="portal-wrap portal-hero__grid">
            <div className="portal-hero__copy">
              <p className="portal-eyebrow"><span /> Debian 13 · Plasma 6 · Native Wayland</p>
              <Heading as="h1">Your Linux desktop is already in your bag.</Heading>
              <p className="portal-hero__lede">
                Portal turns an ARM64 Android tablet into a complete, rootless Debian workstation—with a real KDE Plasma session rendered directly through Wayland.
              </p>
              <div className="portal-actions">
                <Link className="portal-button portal-button--primary" to={downloadUrl}>Get Portal <span>↗</span></Link>
                <Link className="portal-button portal-button--quiet" to="/docs/developer/how-it-works">Explore the architecture <span>→</span></Link>
              </div>
              <p className="portal-caveat">Built and verified on OnePlus Pad 3. Other ARM64 devices are experimental.</p>
            </div>

            <div className="portal-mark" aria-label="Portal application mark">
              <div className="portal-mark__halo" />
              <img src="img/portal-icon.png" alt="Portal icon" />
              <div className="portal-mark__label">
                <span className="portal-status-dot" />
                <div><strong>PORTAL / ACTIVE</strong><small>native compositor · arm64</small></div>
              </div>
            </div>
          </div>
        </section>

        <section className="portal-signal" aria-label="Platform summary">
          <div className="portal-wrap portal-signal__grid">
            {SIGNALS.map(([value, label]) => (
              <div className="portal-signal__item" key={value}><strong>{value}</strong><span>{label}</span></div>
            ))}
          </div>
        </section>

        <section className="portal-section">
          <div className="portal-wrap">
            <div className="portal-section__head">
              <p className="portal-kicker">THE STACK</p>
              <Heading as="h2">Android outside. Debian inside.</Heading>
              <p>Portal owns the difficult boundary between a mobile operating system and a desktop Linux session.</p>
            </div>
            <div className="portal-pipeline" aria-label="Portal architecture">
              {["Android", "Portal host", "Debian 13", "Plasma 6"].map((item, index) => (
                <React.Fragment key={item}>
                  <div className={`portal-node portal-node--${index}`}><small>0{index + 1}</small><strong>{item}</strong></div>
                  {index < 3 && <span className="portal-arrow" aria-hidden="true">→</span>}
                </React.Fragment>
              ))}
            </div>
          </div>
        </section>

        <section className="portal-section portal-section--features">
          <div className="portal-wrap portal-features">
            {FEATURES.map((feature) => (
              <article className="portal-feature" key={feature.marker}>
                <span>{feature.marker}</span>
                <Heading as="h3">{feature.title}</Heading>
                <p>{feature.body}</p>
              </article>
            ))}
          </div>
        </section>

        <section className="portal-section portal-section--terminal">
          <div className="portal-wrap portal-terminal-grid">
            <div>
              <p className="portal-kicker">OPEN, HACKABLE, YOURS</p>
              <Heading as="h2">No cloud computer hiding behind the glass.</Heading>
              <p>Portal is GPL-3.0 software. The compositor, Android integration, Debian bootstrap, recovery tools, and documentation all live in the open.</p>
              <Link className="portal-text-link" to={repositoryUrl}>Read the source on GitHub <span>↗</span></Link>
            </div>
            <div className="portal-terminal" role="img" aria-label="Terminal showing Debian inside Portal">
              <div className="portal-terminal__bar"><i /><i /><i /><span>portal@android — bash</span></div>
              <pre><code><b>$</b> cat /etc/os-release{"\n"}<span>PRETTY_NAME="Debian GNU/Linux 13 (trixie)"</span>{"\n\n"}<b>$</b> echo $XDG_SESSION_TYPE{"\n"}<span>wayland</span>{"\n\n"}<b>$</b> uname -m{"\n"}<span>aarch64</span><em>_</em></code></pre>
            </div>
          </div>
        </section>

        <section className="portal-cta">
          <div className="portal-wrap portal-cta__inner">
            <img src="img/portal-icon.png" alt="" />
            <div><Heading as="h2">Step through.</Heading><p>Build it, inspect it, make the machine yours.</p></div>
            <Link className="portal-button portal-button--primary" to={downloadUrl}>View releases <span>↗</span></Link>
          </div>
        </section>
      </main>
    </Layout>
  );
}
