use super::*;

#[test]
fn live_source_device_lost_is_the_only_fatal_vulkan_present_error() {
    assert!(vk_result_is_device_lost(vk::Result::ERROR_DEVICE_LOST));
    assert!(!vk_result_is_device_lost(
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
    ));
    assert!(present_error_is_device_lost(&PresentError::Vk(
        vk::Result::ERROR_DEVICE_LOST,
    )));
    assert!(!present_error_is_device_lost(&PresentError::Vk(
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
    )));
}

#[test]
fn stationary_cursor_same_rect_mode_and_version_adds_no_damage() {
    let damage = cursor_damage_for_frame(
        Some(rect(10, 20, 16, 16)),
        Some(7),
        Some(rect(10, 20, 16, 16)),
        Some(7),
        false,
    );
    assert!(
        damage.is_empty(),
        "stationary cursor must not keep the output dirty"
    );
}

#[test]
fn moved_sw_cursor_damages_old_and_new_rects() {
    let old = rect(10, 20, 16, 16);
    let new = rect(30, 40, 16, 16);
    let damage = cursor_damage_for_frame(Some(old), Some(7), Some(new), Some(7), false);
    let rects = damage.rects();
    assert!(rects.contains(&old), "old rect must be cleared");
    assert!(rects.contains(&new), "new rect must be painted");
}

#[test]
fn sprite_swap_on_stationary_cursor_damages_once() {
    let rect = rect(10, 20, 16, 16);
    let damage = cursor_damage_for_frame(Some(rect), Some(7), Some(rect), Some(8), false);
    assert_eq!(damage.rects(), &[rect]);
}

#[test]
// Mutation: ignore cursor mode changes in damage calculation; this must fail.
fn mode_change_on_stationary_cursor_damages_its_footprint() {
    let rect = rect(10, 20, 16, 16);
    let damage = cursor_damage_for_frame(Some(rect), Some(7), Some(rect), Some(7), true);
    assert_eq!(damage.rects(), &[rect]);
}

#[test]
fn actual_visibility_reconciles_only_hardware_binding_state() {
    let desired_sw = CursorAssignment::Sw { pos: (100, 100) };
    assert_eq!(
        effective_cursor_prev_mode(OutputCursorMode::Hidden, true, desired_sw),
        OutputCursorMode::Hw,
    );
    let desired_hidden = CursorAssignment::Hidden;
    assert_eq!(
        effective_cursor_prev_mode(
            OutputCursorMode::Sw {
                prev: Some((10, 20))
            },
            true,
            desired_hidden,
        ),
        OutputCursorMode::Hw,
    );
    assert_eq!(
        effective_cursor_prev_mode(OutputCursorMode::Hw, false, desired_sw),
        OutputCursorMode::Hidden,
    );
}

#[test]
fn classify_cursor_mode_dual_output_cursor_on_one_monitor_is_hw() {
    assert_eq!(
        classify_cursor_mode_from_per_output([OutputCursorMode::Hw, OutputCursorMode::Hidden]),
        CursorPlaneMode::Hw,
    );
    assert_eq!(
        classify_cursor_mode_from_per_output([OutputCursorMode::Hidden, OutputCursorMode::Hw]),
        CursorPlaneMode::Hw,
    );
}

#[test]
fn classify_cursor_mode_single_output_cases() {
    assert_eq!(
        classify_cursor_mode_from_per_output([OutputCursorMode::Hw]),
        CursorPlaneMode::Hw,
    );
    assert_eq!(
        classify_cursor_mode_from_per_output([OutputCursorMode::Hidden]),
        CursorPlaneMode::Hidden,
    );
    assert_eq!(
        classify_cursor_mode_from_per_output([OutputCursorMode::Sw { prev: None }]),
        CursorPlaneMode::Sw,
    );
}

#[test]
// Mutation: classify all-Hidden outputs as Sw. The first assertion must fail.
fn cursor_mode_hidden_outputs_and_mixed_hidden_modes() {
    let Some(gpu) = crate::kms::render::gpu::GpuCore::for_tests_with_vk() else {
        eprintln!("skipping: no Vulkan test device");
        return;
    };
    let mut sink = TestSink::new();
    sink.gpu = gpu;
    let layouts = [layout_at((0, 0, 800, 600)), layout_at((800, 0, 800, 600))];
    let mut scene = SceneCompositor::new(&sink, &layouts).expect("construct live test scene");

    assert_eq!(scene.cursor_mode(), CursorPlaneMode::Hidden);

    scene.inner.as_mut().unwrap().outputs[1].last_frame_cursor_mode =
        OutputCursorMode::Sw { prev: None };
    assert_eq!(scene.cursor_mode(), CursorPlaneMode::Sw);

    scene.inner.as_mut().unwrap().outputs[1].last_frame_cursor_mode = OutputCursorMode::Hw;
    assert_eq!(scene.cursor_mode(), CursorPlaneMode::Hw);
}

#[test]
// Mutation: treat Hidden as immovable; re-entry must assign the cursor to output 1.
fn cursor_hidden_in_dead_zone_is_assigned_to_reentered_output() {
    let mut sink = TestSink::new();
    let Some(gpu) = GpuCore::for_tests_with_vk() else {
        eprintln!("skipping: no Vulkan test device");
        return;
    };
    sink.gpu = gpu;
    let outputs = [layout_at((0, 0, 800, 600)), layout_at((900, 0, 800, 400))];
    let mut scene = SceneCompositor::new(&sink, &outputs).expect("construct live test scene");
    let mut core = TestCore::for_tests();
    let windows = crate::kms::render::window_geometry::WindowsMap::new();
    let mut store = crate::kms::render::store::DrawableStore::new();
    let mut cursor_storage = crate::kms::render::store::Storage::for_tests_null(
        extent(16, 16),
        vk::Format::B8G8R8A8_UNORM,
    );
    let cursor_view: vk::ImageView = ash::vk::Handle::from_raw(0xCAFE_BA11);
    cursor_storage.image_view = cursor_view;
    cursor_storage.sample_view = cursor_view;
    let cursor_id = store
        .allocate(
            0xCAFE_BA12,
            crate::kms::render::store::DrawableKind::Pixmap,
            32,
            false,
            cursor_storage,
        )
        .expect("allocate cursor sprite");
    scene.register_cursor(CursorEntry {
        id: cursor_id,
        extent: extent(16, 16),
        hot_x: 0,
        hot_y: 0,
        record_version: 1,
        bgra_bytes: None,
    });

    core.cursor_x = 850.0;
    core.cursor_y = 450.0;
    let dead_zone_assignments: Vec<_> = (0..outputs.len())
        .map(|output_idx| {
            scene.cursor_assignment_for_tests(
                &core.input(&windows, &outputs),
                &mut store,
                &sink,
                output_idx,
            )
        })
        .collect();
    assert_eq!(
        dead_zone_assignments,
        vec![CursorAssignment::Hidden, CursorAssignment::Hidden],
    );

    core.cursor_x = 950.0;
    core.cursor_y = 350.0;
    let input = core.input(&windows, &outputs);
    assert_eq!(
        scene.cursor_assignment_for_tests(&input, &mut store, &sink, 0),
        CursorAssignment::Hidden,
    );
    assert_eq!(
        scene.cursor_assignment_for_tests(&input, &mut store, &sink, 1),
        CursorAssignment::Sw { pos: (50, 350) },
    );
}

#[test]
fn classify_cursor_mode_hw_and_sw_is_mixed() {
    assert_eq!(
        classify_cursor_mode_from_per_output([
            OutputCursorMode::Hw,
            OutputCursorMode::Sw { prev: None },
        ]),
        CursorPlaneMode::Mixed,
    );
}

#[test]
fn classify_cursor_mode_no_outputs_is_sw() {
    let empty: [OutputCursorMode; 0] = [];
    assert_eq!(
        classify_cursor_mode_from_per_output(empty),
        CursorPlaneMode::Sw,
    );
}

#[test]
fn stub_cursor_mode_defaults_to_software() {
    let scene = SceneCompositor::stub();
    assert_eq!(scene.cursor_mode(), CursorPlaneMode::Sw);
}
