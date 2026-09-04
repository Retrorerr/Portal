# Portal icon

The mark is an oblique ivory aperture with a detached ember-orange threshold,
on warm charcoal. Its open form should remain legible without lettering.

- `portal-icon.svg`: legacy icon and repository artwork.
- `portal-icon-foreground.svg`: transparent Android adaptive foreground.
- `portal-icon-monochrome.svg`: the same silhouette for themed icons.

Regenerate PNGs with Node.js and `sharp` available:

```sh
node scripts/render-icon.cjs target/portal-icon-preview.png
```

The renderer checks the foreground against Android's 66dp safe circle inside
the 108dp layer. The preview models the central 72dp crop and its 1.5x scale;
masking the whole source image would understate launcher clipping. Keep the
transparent margins. Adaptive resources are exported at 108dp per density;
legacy resources use 48dp. The background also lives in the xbuild adaptive XML.
