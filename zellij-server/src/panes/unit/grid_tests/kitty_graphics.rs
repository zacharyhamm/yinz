use super::*;

#[test]
fn kitty_transmit_and_display_raw_rgb() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1", &rgb_raster(20, 40)),
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 0,
            y: 0,
            width: 20,
            height: 40,
        }
    );
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn kitty_transmit_and_display_png() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=100,i=1", &KITTY_PNG_2X2),
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        }
    );
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn kitty_cursor_advances_after_display() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1", &rgb_raster(20, 40)),
    );
    let (cursor_x, cursor_y, _) = grid.cursor_coordinates().unwrap();
    assert_eq!((cursor_x, cursor_y), (2, 1));
}

#[test]
fn kitty_cursor_unmoved_with_c1() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    let (cursor_x, cursor_y, _) = grid.cursor_coordinates().unwrap();
    assert_eq!((cursor_x, cursor_y), (0, 0));
}

#[test]
fn kitty_placement_survives_text_overwrite_and_erase() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"XXXX");
    assert_eq!(grid.kitty_placement_count(), 1);
    for erase_sequence in [
        &b"\x1b[K"[..],
        &b"\x1b[1K"[..],
        &b"\x1b[0J"[..],
        &b"\x1b[1J"[..],
    ] {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, erase_sequence);
        assert_eq!(grid.kitty_placement_count(), 1);
    }
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn kitty_replace_same_ids_moves_placement() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,p=1,C=1", &rgb_raster(20, 40)),
    );
    assert_snapshot!(format!("{:?}", grid));
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[10;5H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=p,i=1,p=1,C=1", b""),
    );
    assert_snapshot!(format!("{:?}", grid));
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 40,
            y: 180,
            width: 20,
            height: 40,
        }
    );
}

#[test]
fn kitty_delete_all_visible() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=A", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![]);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 0);
}

#[test]
fn kitty_delete_by_image_id() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=i,i=2", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=2,p=1,C=1", &rgb_raster(20, 40)),
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=p,i=2,p=2,C=1", b""),
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=i,i=2,p=1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(2, 2)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 3200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=I,i=2", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().image_count(), 2);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 4000);
}

#[test]
fn kitty_delete_by_image_number() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,I=7,C=1", &rgb_raster(10, 20)),
    );
    assert_eq!(grid.kitty_placement_count(), 4);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 8000);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=n,I=7", b""),
    );
    assert_eq!(
        kitty_placement_id_set(&mut grid),
        vec![(1, 0), (2, 0), (5, 0)]
    );
    assert_eq!(kitty_image_store.borrow().total_bytes(), 8000);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=N,I=7", b""),
    );
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);
    assert_eq!(kitty_image_store.borrow().image_count(), 3);
}

#[test]
fn kitty_delete_cursor_intersecting() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=c", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (2, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=C", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (2, 0)]);
    assert_eq!(kitty_image_store.borrow().image_count(), 2);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 6400);
}

#[test]
fn kitty_delete_cell_intersecting() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=p,x=11,y=6", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=P,x=11,y=6", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 4000);
}

#[test]
fn kitty_delete_cell_with_z() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=q,x=11,y=6,z=5", b""),
    );
    assert_eq!(
        kitty_placement_id_set(&mut grid),
        vec![(1, 0), (2, 0), (5, 0)]
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=q,x=11,y=6,z=1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=Q,x=11,y=6,z=1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 4000);
}

#[test]
fn kitty_delete_column() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=x,x=1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(2, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=X,x=1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(2, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 4000);
}

#[test]
fn kitty_delete_row() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=y,y=6", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=Y,y=6", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 4000);
}

#[test]
fn kitty_delete_by_z_index() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=z,z=-1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (2, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=Z,z=-1", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(1, 0), (2, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 6400);
}

#[test]
fn kitty_delete_image_id_range() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=r,x=1,y=2", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(5, 0)]);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 7200);

    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=R,x=1,y=2", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(5, 0)]);
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 800);
}

#[test]
fn kitty_delete_image_id_range_with_omitted_lower_bound() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    build_three_kitty_placements(&mut grid, &mut vte_parser, &mut interceptor);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=d,d=r,y=2", b""),
    );
    assert_eq!(kitty_placement_id_set(&mut grid), vec![(5, 0)]);
    let replies: Vec<String> = grid
        .pending_messages_to_pty
        .iter()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .collect();
    assert!(
        !replies.iter().any(|reply| reply.contains("EINVAL")),
        "d=r with omitted lower bound must not emit EINVAL, got {:?}",
        replies
    );
}

#[test]
fn kitty_retransmit_to_existing_id_replaces_image() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=32,s=4,v=4,i=1,C=1", &rgba_raster(4, 4)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 64);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=t,f=32,s=2,v=2,i=1", &rgba_raster(2, 2)),
    );
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(kitty_image_store.borrow().total_bytes(), 16);
}

#[test]
fn kitty_c_r_scaling_produces_exact_cell_rect_and_variant() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=10,c=3,r=2,i=1", &rgb_raster(10, 10)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    let placement = &grid.kitty_placements()[0];
    assert_eq!(placement.dest_cells, (3, 2));
    assert_eq!(
        placement.display_rect,
        PixelRect {
            x: 0,
            y: 0,
            width: 30,
            height: 40,
        }
    );
    let internal_id = placement.internal_id;
    assert_eq!(
        kitty_image_store
            .borrow()
            .scaled_variant(internal_id, (3, 2))
            .unwrap()
            .len(),
        30 * 40 * 4
    );
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn kitty_aspect_fit_single_axis() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=10,c=4,i=1", &rgb_raster(10, 10)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    let placement = &grid.kitty_placements()[0];
    assert_eq!(placement.dest_cells, (4, 2));
    assert_eq!(
        placement.display_rect,
        PixelRect {
            x: 0,
            y: 0,
            width: 40,
            height: 40,
        }
    );
}

#[test]
fn kitty_yazi_kgpold_stream_roundtrip() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    let raster = rgb_raster(2, 2);
    let full_b64 = BASE64_ENCODER.encode(&raster);
    let (b64_chunk_one, b64_chunk_two) = full_b64.split_at(8);
    let mut stream = Vec::new();
    stream.extend_from_slice(
        format!(
            "\x1b_Gq=2,a=T,z=-1,C=1,f=24,s=2,v=2,m=1;{}\x1b\\",
            b64_chunk_one
        )
        .as_bytes(),
    );
    stream.extend_from_slice(format!("\x1b_Gm=0;{}\x1b\\", b64_chunk_two).as_bytes());
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, &stream);
    assert_eq!(grid.kitty_placement_count(), 1);
    let placement = &grid.kitty_placements()[0];
    assert_eq!(placement.z_index, -1);
    assert_eq!(placement.image_id, 0);
    let (cursor_x, cursor_y, _) = grid.cursor_coordinates().unwrap();
    assert_eq!((cursor_x, cursor_y), (0, 0));
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Gq=2,a=d,d=A\x1b\\",
    );
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_csi_2j_clears_placements() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2J");
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_ris_clears_placements_and_store() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1bc");
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_image_in_alternate_buffer() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[?1049h");
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_snapshot!(format!("{:?}", grid));
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=32,s=10,v=20,i=2,C=1", &rgba_raster(10, 20)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(kitty_image_store.borrow().image_count(), 2);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[?1049l");
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_snapshot!(format!("{:?}", grid));
    assert_eq!(
        kitty_image_store.borrow().image_count(),
        1,
        "alternate screen image was freed when leaving the alternate screen"
    );
}

#[test]
fn kitty_placements_are_reaped_when_scrolled_off() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    for _ in 0..10_040 {
        grid.add_canonical_line();
    }
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_scroll_region_moves_inside_placement_and_clips_straddler() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2;5r");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,i=1,C=1", &rgb_raster(10, 20)),
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[5;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=40,i=2,C=1", &rgb_raster(10, 40)),
    );
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 0,
            y: 40,
            width: 10,
            height: 20,
        }
    );
    assert_eq!(
        grid.kitty_placements()[1].display_rect,
        PixelRect {
            x: 0,
            y: 80,
            width: 10,
            height: 40,
        }
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\n");
    assert_eq!(grid.kitty_placement_count(), 2);
    let placement_a = &grid.kitty_placements()[0];
    assert_eq!(
        placement_a.display_rect,
        PixelRect {
            x: 0,
            y: 20,
            width: 10,
            height: 20,
        }
    );
    assert_eq!(placement_a.emit_y, 0);
    let placement_b = &grid.kitty_placements()[1];
    assert_eq!(
        placement_b.display_rect,
        PixelRect {
            x: 0,
            y: 100,
            width: 10,
            height: 20,
        }
    );
    assert_eq!(placement_b.emit_y, 20);
    assert_eq!(placement_b.emit_x, 0);
}

#[test]
fn kitty_reply_query_probe_exact_bytes() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\",
    );
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
    assert_eq!(grid.kitty_placement_count(), 0);
}

#[test]
fn kitty_reply_display_unknown_id_enoent() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=p,i=10", &[]),
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=10;ENOENT"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn kitty_reply_quiet_one_suppresses_ok_not_errors() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=2,v=2,i=1,q=1,C=1", &rgb_raster(2, 2)),
    );
    assert!(grid.pending_messages_to_pty.is_empty());
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=p,i=99,q=1", &[]),
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert!(reply.contains("i=99;ENOENT"));
}

#[test]
fn kitty_reply_quiet_two_suppresses_everything() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=2,v=2,i=1,q=2,C=1", &rgb_raster(2, 2)),
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=p,i=99,q=2", &[]),
    );
    assert!(grid.pending_messages_to_pty.is_empty());
}

#[test]
fn kitty_reply_transmit_with_image_number_echoes_assigned_id() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=t,I=5,f=24,s=2,v=2", &rgb_raster(2, 2)),
    );
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![format!("\u{1b}_Gi={},I=5;OK\u{1b}\\", u32::MAX).into_bytes()]
    );
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
}

#[test]
fn kitty_reply_anonymous_transmit_and_display_is_silent() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=2,v=2,C=1", &rgb_raster(2, 2)),
    );
    assert!(grid.pending_messages_to_pty.is_empty());
    assert_eq!(grid.kitty_placement_count(), 1);
}

#[test]
fn kitty_reply_malformed_control_einval_without_ids() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Gnot-a-kv\x1b\\",
    );
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1b_G;EINVAL:malformed control data\x1b\\".to_vec()]
    );
}

#[test]
fn kitty_reply_icat_detection_sequence() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    let dir = tempfile::tempdir().unwrap();
    // The marker in the name is half of what licenses a `t=t` transfer to delete the file; the
    // temp directory is the other half. See `should_delete_temp_file`.
    let path = dir.path().join("tty-graphics-protocol-icat-detect.bin");
    std::fs::write(&path, [1u8, 2, 3]).unwrap();
    let path_bytes = path.to_str().unwrap().as_bytes();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Gt=d,a=q,i=1,s=1,v=1,f=24,S=3;MTIz\x1b\\",
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        format!(
            "\x1b_Gt=t,a=q,i=2,s=1,v=1,f=24,S={};{}\x1b\\",
            path_bytes.len(),
            BASE64_ENCODER.encode(path_bytes)
        )
        .as_bytes(),
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        format!(
            "\x1b_Gt=s,a=q,i=3,s=1,v=1,f=24,S=3;{}\x1b\\",
            BASE64_ENCODER.encode(b"/some-shm")
        )
        .as_bytes(),
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 3);
    assert_eq!(
        grid.pending_messages_to_pty[0],
        b"\x1b_Gi=1;OK\x1b\\".to_vec()
    );
    assert_eq!(
        grid.pending_messages_to_pty[1],
        b"\x1b_Gi=2;OK\x1b\\".to_vec()
    );
    let third = String::from_utf8(grid.pending_messages_to_pty[2].clone()).unwrap();
    // The shared memory medium is supported, so what this probe finds out is that the object it
    // named is not there - which is what tells icat the medium works at all.
    assert!(third.starts_with("\x1b_Gi=3;EBADF"));
    assert!(third.ends_with("\x1b\\"));
    assert!(path.starts_with(std::env::temp_dir()));
    assert!(!path.exists());
}

#[test]
fn kitty_reply_query_when_host_unsupported() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    grid.update_kitty_host_support(KittyHostSupport::Unsupported);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\",
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_disabled_protocol_answers_queries_with_silence() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    grid.update_kitty_host_support(KittyHostSupport::ProtocolDisabled);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\",
    );
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "a disabled protocol must not reply to queries at all"
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_disabled_protocol_ignores_transmitted_images() {
    let (mut grid, kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    grid.update_kitty_host_support(KittyHostSupport::ProtocolDisabled);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
    assert!(grid.pending_messages_to_pty.is_empty());
}

#[test]
fn kitty_disabled_protocol_does_not_leak_apc_bytes_into_the_grid() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(20, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    grid.update_kitty_host_support(KittyHostSupport::ProtocolDisabled);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"a");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"b");
    assert_eq!(row_text(&grid.viewport[0]), "ab");
}

#[test]
fn kitty_conformance_image_put_cursor_positions() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,i=1", &rgb_raster(10, 20)),
    );
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((1, 0))
    );
}

#[test]
fn kitty_conformance_image_put_scaled_cursor() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,c=3,i=1", &rgb_raster(20, 40)),
    );
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((3, 2))
    );
    assert_eq!(grid.kitty_placements()[0].dest_cells, (3, 3));
}

#[test]
fn kitty_conformance_image_put_full_width_wraps() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=100,v=20,i=1", &rgb_raster(100, 20)),
    );
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((0, 1))
    );
}

#[test]
fn kitty_conformance_image_put_pixel_offsets_cursor() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,X=5,Y=5,i=1", &rgb_raster(10, 20)),
    );
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((2, 1))
    );
    assert_eq!(grid.kitty_placements()[0].dest_cells, (2, 2));
}

#[test]
fn kitty_conformance_bottom_row_placement_scrolls() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[5;1H");
    assert_eq!(grid.lines_above.len(), 0);
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=40,i=1", &rgb_raster(10, 40)),
    );
    assert_eq!(grid.lines_above.len(), 1);
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((1, 4))
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 0,
            y: 80,
            width: 10,
            height: 40,
        }
    );
}

#[test]
fn kitty_conformance_bottom_row_placement_c1_does_not_scroll() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[5;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=40,i=1,C=1", &rgb_raster(10, 40)),
    );
    assert_eq!(grid.lines_above.len(), 0);
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((0, 4))
    );
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        PixelRect {
            x: 0,
            y: 80,
            width: 10,
            height: 40,
        }
    );
}

#[test]
fn kitty_conformance_full_screen_index_scroll_reaps_image() {
    let (mut grid, kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,i=1,C=1", &rgb_raster(10, 20)),
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    let scroll_buffer_size = *SCROLL_BUFFER_SIZE.get().unwrap();
    for _ in 0..(scroll_buffer_size + grid.height) {
        grid.add_canonical_line();
    }
    let _ = grid.read_changes(0, 0);
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_conformance_margin_scroll_leaves_outside_images_untouched() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,i=1,C=1", &rgb_raster(10, 20)),
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2;4r");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[4;1H");
    let outside_before = grid.kitty_placements()[0].display_rect;
    for _ in 0..3 {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\n");
    }
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(grid.kitty_placements()[0].display_rect, outside_before);
    assert_eq!(grid.kitty_placements()[0].emit_y, 0);
}

#[test]
fn kitty_conformance_transmit_with_both_id_and_number_is_einval() {
    let (mut grid, kitty_image_store) = new_kitty_grid(5, 10);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=t,f=24,s=1,v=1,I=1,i=3", &rgb_raster(1, 1)),
    );
    assert_eq!(grid.kitty_placement_count(), 0);
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert!(
        reply.contains("EINVAL"),
        "expected EINVAL for transmit with both i and I, got {:?}",
        reply
    );
}

#[test]
fn kitty_placement_keeps_screen_row_across_width_reflow() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(6, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    for line in 0..5 {
        let text = format!("row{}-{}", line, "x".repeat(30));
        feed_kitty_bytes(
            &mut grid,
            &mut vte_parser,
            &mut interceptor,
            text.as_bytes(),
        );
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\r\n");
    }
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    let anchor_canonical_line = grid.kitty_placements()[0].vertical_anchor.canonical_line;
    let px_below_before = kitty_image_px_below_canonical_line(&mut grid, anchor_canonical_line);
    let lines_above_before = grid.lines_above.len();

    grid.change_size(6, 10);
    assert_ne!(
        grid.lines_above.len(),
        lines_above_before,
        "width reflow must change the scrollback row count for this test to be meaningful"
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        kitty_image_px_below_canonical_line(&mut grid, anchor_canonical_line),
        px_below_before,
        "image must keep the same pixel offset below its anchor canonical line (track the text) across a narrowing width reflow"
    );

    grid.change_size(6, 40);
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        kitty_image_px_below_canonical_line(&mut grid, anchor_canonical_line),
        px_below_before,
        "image must keep the same pixel offset below its anchor canonical line (track the text) across a widening width reflow"
    );
}

#[test]
fn kitty_placement_follows_text_across_width_reflow() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    for line in 0..3 {
        let text = format!("r{}-{}", line, "x".repeat(34));
        feed_kitty_bytes(
            &mut grid,
            &mut vte_parser,
            &mut interceptor,
            text.as_bytes(),
        );
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\r\n");
    }
    let image_canonical_line = 3usize;
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[7;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );

    let image_row_before = kitty_image_absolute_cell_row(&mut grid);
    let text_row_before = buffer_cell_row_of_canonical_line(&grid, image_canonical_line);
    assert_eq!(
        image_row_before, text_row_before,
        "image top must sit on the buffer cell row of its canonical line before reflow"
    );

    grid.change_size(10, 10);
    assert_eq!(grid.kitty_placement_count(), 1);
    let image_row_narrow = kitty_image_absolute_cell_row(&mut grid);
    let text_row_narrow = buffer_cell_row_of_canonical_line(&grid, image_canonical_line);
    assert_eq!(
        image_row_narrow, text_row_narrow,
        "image must track its canonical text line after narrowing reflow (lines above rewrap into more rows)"
    );
    assert!(
        text_row_narrow > text_row_before,
        "narrowing must push the image's canonical line further down the buffer for the test to be meaningful"
    );

    grid.change_size(10, 40);
    assert_eq!(grid.kitty_placement_count(), 1);
    let image_row_wide = kitty_image_absolute_cell_row(&mut grid);
    let text_row_wide = buffer_cell_row_of_canonical_line(&grid, image_canonical_line);
    assert_eq!(
        image_row_wide, text_row_wide,
        "image must track its canonical text line after widening reflow (lines above rewrap into fewer rows)"
    );
    assert!(
        text_row_wide < text_row_narrow,
        "widening must pull the image's canonical line back up for the test to be meaningful"
    );
}

#[test]
fn kitty_placement_moves_with_csi_insert_lines() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[6;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    let row_before = kitty_image_absolute_cell_row(&mut grid);
    assert_eq!(row_before, 5);

    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    let inserted = 2usize;
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2L");

    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        row_before + inserted as isize,
        "image below the insertion point must shift down by the inserted line count"
    );
}

#[test]
fn kitty_placement_moves_with_csi_delete_lines() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[6;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    let row_before = kitty_image_absolute_cell_row(&mut grid);
    assert_eq!(row_before, 5);

    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    let deleted = 2usize;
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2M");

    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        row_before - deleted as isize,
        "image below the deletion point must shift up by the deleted line count"
    );
}

#[test]
fn kitty_placement_in_deleted_range_is_removed() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[4;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(kitty_image_absolute_cell_row(&mut grid), 3);

    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[4;1H");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1M");

    assert_eq!(
        grid.kitty_placement_count(),
        0,
        "an image whose only row is deleted by CSI M must be removed"
    );
}

#[test]
fn kitty_placement_tracks_text_when_a_wrapped_row_moves_into_the_scrollback() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(5, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"AAAAAAAAAAAAAAAAAAAAAAAAA\r\n",
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"MARKER");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[3;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(
        Some(kitty_image_absolute_cell_row(&mut grid)),
        kitty_marker_absolute_cell_row(&grid)
    );
    for _ in 0..5 {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\r\nx");
    }
    assert!(kitty_scrollback_has_merged_wrapped_rows(&grid));
    assert_eq!(
        Some(kitty_image_absolute_cell_row(&mut grid)),
        kitty_marker_absolute_cell_row(&grid),
        "the image must stay on the row of its marker line after a wrapped row is merged into the scrollback"
    );
}

#[test]
fn kitty_placement_tracks_text_when_a_wrapped_row_is_split_out_of_the_scrollback() {
    let (mut grid, _kitty_image_store, _vte_parser, _interceptor) =
        kitty_wrapped_scrollback_setup(25);
    for step in 0..2 {
        grid.scroll_up_one_line();
        assert_eq!(
            grid.kitty_placement_count(),
            1,
            "scrolling the scrollback must not free the image (step {})",
            step
        );
        assert_eq!(
            Some(kitty_image_absolute_cell_row(&mut grid)),
            kitty_marker_absolute_cell_row(&grid),
            "the image must stay on the row of its marker line after scrollback scroll up (step {})",
            step
        );
    }
    assert!(
        !grid.viewport[0].is_canonical,
        "the wrapped scrollback row must have been split back into the viewport"
    );
}

#[test]
fn kitty_placement_tracks_text_when_a_multiply_wrapped_row_is_split_out_of_the_scrollback() {
    let (mut grid, _kitty_image_store, _vte_parser, _interceptor) =
        kitty_wrapped_scrollback_setup(45);
    let mut non_canonical_viewport_rows = 0;
    for step in 0..4 {
        grid.scroll_up_one_line();
        assert_eq!(
            grid.kitty_placement_count(),
            1,
            "scrolling the scrollback must not free the image (step {})",
            step
        );
        assert_eq!(
            Some(kitty_image_absolute_cell_row(&mut grid)),
            kitty_marker_absolute_cell_row(&grid),
            "the image must stay on the row of its marker line after scrollback scroll up (step {})",
            step
        );
        non_canonical_viewport_rows = grid.viewport.iter().filter(|row| !row.is_canonical).count();
    }
    assert!(
        non_canonical_viewport_rows >= 2,
        "a canonical line wrapping into three rows must be split into at least two continuation rows in the viewport"
    );
}

#[test]
fn kitty_placement_geometry_survives_a_scrollback_scroll_round_trip() {
    let (mut grid, _kitty_image_store, _vte_parser, _interceptor) =
        kitty_wrapped_scrollback_setup(45);
    let display_rect_before = grid.kitty_placements()[0].display_rect;
    let anchor_before = grid.kitty_placements()[0].vertical_anchor;
    let lines_above_before = grid.lines_above.len();
    let marker_row_before = kitty_marker_absolute_cell_row(&grid);
    for _ in 0..4 {
        grid.scroll_up_one_line();
    }
    for _ in 0..4 {
        grid.scroll_down_one_line();
    }
    assert_eq!(grid.lines_above.len(), lines_above_before);
    assert_eq!(
        kitty_marker_absolute_cell_row(&grid),
        marker_row_before,
        "the round trip must restore the marker line row"
    );
    assert_eq!(
        grid.kitty_placements()[0].display_rect,
        display_rect_before,
        "the round trip must restore the placement pixel geometry"
    );
    assert_eq!(
        grid.kitty_placements()[0].vertical_anchor,
        anchor_before,
        "the round trip must restore the placement anchor"
    );
}

#[test]
fn kitty_placement_anchor_survives_scroll_buffer_front_eviction() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    let scroll_buffer_size = *SCROLL_BUFFER_SIZE.get().unwrap();
    for _ in 0..(scroll_buffer_size + grid.height) {
        grid.add_canonical_line();
    }
    assert_eq!(
        grid.lines_above.len(),
        scroll_buffer_size,
        "the scroll buffer must be full for this test to be meaningful"
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"MARKER");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(
        Some(kitty_image_absolute_cell_row(&mut grid)),
        kitty_marker_absolute_cell_row(&grid)
    );
    let anchored_line = grid.kitty_placements()[0].vertical_anchor.canonical_line;
    assert_eq!(
        anchored_line,
        kitty_marker_canonical_line_index(&grid).expect("marker must exist")
    );
    let front_drops_before = grid.kitty_grid.front_drops();

    grid.change_size(5, 20);

    let evicted_lines = grid.kitty_grid.front_drops() - front_drops_before;
    assert!(
        evicted_lines >= 5,
        "shrinking the viewport must evict rows from the front of a full scroll buffer"
    );
    assert_eq!(
        grid.kitty_placement_count(),
        1,
        "a placement below the evicted rows must survive the eviction"
    );
    let marker_canonical_line =
        kitty_marker_canonical_line_index(&grid).expect("marker must still exist");
    assert_eq!(
        grid.kitty_placements()[0].vertical_anchor.canonical_line,
        marker_canonical_line,
        "the placement anchor must still resolve to its marker text line after front eviction"
    );
    assert_eq!(
        Some(kitty_image_absolute_cell_row(&mut grid)),
        kitty_marker_absolute_cell_row(&grid),
        "the image must stay on the row of its marker line across scroll buffer front eviction"
    );
}

#[test]
fn kitty_top_anchored_scroll_up_keeps_image_in_scrollback() {
    let (mut grid, kitty_image_store, mut vte_parser, mut interceptor) =
        kitty_marker_with_image_on_first_row(5, 20);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[S");
    assert_eq!(
        grid.lines_above.len(),
        1,
        "a top anchored scroll region moves the top row into the scrollback"
    );
    assert_eq!(
        grid.kitty_placement_count(),
        1,
        "an image whose text was preserved into the scrollback must not be freed"
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(kitty_marker_absolute_cell_row(&grid), Some(0));
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        0,
        "the image must follow its text into the scrollback"
    );
    assert_eq!(grid.kitty_placements()[0].display_rect.height, 20);
    assert_eq!(grid.kitty_placements()[0].emit_y, 0);

    grid.scroll_up_one_line();
    assert_eq!(grid.lines_above.len(), 0);
    assert_eq!(kitty_marker_absolute_cell_row(&grid), Some(0));
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        0,
        "the image must still sit on its marker row when scrolled back into view"
    );
}

#[test]
fn kitty_top_anchored_delete_line_keeps_image_in_scrollback() {
    let (mut grid, kitty_image_store, mut vte_parser, mut interceptor) =
        kitty_marker_with_image_on_first_row(5, 20);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1M");
    assert_eq!(
        grid.lines_above.len(),
        1,
        "a top anchored line deletion moves the deleted row into the scrollback"
    );
    assert_eq!(
        grid.kitty_placement_count(),
        1,
        "an image whose text was preserved into the scrollback must not be freed"
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 1);
    assert_eq!(kitty_marker_absolute_cell_row(&grid), Some(0));
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        0,
        "the image must follow its text into the scrollback"
    );

    grid.scroll_up_one_line();
    assert_eq!(kitty_marker_absolute_cell_row(&grid), Some(0));
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        0,
        "the image must still sit on its marker row when scrolled back into view"
    );
}

#[test]
fn kitty_non_top_anchored_region_scroll_still_frees_image() {
    let (mut grid, kitty_image_store) = new_kitty_grid(5, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2;5r");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[2;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(kitty_image_absolute_cell_row(&mut grid), 1);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[S");
    assert_eq!(
        grid.lines_above.len(),
        0,
        "a region that does not start at the top of the viewport discards its top row"
    );
    assert_eq!(
        grid.kitty_placement_count(),
        0,
        "an image scrolled out of a non top anchored region must be freed"
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_alternate_screen_top_anchored_scroll_frees_image() {
    let (mut grid, kitty_image_store) = new_kitty_grid(5, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[?1049h");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(kitty_image_absolute_cell_row(&mut grid), 0);
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[S");
    assert_eq!(
        grid.lines_above.len(),
        0,
        "the alternate screen has no scrollback"
    );
    assert_eq!(
        grid.kitty_placement_count(),
        0,
        "an image scrolled off the alternate screen must be freed"
    );
    assert_eq!(kitty_image_store.borrow().image_count(), 0);
}

#[test]
fn kitty_placement_rejoins_its_text_after_editing_while_scrolled_back() {
    let (mut grid, _kitty_image_store) = new_kitty_grid(10, 40);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    for line in 0..20 {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[10;1H");
        let text = format!("\r\npad{}", line);
        feed_kitty_bytes(
            &mut grid,
            &mut vte_parser,
            &mut interceptor,
            text.as_bytes(),
        );
    }
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b[10;1H\r\nMARKER",
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[10;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    for line in 0..3 {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[10;1H");
        let text = format!("\r\ntail{}", line);
        feed_kitty_bytes(
            &mut grid,
            &mut vte_parser,
            &mut interceptor,
            text.as_bytes(),
        );
    }
    let _ = grid.read_changes(0, 0);
    for _ in 0..8 {
        grid.scroll_up_one_line();
    }
    let _ = grid.read_changes(0, 0);
    assert!(
        kitty_marker_is_below_the_viewport(&grid),
        "the marker line must be scrolled below the viewport for this scenario"
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        b"\x1b[1;1H\x1b[1M",
    );
    let _ = grid.read_changes(0, 0);
    grid.reset_viewport();
    let _ = grid.read_changes(0, 0);
    assert!(
        !kitty_marker_is_below_the_viewport(&grid),
        "the marker line must be back inside the buffer after restoring the viewport"
    );
    let marker_row = kitty_marker_extended_absolute_cell_row(&grid)
        .expect("the marker line must survive the edit");
    assert_eq!(
        grid.kitty_placement_count(),
        1,
        "the image must survive an edit made while scrolled back"
    );
    assert_eq!(
        kitty_image_absolute_cell_row(&mut grid),
        marker_row,
        "the image must rejoin its marker line once the viewport is restored"
    );
}
