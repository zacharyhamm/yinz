# Vendored frontend libraries

The builtin web app has no build step. These files are copied from npm registry
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

## Files

| Shipped file | Source (path inside tarball) | Upstream SHA-256 | Shipped SHA-256 |
|---|---|---|---|
| `preact.module.js` | preact: `package/dist/preact.module.js` | `c30e721ebfdc6e2ad4c18c14d2dfb82667829c8aec27de1207774e3fc16858a8` | `25a5df7e9f628a587743c4641a368737a3b218f28ef738ba0376a2d5bdfc948c` |
| `hooks.module.js` | preact: `package/hooks/dist/hooks.module.js` | `a6ee626f2d01570592dd569a792e3f050154aa02890eead8c223fa3ed5aa3d5a` | `9e7eff58e0ae604583461eba1344da69a6894eab575eb591c635cc0d3f9ec57d` |
| `htm.module.js` | htm: `package/dist/htm.module.js` | `ab33dd3f38059b9be4d5f5350128eefb2356639c4e0bbe9d9e8b3ba75847e9e4` | `ab33dd3f38059b9be4d5f5350128eefb2356639c4e0bbe9d9e8b3ba75847e9e4` |
| `LICENSE-preact.txt` | preact: `package/LICENSE` (MIT) | `1fe6958409c8c257a70c587a18b6f7f412b179b456630790d30b2ec9a8e4b7d4` | `1fe6958409c8c257a70c587a18b6f7f412b179b456630790d30b2ec9a8e4b7d4` |
| `LICENSE-htm.txt` | htm: `package/LICENSE` (Apache License 2.0) | `740725f7252e750af735d0028cc534970772f513331e9f68150fede8fb3ce00f` | `740725f7252e750af735d0028cc534970772f513331e9f68150fede8fb3ce00f` |

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
