## About

This is the builtin layout manager, a [Yinz][yinz] plugin in Rust. It lists, opens, imports, renames and saves layouts, and it can also serve as a template to start developing your own plugins.

More about Yinz plugins: [the `yinz-tile` plugin API][docs]

[yinz]: https://github.com/zacharyhamm/yinz
[docs]: ../../yinz-tile/src/lib.rs

## Development

*Note*: you will need to have `wasm32-wasip1` added to rust as a target to build the plugin. This can be done with `rustup target add wasm32-wasip1`.

1. Build the plugins at the root of this repository: `cargo x build --plugins-only`
2. Load it inside a running Yinz session: `yinz action start-or-reload-plugin file:target/wasm32-wasip1/debug/layout-manager.wasm`
3. Repeat on changes (perhaps with a `watchexec` or similar command to run on fs changes).
