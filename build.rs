fn main() {
    // Rebuild when a translation changes, not only when a .slint file does.
    println!("cargo:rerun-if-changed=lang");
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_bundled_translations("lang")
            // The .po files carry no msgctxt, so strings match by text alone.
            .with_default_translation_context(slint_build::DefaultTranslationContext::None),
    )
    .expect("Slint compilation failed");

    // Windows: the icon shown for thulli.exe in Explorer, pins and shortcuts.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon/thulli.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/thulli.ico");
        res.compile().expect("embedding the app icon failed");
    }
}
