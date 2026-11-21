# Vendored frontend libraries

The builtin web app has no build step. The JavaScript libraries come from npm registry
tarballs whose integrity was verified against registry metadata (`dist.integrity`
SHA-512 and `dist.shasum` SHA-1) before extraction. Only the files listed here are
shipped. This record is installed with the builtins but not served (the asset
loader serves only allowlisted extensions; licenses use `.txt` so they are served
beside the code).

## Why not `htm/preact/standalone.module.js`

The plan preferred htm 3.1.1's `preact/standalone.module.js`. It exports the
needed hooks, but it embeds a re-minified Preact with no version string; htm's
`package.json` only records the devDependency range `preact ^10.2.0`. The embedded
Preact version cannot be identified, so the plan's fallback is used: exact-pinned
Preact and Preact hooks ESM builds plus htm's standalone core.

## Packages

| Package | Version | License | Tarball (immutable) | npm integrity | npm shasum |
|---|---|---|---|---|---|
| preact | 10.29.8 | MIT | https://registry.npmjs.org/preact/-/preact-10.29.8.tgz | `sha512-ej2aVZ+vZ8WO7tvlQWRM9N63A0KzF9q4mWJfDUHgYaIofWY9hu74QdnQrjoPMmZi2/nZ5gN0bJCQF49xQqx09Q==` | `fc85b82dc2474e245b1430b51781d417361f5fc3` |
| htm | 3.1.1 | Apache-2.0 | https://registry.npmjs.org/htm/-/htm-3.1.1.tgz | `sha512-983Vyg8NwUE7JkZ6NmOqpCZ+sh1bKv2iYTlUkzlWmA5JD2acKoxd4KVxbMmxX/85mtfdnDmTFoNKcg5DGAvxNQ==` | `49266582be0dc66ed2235d5ea892307cc0c24b78` |
| @xterm/xterm | 6.0.0 | MIT | https://registry.npmjs.org/@xterm/xterm/-/xterm-6.0.0.tgz | `sha512-TQwDdQGtwwDt+2cgKDLn0IRaSxYu1tSUjgKarSDkUM0ZNiSRXFpjxEsvc/Zgc5kq5omJ+V0a8/kIM2WD3sMOYg==` | `93637b0f2ee3a70718b5746a27c9c506af16745b` |

## Files

| Shipped file | Source file | Upstream SHA-256 | Shipped SHA-256 |
|---|---|---|---|
| `preact.module.js` | preact: `package/dist/preact.module.js` | `c30e721ebfdc6e2ad4c18c14d2dfb82667829c8aec27de1207774e3fc16858a8` | `25a5df7e9f628a587743c4641a368737a3b218f28ef738ba0376a2d5bdfc948c` |
| `hooks.module.js` | preact: `package/hooks/dist/hooks.module.js` | `a6ee626f2d01570592dd569a792e3f050154aa02890eead8c223fa3ed5aa3d5a` | `9e7eff58e0ae604583461eba1344da69a6894eab575eb591c635cc0d3f9ec57d` |
| `htm.module.js` | htm: `package/dist/htm.module.js` | `ab33dd3f38059b9be4d5f5350128eefb2356639c4e0bbe9d9e8b3ba75847e9e4` | `ab33dd3f38059b9be4d5f5350128eefb2356639c4e0bbe9d9e8b3ba75847e9e4` |
| `LICENSE-preact.txt` | preact: `package/LICENSE` (MIT) | `1fe6958409c8c257a70c587a18b6f7f412b179b456630790d30b2ec9a8e4b7d4` | `1fe6958409c8c257a70c587a18b6f7f412b179b456630790d30b2ec9a8e4b7d4` |
| `LICENSE-htm.txt` | htm: `package/LICENSE` (Apache License 2.0) | `740725f7252e750af735d0028cc534970772f513331e9f68150fede8fb3ce00f` | `740725f7252e750af735d0028cc534970772f513331e9f68150fede8fb3ce00f` |
| `xterm.mjs` | @xterm/xterm: `package/lib/xterm.mjs` | `b336ec65a086c056d4804b3d4c2347da5663d3f23c3f25be866467bd8857ad59` | `59ca5174aa9e0fd9485fb464a4564b9ff4a2afe26968a182ed4d63fc863b849b` |
| `xterm.css` | @xterm/xterm: `package/css/xterm.css` | `854a7c0fb70e8b1a083c16797ab827299fb18744f5ad34f227b48337e33293c6` | `854a7c0fb70e8b1a083c16797ab827299fb18744f5ad34f227b48337e33293c6` |
| `LICENSE-xterm.txt` | @xterm/xterm: `package/LICENSE` | `b569f629d00f2626a8100df2a1798210535621e42164dfd426a6fe5aac7b0ccd` | `b569f629d00f2626a8100df2a1798210535621e42164dfd426a6fe5aac7b0ccd` |
| `TerminessNerdFontMono-Regular-latin.woff2` | Nerd Fonts 3.5.1: `TerminessNerdFontMono-Regular.ttf` | `1f0b30594e217a4771770dbdcae90ecbf1620b8b15429d01a23481865cce4de8` | `b4d0a5fb404040c75a9a6d902d6bc642b69e0798d4cd7c127df08cbba7ace0a7` |
| `TerminessNerdFontMono-Regular-extended.woff2` | Nerd Fonts 3.5.1: `TerminessNerdFontMono-Regular.ttf` | `1f0b30594e217a4771770dbdcae90ecbf1620b8b15429d01a23481865cce4de8` | `ca23f0d5a640501b28aba1ab5d3e1d7613a4c31c00abf8819f29130e325a1ac2` |
| `LICENSE-Nerd-Fonts.txt` | Glyph project notices (sources in file) | multiple | `0c52659ac298add701ef8e1853f0d68160e61586dc7f5d72a3690c3ad1afde52` |
| `LICENSE-Terminess.txt` | Terminess: `LICENSE.txt` (SIL OFL 1.1) | `6a30ebc918d73e02c283f04099fb2cf97ddc15e1c8368960b3f1bbb7ee959ae4` | `15aed6aae8f1d81ddd8858a0dfe649b80b884fb5163166ff365ae445236008f9` |

The regular face and its Terminus license come from Arch Linux's
`ttf-terminus-nerd` 3.5.1-2 package. The Terminus license has LF line endings
and trailing whitespace removed. `LICENSE-Nerd-Fonts.txt` credits the embedded
glyph projects and includes their notices and sources.

Generate the two WOFF2 files from the regular TTF with FontTools:

```sh
pyftsubset TerminessNerdFontMono-Regular.ttf --flavor=woff2 --unicodes=U+0000-00FF --output-file=TerminessNerdFontMono-Regular-latin.woff2
pyftsubset TerminessNerdFontMono-Regular.ttf --flavor=woff2 --unicodes=U+0100-10FFFF --output-file=TerminessNerdFontMono-Regular-extended.woff2
```

## Transformations

1. `preact.module.js`: deleted the final line `//# sourceMappingURL=preact.module.js.map`
   (source maps are not shipped and `.map` is not a served extension). No other change.
2. `hooks.module.js`: rewrote the bare import `from"preact"` to the relative
   `from"./preact.module.js"` (the only import in the file) and deleted the final
   line `//# sourceMappingURL=hooks.module.js.map`. No other change.
3. `htm.module.js`: unchanged. It has no imports; `views/ui.js` binds it to
   Preact's `h` (`htm.bind(h)`).

Equivalent commands, run outside the repository on the extracted tarballs:

```sh
sed -e '/^\/\/# sourceMappingURL=preact\.module\.js\.map$/d' package/dist/preact.module.js > preact.module.js
sed -e 's|from"preact"|from"./preact.module.js"|' \
    -e '/^\/\/# sourceMappingURL=hooks\.module\.js\.map$/d' package/hooks/dist/hooks.module.js > hooks.module.js
```

The resulting import graph uses relative specifiers only and contains no dynamic
imports, `eval` or `new Function` (checked by `yinz-steel-builtins/src/unit/web/api.test.mjs`).

The xterm ESM build uses the registered `yinz-xterm-style` element for its three
dynamic stylesheets so the renderer can install constructed stylesheets under
the site's external-only CSP. Its core data emitter forwards only events marked
as user input. This keeps device replies on the owning PTY. Mouse coordinates
are converted from the scaled bounding rectangle into local layout pixels
before cell lookup. The print loop skips zero-width codepoints when the width provider marks them
as non-joining, matching the native grid. Cursor save and restore copy all four
charset designations and retain the active shift level, as the native cursor
does. Committed input-only insertions use the normal input handler in screen-reader
mode. The composition helper retains ownership through commit delivery.
All mouse encodings emit through `onBinary` so the client can distinguish
pointer reports from keyboard input and pasted bytes.
DECSET 1049 saves the normal cursor and starts the alternate buffer at the
top left. Repeating it while the alternate buffer is active preserves both
cursors. Modes 47 and 1047 still enter at the normal cursor position.
The unshipped source map reference is removed.
