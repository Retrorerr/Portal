# Portal website

The Portal product site and documentation are built with [Docusaurus](https://docusaurus.io/).

## Installation

```
pnpm install
```

## Local development

```
pnpm start
```

This command starts a local development server and opens up a browser window. Most changes are reflected live without having to restart the server.

## Production build

```
pnpm typecheck
pnpm build
```

This command generates static content into the `build` directory and can be served using any static contents hosting service.

## Deployment

Changes under `gh-pages/` on `main` are deployed to `https://retrorerr.github.io/Portal/` by GitHub Actions.
