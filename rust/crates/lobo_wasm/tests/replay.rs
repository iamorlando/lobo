#[test]
fn shaders_validate_without_gpu_readback() {
    use lobo_wasm::presentation;
    for source in [
        include_str!("../src/compute.wgsl").to_owned(),
        presentation::display_shader(false),
        presentation::display_shader(true),
        presentation::themed_shader(include_str!("../src/bars.wgsl"), false),
        presentation::themed_shader(include_str!("../src/bars.wgsl"), true),
        presentation::themed_shader(include_str!("../src/loading.wgsl"), false),
        presentation::themed_shader(include_str!("../src/loading.wgsl"), true),
    ] {
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
        // The atomic compute view and read-only display view must have the
        // same stride as the renderer's allocation and GPU-to-GPU copies.
        for (_, ty) in module.types.iter() {
            let expected = match ty.name.as_deref() {
                Some("Book") => presentation::BOOK_BYTES,
                Some("History") => presentation::HISTORY_BYTES,
                Some("Theme") => presentation::THEME_BYTES,
                _ => continue,
            };
            let naga::TypeInner::Struct { span, .. } = ty.inner else {
                panic!("GPU buffer element is not a struct")
            };
            assert_eq!(u64::from(span), expected, "{:?}", ty.name);
        }
    }
}
