#[test]
fn deno_images_match_the_wasm_kitty_protocol_and_reject_invalid_coordinates() {
    use yinz_tile::ui_components::{image_row, place_image_sequence, png_transmit_sequence};
    let mut runtime = sdk();
    let png = include_bytes!("../../../../yinz-test-support/assets/graphics.png");
    run(&mut runtime, &format!(
        "const image = yinz.ui.image; const png = new Uint8Array({}); check(image.transmitPng(45056, png) === {}); check(image.transmitPng(45056, [...png]) === {}); check(image.place(45056, 10, 6) === {});",
        json!(png.as_slice()), json!(png_transmit_sequence(45056, png)),
        json!(png_transmit_sequence(45056, png)), json!(place_image_sequence(45056, 10, 6))
    ));
    for row in 0..6 {
        run(&mut runtime, &format!(
            "check(image.row(45056, {row}, 10) === {});", json!(image_row(45056, row, 10))
        ));
    }
    run(&mut runtime, r#"
      for (const call of [
        () => image.transmitPng(0, png), () => image.place(0x1000000, 1, 1),
        () => image.place(1, 298, 1), () => image.place(1, 1, 298),
        () => image.row(1, 297, 1), () => image.row(1, 0, 298),
        () => image.transmitPng(1, [256]), () => image.transmitPng(1, [0.1]),
        () => image.transmitPng(1, 'png'), () => image.place(1, -1, 1),
        () => image.place(1, 1, 0), () => image.row(1, 0.5, 1),
        () => image.row(1, NaN, 1), () => image.place(1, 1000000, 1),
        () => image.row(1, 1000000, 1), () => image.row(1, 0, 1000000),
        () => image.transmitPng(1, new Proxy([], {})),
      ]) {
        let rejected = false;
        try { call(); } catch (_) { rejected = true; }
        check(rejected);
      }
      check(image.maxId === 0xffffff && image.maxDimension === 297);
      check(image.place(image.maxId, 297, 297).includes('i=16777215,c=297,r=297'));
      check(image.row(image.maxId, 296, 297).includes('\u{10eeee}'));
    "#);
}

#[test]
fn deno_request_ids_use_v4_uuids_without_a_host_command_or_plugin_callback() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      const ids = Array.from({length: 100}, () => yinz.randomUUID());
      check(new Set(ids).size === ids.length);
      check(ids.every(id => /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id)));
    "#);
    assert!(runtime.op_state().borrow().borrow::<TestHost>().commands.is_empty());
}

#[test]
fn deno_image_and_uuid_types_are_available_to_plugin_authors() {
    let directory = tempfile::tempdir().unwrap();
    checker::check_semantic_source(&directory.path().join("image.ts"), r#"
      import {yinz} from 'yinz:plugin';
      const request: string = yinz.randomUUID();
      const image = yinz.ui.image;
      const frame: string = image.transmitPng(1, new Uint8Array([137, 80, 78, 71]))
        + image.place(1, 2, 1) + image.row(1, 0, 2);
      // @ts-expect-error PNG data must be bytes.
      image.transmitPng(1, 'png');
      // @ts-expect-error Coordinates must be numbers.
      image.row(1, '0', 2);
      export default {render() { return frame + request; }};
    "#).unwrap();
}
